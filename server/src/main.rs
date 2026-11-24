//! M2 server: capture the primary monitor, encode H.264, send over UDP.
//!
//! ```text
//! cargo run -p server --release -- --dest 0.0.0.0:9100
//! ```!
//! 
//! Flags:
//!   `--dest <ip:port>` (default 0.0.0.0:9100, where video is sent)
//!   `--bind <ip:port>` (default 0.0.0.0:9000, also receives keyframe requests)
//!   `--seconds <n>` (default 0 = run until Ctrl+C)
//!   `--width <W>` (default 1280, encode/stream width)
//!   `--height <H>` (default 720, encode/stream height)
//!   `--fps <n>` (default 30, frames per second)
//!   `--encode-path <name>` (default "auto", auto-detect: nvenc>qsv>amf>libx264; force: nvenc|qsv|amf|libx264)
//!   `--monitor <index>` (default 0, DXGI output index to capture (M6))
//!   `--list-monitors` (print each monitor's index, name, resolution, and position, then exit (M6))
//!   `--virtual` (create a Virtual Display Driver monitor at --width/--height/--fps, capture it,
//!                and remove it on exit; falls back to the primary monitor if the driver is missing (M7))
//!   `--install-driver` (install the bundled Virtual Display Driver through UAC (M8))
//!   `--uninstall-driver` (remove the Virtual Display Driver and its driver package (M8))
//!   `--driver-status` (print whether the driver is installed and working (M8))
//!   `--help` (print this help message)
mod capture;
mod encoder;
mod input;
mod net;
#[cfg(windows)]
mod driver_install;
#[cfg(windows)]
mod orientation;
#[cfg(windows)]
mod virtual_display;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use common::{DEFAULT_CLIENT_PORT, DEFAULT_SERVER_PORT};

use crate::encoder::{EncodedFrame, Encoder};
use crate::net::NetSender;

