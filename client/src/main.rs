//! M2 client: receive H.264 over UDP, decode with ffmpeg, show in an SDL window.
//!
//! ```text
//! cargo run -p client --release -- --server 192.168.50.1:9000
//! ```
//!
//! Flags: `--bind <ip:port>` (default 0.0.0.0:9100, where video arrives),
//! `--server <ip:port>` (default 127.0.0.1:9000, where keyframe requests go),
//! `--size <WxH>` (default 1280x720, must match the server's capture size),
//! `--window <WxH>` (default: fit the desktop), `--fullscreen`,
//! `--seconds <n>` (stop after n seconds, for testing).
//! `--width`, `--height`, `--fps` are handled by the server; the client
//! uses the `--size` value for display scaling.

mod decoder;
mod net;
mod stats;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use common::{DEFAULT_CLIENT_PORT, DEFAULT_SERVER_PORT};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;

use crate::decoder::{Decoder, DecodedFrame, RenderStats};
use crate::net::Receiver;
use crate::stats::{take_counter, Stats};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind: SocketAddr = value(&args, "--bind")
        .unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_CLIENT_PORT}"))
        .parse()
        .context("--bind must look like 0.0.0.0:9100")?;
    let server: SocketAddr = value(&args, "--server")
        .unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_SERVER_PORT}"))
        .parse()
        .context("--server must look like 127.0.0.1:9000")?;
    let (width, height) =
        parse_size(&value(&args, "--size").unwrap_or_else(|| "1280x720".to_string()))?;
    let requested_window = value(&args, "--window").map(|text| parse_size(&text)).transpose()?;
    let fullscreen = args.iter().any(|arg| arg == "--fullscreen");
    let seconds: u64 = match value(&args, "--seconds") {
        Some(text) => text.parse().context("--seconds must be a number")?,
        None => 0,
    };

    // Runtime diagnostics: show exactly which ports the processes are using.
    println!(
        "client: video on {bind}, keyframe requests to {server}, stream {width}x{height}"
    );
    println!("client_ports: bind={bind} server={server}");

    let sdl = sdl2::init().map_err(|err| anyhow!("SDL init failed: {err}"))?;
    let video = sdl
        .video()
        .map_err(|err| anyhow!("SDL video init failed: {err}"))?;

    // Default to a window that fits on the screen: the stream may well be
    // 1280x720 on a 1366x768 laptop panel. SDL scales the texture for free.
    let (window_width, window_height) = match requested_window {
        Some(size) => size,
        None => {
            let desktop = video
                .desktop_display_mode(0)
                .map_err(|err| anyhow!("reading the desktop mode failed: {err}"))?;
            let max_width = (desktop.w.max(0) as u32) * 9 / 10;
            let max_height = (desktop.h.max(0) as u32) * 9 / 10;
            (width.min(max_width), height.min(max_height))
        }
    };

    println!(
        "client: video on {bind}, keyframe requests to {server}, stream {width}x{height}, window {window_width}x{window_height}{}",
        if fullscreen { " (fullscreen)" } else { "" }
    );

    let stats = Arc::new(Stats::default());
    let running = Arc::new(AtomicBool::new(true));

    let (decoder, sink) = Decoder::new(width, height)?;
    let receiver = Receiver::new(bind, server, stats.clone(), running.clone())?;
    let net_thread = receiver.spawn(decoder);

    let mut builder = video.window("lowlat-screen", window_width, window_height);
    builder.resizable();
    if fullscreen {
        builder.fullscreen_desktop();
    }
    let window = builder
        .build()
        .map_err(|err| anyhow!("creating the window failed: {err}"))?;
    let mut canvas = window
        .into_canvas()
        .build()
        .map_err(|err| anyhow!("creating the canvas failed: {err}"))?;
    let texture_creator = canvas.texture_creator();
    // Use IYUV planar format: Y plane, then U, then V. No BGRA conversion.
    // SDL_PIXELFORMAT_IYUV layout: [Y_0...Y_{n-1} U_0...U_{n/2-1} V_0...V_{n/2-1}]
    // with pitch: Y = width, U = width/2, V = width/2
    let mut texture = texture_creator
        .create_texture_streaming(PixelFormatEnum::IYUV, width, height)
        .map_err(|err| anyhow!("creating the video texture failed: {err}"))?;
    let mut event_pump = sdl
        .event_pump()
        .map_err(|err| anyhow!("getting the SDL event pump failed: {err}"))?;

    let pitch = width as usize; // IYUV: Y plane pitch = width (bytes per row)
    let started = Instant::now();
    let limit = (seconds > 0).then(|| Duration::from_secs(seconds));
    let mut last_report = Instant::now();
    let mut frames_rendered = 0u64;
    let mut decode_total_ms: f32 = 0.0;
    let mut render_total_ms: f32 = 0.0;

    'outer: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'outer,
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'outer,
                _ => {}
            }
        }

        if let Some(limit) = limit {
            if started.elapsed() >= limit {
                break;
            }
        }

        // Always render the newest frame; anything older is already gone.
        match sink.take_latest() {
            Some(frame) => {
                let render_started = Instant::now();
                texture
                    .update(
                        None,
                        &frame.data,
                        pitch, // IYUV pitch = width in bytes per row (usize)
                    )
                    .map_err(|err| anyhow!("uploading a frame failed: {err}"))?;
                canvas
                    .copy(&texture, None, None)
                    .map_err(|err| anyhow!("drawing a frame failed: {err}"))?;
                canvas.present();

                let render_elapsed = render_started.elapsed().as_secs_f32() * 1000.0;
                let decode_ms = frame.decode_ms;
                let render_ms = render_elapsed;

                decode_total_ms += decode_ms;
                render_total_ms += render_ms;
                stats.record_decode(decode_ms);
                stats.record_render(render_ms);
                stats.frames_rendered.fetch_add(1, Ordering::Relaxed);
                frames_rendered += 1;
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            report(&stats, decode_total_ms, render_total_ms, frames_rendered);
            last_report = Instant::now();
        }
    }

    running.store(false, Ordering::Relaxed);
    let _ = net_thread.join();
    println!("client stopped after {frames_rendered} rendered frames");
    Ok(())
}

