//! M2 server: capture the primary monitor, encode H.264, send over UDP.
//!
//! ```text
//! cargo run -p server --release -- --dest 127.0.0.1:9100
//! ```!
//! 
//! Flags:
//!   `--dest <ip:port>` (default 127.0.0.1:9100, where video is sent)
//!   `--bind <ip:port>` (default 127.0.0.1:9000, also receives keyframe requests)
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
mod net;
#[cfg(windows)]
mod driver_install;
#[cfg(windows)]
mod virtual_display;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
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
         --dest <ip:port>       Destination address for UDP video (default: 127.0.0.1:9100)\n\
         --bind <ip:port>       Local bind address for UDP (default: 127.0.0.1:9000)\n\
         --seconds <n>          Run for n seconds, 0 = forever (default: 0)\n\
         --width <W>            Encode/stream width in pixels (default: 1280)\n\
         --height <H>           Encode/stream height in pixels (default: 720)\n\
         --fps <n>              Frames per second (default: 30)\n\
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
        .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_SERVER_PORT}"))
        .parse()
        .context("--bind must look like 127.0.0.1:9000")?;

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

    // Create capture with selected monitor index
    let mut capture = capture::Capture::new(monitor_index)?;

    // Print capture info - use the actual captured output's resolution
    println!(
        "capture monitor {}: {}x{}",
        monitor_index,
        capture.width(),
        capture.height()
    );

    let (frames_tx, frames_rx) = sync_channel::<EncodedFrame>(2);
    let force_keyframe = Arc::new(AtomicBool::new(false));
    let net = NetSender::new(bind, dest)?.spawn(frames_rx, force_keyframe.clone());

    println!(
        "server: bind {}, sending to {}, {} fps target (keyframe listener on {})",
        bind, dest, fps, bind
    );

    // If --width/--height differ from capture size, we need scaling later
    let mut need_scale = (width != capture.width()) || (height != capture.height());
    if need_scale {
        println!(
            "scaling from capture size {}x{} to encode size {}x{} via ffmpeg -vf scale={}:{}",
            capture.width(),
            capture.height(),
            width,
            height,
            width,
            height
        );
    }

    let mut encoder = Encoder::new(
        capture.width(),
        capture.height(),
        capture.pixel_format(),
        width,
        height,
        fps,
        frames_tx.clone(),
        &encode_path,
        need_scale,
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
    let mut last_brightness: f32 = 0.0;
    let mut frame_is_new: &'static str = "new";
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

        // A restart gives us a fresh IDR with SPS/PPS, which is how the child
        // process serves keyframe requests. It costs ~100-300 ms of stream.
        if force_keyframe.swap(false, Ordering::Relaxed) {
            restarts += 1;
            encoder.finish()?;
            encoder = Encoder::new(
                capture.width(),
                capture.height(),
                capture.pixel_format(),
                width,
                height,
                fps,
                frames_tx.clone(),
                &encode_path,
                need_scale,
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
            need_scale = (width != capture.width()) || (height != capture.height());
            encoder.finish()?;
            encoder = Encoder::new(
                capture.width(),
                capture.height(),
                capture.pixel_format(),
                width,
                height,
                fps,
                frames_tx.clone(),
                &encode_path,
                need_scale,
            )?;
            println!(
                "capture is now {}x{}, restarted encoder",
                capture.width(),
                capture.height()
            );
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