fn print_help() {
    eprintln!(
        "lowlat-screen server usage:\n\
         --dest <ip:port>       Destination address for UDP video (default: 0.0.0.0:9100)\n\
         --bind <ip:port>       Local bind address for UDP (default: 0.0.0.0:9000)\n\
         --seconds <n>          Run for n seconds, 0 = forever (default: 0)\n\
         --width <W>            Encode/stream width in pixels (default: 1280)\n\
         --height <H>           Encode/stream height in pixels (default: 720)\n\
         --fps <n>              Frames per second (default: 30)\n\
         --orientation <n>      Display orientation: 0, 90, 180, 270 (default: 0) (M9)\n\
         --encode-path <name>   Video encoder path (default: auto, auto-detect: nvenc>qsv>amf>libx264; force: nvenc|qsv|amf|libx264)\n\
         --monitor <index>      DXGI output index to capture (default: 0) (M6)\n\
         --list-monitors        Print each monitor's index, name, resolution, and position, then exit (M6)\n\
         --virtual              Create a Virtual Display Driver monitor at --width/--height/--fps,\n\
                                capture it, and remove it on exit (Windows; falls back to the\n\
                                primary monitor if the driver is not installed) (M7)\n\
         --install-driver       Install the bundled Virtual Display Driver (asks for admin\n\
                                through UAC) and verify it with SetupAPI (M8)\n\
         --uninstall-driver     Remove the Virtual Display Driver device and driver package\n\
                                (asks for admin through UAC) (M8)\n\
         --driver-status        Print whether the driver is installed and working (M8)\n\
         --help                 Print this help message"
    );
}

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Print help if requested
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        std::process::exit(0);
    }

    // M8: driver lifecycle. These exit before any capture setup.
    // `--elevated` is the marker the re-launched (UAC) child carries.
    if args.iter().any(|a| a == "--install-driver") {
        #[cfg(windows)]
        {
            if let Some(code) = driver_install::ensure_elevated("--install-driver")? {
                std::process::exit(code);
            }
            driver_install::install()?;
            std::process::exit(0);
        }
        #[cfg(not(windows))]
        anyhow::bail!("--install-driver is Windows-only");
    }

    if args.iter().any(|a| a == "--uninstall-driver") {
        #[cfg(windows)]
        {
            if let Some(code) = driver_install::ensure_elevated("--uninstall-driver")? {
                std::process::exit(code);
            }
            driver_install::uninstall()?;
            std::process::exit(0);
        }
        #[cfg(not(windows))]
        anyhow::bail!("--uninstall-driver is Windows-only");
    }

    if args.iter().any(|a| a == "--driver-status") {
        #[cfg(windows)]
        {
            let installed = driver_install::status();
            std::process::exit(if installed { 0 } else { 1 });
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("--driver-status is Windows-only");
        }
    }

    let dest: SocketAddr = value(&args, "--dest")
        .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_CLIENT_PORT}"))
        .parse()
        .context("--dest must look like 192.168.50.2:9100")?;
    let bind: SocketAddr = value(&args, "--bind")
        .unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_SERVER_PORT}"))
        .parse()
        .context("--bind must look like 0.0.0.0:9000")?;

    // M3 flags: encode resolution and fps
    let width: u32 = value(&args, "--width")
        .unwrap_or_else(|| "1280".to_string())
        .parse()
        .context("--width must be a positive integer")?;
    let height: u32 = value(&args, "--height")
        .unwrap_or_else(|| "720".to_string())
        .parse()
        .context("--height must be a positive integer")?;
    let fps: u32 = value(&args, "--fps")
        .unwrap_or_else(|| "30".to_string())
        .parse()
        .context("--fps must be a positive integer")?;
    let mut orientation: u16 = value(&args, "--orientation")
        .unwrap_or_else(|| "0".to_string())
        .parse()
        .context("--orientation must be a number (0, 90, 180, 270)")?;

    // Runtime duration flag: 0 = run forever (default).
    let seconds: u64 = value(&args, "--seconds")
        .unwrap_or_else(|| "0".to_string())
        .parse()
        .context("--seconds must be a number")?;

    // Encoder path: if user specified --encode-path, use it directly;
    // otherwise auto-detect.
    let encode_path = value(&args, "--encode-path");
    let encode_path = match encode_path {
        Some(path) => path,
        None => "auto".to_string(),
    };

    // M6: monitor selection (default 0 = primary)
    let mut monitor_index: u32 = value(&args, "--monitor")
        .unwrap_or_else(|| "0".to_string())
        .parse()
        .context("--monitor must be a non-negative integer")?;

    // M7: create and capture a virtual monitor instead of a physical one.
    let virtual_mode = args.iter().any(|a| a == "--virtual");

    // M6: list monitors and exit
    if args.iter().any(|a| a == "--list-monitors") {
        let monitors = capture::Capture::list_monitors()?;
        for m in monitors {
            eprintln!(
                "monitor {}: {}x{} at ({},{}) name='{}'",
                m.index, m.width, m.height, m.left, m.top, m.name
            );
        }
        std::process::exit(0);
    }

    // M7: attach the virtual monitor before opening the capture. The guard
    // detaches it at the end of `run` (normal exit and panic); Ctrl+C is
    // handled inside the module.
    #[cfg(windows)]
    let _virtual_guard: Option<virtual_display::Guard> = if virtual_mode {
        if virtual_display::is_driver_installed() {
            monitor_index = virtual_display::attach(width, height, fps)
                .context("attaching the virtual display")?;
            Some(virtual_display::Guard::new())
        } else {
            eprintln!(
                "Virtual display driver not installed. Run server.exe --install-driver (needs admin)"
            );
            eprintln!(
                "--virtual: falling back to the primary monitor (index {monitor_index})"
            );
            None
        }
    } else {
        None
    };

    #[cfg(not(windows))]
    if virtual_mode {
        eprintln!("--virtual is Windows-only; falling back to the primary monitor");
    }

    let explicit_orientation = value(&args, "--orientation").is_some();
    if explicit_orientation {
        let monitors = capture::Capture::list_monitors()?;
        if let Some(m) = monitors.get(monitor_index as usize) {
            #[cfg(windows)]
            {
                let _ = orientation::apply_orientation(&m.name, orientation.into(), width, height);
            }
        }
    }

    // Create capture with selected monitor index
    let mut capture = capture::Capture::new(monitor_index)?;

    if orientation == 0 {
        orientation = match capture.rotation() {
            2 => 90,
            3 => 180,
            4 => 270,
            _ => 0,
        };
    }

    // If user did not explicitly specify width & height, use monitor's native capture resolution.
    // In portrait orientation (90° or 270°), the stream is transposed upright so width and height swap.
    let (mut encode_width, mut encode_height) = if value(&args, "--width").is_some() || value(&args, "--height").is_some() {
        (width, height)
    } else if capture.rotation() == 2 || capture.rotation() == 4 || orientation == 90 || orientation == 270 {
        (capture.height(), capture.width())
    } else {
        (capture.width(), capture.height())
    };

    // Print capture info - use the actual captured output's resolution
    println!(
        "capture monitor {}: {}x{}",
        monitor_index,
        capture.width(),
        capture.height()
    );
    println!("capture size: {}x{}", capture.width(), capture.height());
    println!("encoder size: {}x{}", encode_width, encode_height);

    let (frames_tx, frames_rx) = sync_channel::<EncodedFrame>(2);
    let force_keyframe = Arc::new(AtomicBool::new(false));
    let pending_orientation = Arc::new(AtomicU32::new(0xFFFF));
    let (config_tx, config_rx) = sync_channel::<(u32, u32, u16, u8, input::MonitorRect)>(4);
    let monitor_rect = input::MonitorRect {
        left: capture.x(),
        top: capture.y(),
        width: encode_width,
        height: encode_height,
    };
    let net = NetSender::new(bind, dest)?.spawn(
        frames_rx,
        force_keyframe.clone(),
        pending_orientation.clone(),
        config_rx,
        monitor_rect,
        orientation.into(),
        capture.rotation() as u8,
        encode_width,
        encode_height,
    );

    println!(
        "server: bind {}, sending to {}, {} fps target (keyframe listener on {})",
        bind, dest, fps, bind
    );

    // If encode_width/encode_height differ from capture size, we need scaling/transpose
    let mut need_scale = (encode_width != capture.width()) || (encode_height != capture.height());
    if need_scale {
        println!(
            "scaling from capture size {}x{} to encode size {}x{} via ffmpeg -vf scale={}:{}",
            capture.width(),
            capture.height(),
            encode_width,
            encode_height,
            encode_width,
            encode_height
        );
    }

    let mut encoder = Encoder::new(
        capture.width(),
        capture.height(),
        capture.pixel_format(),
        encode_width,
        encode_height,
        fps,
        frames_tx.clone(),
        &encode_path,
        need_scale,
        orientation.into(),
    )?;

    println!("streaming - press Ctrl+C to stop");

    let start = Instant::now();
    let limit = (seconds > 0).then(|| Duration::from_secs(seconds));
    let mut copy_ms: Vec<f32> = Vec::new();
    let mut wait_ms: Vec<f32> = Vec::new();
    let mut encode_ms: Vec<f32> = Vec::new();
    let mut frames_written = 0u64;
    let mut new_frames = 0u64;
    let mut restarts = 0u64;
    let mut last_brightness: f32;
    let mut frame_is_new: &'static str;
    let mut last_report = Instant::now();
    // Absolute deadlines, so a slow frame is paid back over the next ones
    // instead of stretching every frame by its own cost.
    let mut next_deadline = start + Duration::from_secs_f64(1.0 / fps as f64);

    loop {
        if let Some(limit) = limit {
            if start.elapsed() >= limit {
                break;
            }
        }

        // Check for runtime orientation change request from client
        let req_orient = pending_orientation.swap(0xFFFF, Ordering::SeqCst);
        if req_orient != 0xFFFF {
            let new_orient = req_orient as u16;
            #[cfg(windows)]
            {
                let (w, h) = orientation::apply_orientation(&capture.name(), new_orient, encode_width, encode_height);
                encode_width = w;
                encode_height = h;
            }
            orientation = new_orient;
            println!("orientation changed to {new_orient}°, target size {encode_width}x{encode_height}");
            let _ = capture.force_recreate();
            need_scale = (encode_width != capture.width()) || (encode_height != capture.height());
            encoder.finish()?;
            encoder = Encoder::new(
                capture.width(),
                capture.height(),
                capture.pixel_format(),
                encode_width,
                encode_height,
                fps,
                frames_tx.clone(),
                &encode_path,
                need_scale,
                orientation.into(),
            )?;
            println!("capture size: {}x{}", capture.width(), capture.height());
            println!("encoder size: {}x{}", encode_width, encode_height);
            let new_rect = input::MonitorRect {
                left: capture.x(),
                top: capture.y(),
                width: encode_width,
                height: encode_height,
            };
            let _ = config_tx.send((encode_width, encode_height, orientation.into(), capture.rotation() as u8, new_rect));
            next_deadline = Instant::now() + Duration::from_secs_f64(1.0 / fps as f64);
        }

        // A restart gives us a fresh IDR with SPS/PPS, which is how the child
        // process serves keyframe requests. It costs ~100-300 ms of stream.
        if force_keyframe.swap(false, Ordering::Relaxed) {
            restarts += 1;
            encoder.finish()?;
            encoder = Encoder::new(
                capture.width(),
                capture.height(),
                capture.pixel_format(),
                encode_width,
                encode_height,
                fps,
                frames_tx.clone(),
                &encode_path,
                need_scale,
                orientation.into(),
            )?;
            println!("restarted encoder for keyframe request #{restarts}");
            // Do not try to make up the time the restart consumed.
            next_deadline = Instant::now() + Duration::from_secs_f64(1.0 / fps as f64);
        }

        let now = Instant::now();
        if now >= next_deadline {
            // Behind schedule: skip the slots we already missed.
            let behind = now.duration_since(next_deadline).as_secs_f64();
            let missed = (behind / Duration::from_secs_f64(1.0 / fps as f64).as_secs_f64()) as u32 + 1;
            next_deadline += Duration::from_secs_f64(1.0 / fps as f64) * missed;
        }

        let frame_started = Instant::now();
        let timeout = next_deadline.saturating_duration_since(frame_started);
        let acquired = capture.acquire(timeout.as_millis().max(1) as u32)?;

        // A desktop mode change resizes the capture; ffmpeg was told the old
        // input size, so restart it before feeding the new-sized buffer.
        if capture.take_size_change() {
            orientation = match capture.rotation() {
                2 => 90,
                3 => 180,
                4 => 270,
                _ => 0,
            };
            if value(&args, "--width").is_none() && value(&args, "--height").is_none() {
                if capture.rotation() == 2 || capture.rotation() == 4 || orientation == 90 || orientation == 270 {
                    encode_width = capture.height();
                    encode_height = capture.width();
                } else {
                    encode_width = capture.width();
                    encode_height = capture.height();
                }
            }
            need_scale = (encode_width != capture.width()) || (encode_height != capture.height());
            encoder.finish()?;
            encoder = Encoder::new(
                capture.width(),
                capture.height(),
                capture.pixel_format(),
                encode_width,
                encode_height,
                fps,
                frames_tx.clone(),
                &encode_path,
                need_scale,
                orientation.into(),
            )?;
            println!(
                "capture is now {}x{} (orientation {}°), restarted encoder",
                capture.width(),
                capture.height(),
                orientation
            );
            println!("capture size: {}x{}", capture.width(), capture.height());
            println!("encoder size: {}x{}", encode_width, encode_height);
            let new_rect = input::MonitorRect {
                left: capture.x(),
                top: capture.y(),
                width: encode_width,
                height: encode_height,
            };
            let _ = config_tx.send((encode_width, encode_height, orientation.into(), capture.rotation() as u8, new_rect));
        }

        if acquired.new_frame {
            new_frames += 1;
        }
        last_brightness = acquired.brightness;
        frame_is_new = acquired.frame_status;

        let encode_started = Instant::now();
        encoder.write_frame(capture.frame())?;
        let this_encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;

        copy_ms.push((acquired.copy.as_secs_f64() * 1000.0) as f32);
        wait_ms.push((acquired.wait.as_secs_f64() * 1000.0) as f32);
        encode_ms.push(this_encode_ms as f32);
        frames_written += 1;

        let rest = next_deadline.saturating_duration_since(Instant::now());
        if !rest.is_zero() {
            std::thread::sleep(rest);
        }
        next_deadline += Duration::from_secs_f64(1.0 / fps as f64);

        if last_report.elapsed() >= Duration::from_secs(1) {
            println!(
                "[{:>3}s] frames={:<5} new={:<4} brightness={:.2} frame_status={} capture copy avg {:>5.2} ms | wait avg {:>5.2} ms | encode submit avg {:>5.2} ms",
                start.elapsed().as_secs(),
                frames_written,
                new_frames,
                last_brightness,
                frame_is_new,
                avg(&copy_ms),
                avg(&wait_ms),
                avg(&encode_ms),
            );
            copy_ms.clear();
            wait_ms.clear();
            encode_ms.clear();
            last_report = Instant::now();
        }
    }

    encoder.finish()?;
    drop(frames_tx); // lets the udp sender drain and exit
    let _ = net.join();

    println!(
        "done: {frames_written} frames written to the encoder, {restarts} encoder restarts"
    );
    Ok(())
}

fn avg(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}