fn report(
    stats: &Stats,
    decode_total_ms: f32,
    render_total_ms: f32,
    frames_rendered: u64,
) {
    let packets = take_counter(&stats.packets);
    let bytes = take_counter(&stats.bytes);
    let received = take_counter(&stats.frames_received);
    let dropped = take_counter(&stats.frames_dropped);
    let replaced = take_counter(&stats.frames_replaced);
    let requests = take_counter(&stats.keyframe_requests);
    let rendered = take_counter(&stats.frames_rendered);

    let (assembly_avg, assembly_max) = stats.take_assembly();
    let (decode_avg, decode_max) = stats.take_decode();
    let (render_avg, render_max) = stats.take_render();

    println!(
        "recv {received:>3}/s {packets:>4} pkts {:>6.1} Mbit/s | shown {rendered:>3}/s | assembly {assembly_avg:>4.1}/{assembly_max:<4.1} ms | decode {decode_avg:>4.1}/{decode_max:<5.1} ms | render {render_avg:>4.1}/{render_max:<4.1} ms | dropped {dropped} replaced {replaced} kf-req {requests}",
        bytes as f64 * 8.0 / 1_000_000.0,
    );
    // Per-frame averages since last report
    if frames_rendered > 0 {
        let avg_decode = decode_total_ms / frames_rendered as f32;
        let avg_render = render_total_ms / frames_rendered as f32;
        println!(
            "  per-frame: decode {avg_decode:.1} ms | render {avg_render:.1} ms",
        );
    }
}

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once('x')
        .with_context(|| format!("size must look like 1280x720, got {text:?}"))?;
    Ok((
        width.trim().parse().context("invalid width in --size")?,
        height.trim().parse().context("invalid height in --size")?,
    ))
}