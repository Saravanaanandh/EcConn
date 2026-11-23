//! M2 client: receive H.264 over UDP, decode with ffmpeg, show in an SDL window.

mod decoder;
mod net;
mod stats;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use common::{DEFAULT_CLIENT_PORT, DEFAULT_SERVER_PORT};
use common::InputMsg as CommonInputMsg;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::mouse::MouseButton;
use sdl2::keyboard::Scancode;

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
    let mut fullscreen = args.iter().any(|arg| arg == "--fullscreen");
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
    let mut receiver = Receiver::new(bind, server, stats.clone(), running.clone())?;

    let net_thread = receiver.spawn(decoder);

    let mut builder = video.window("lowlat-screen", window_width, window_height);
    builder.resizable();
    if fullscreen {
        builder.fullscreen_desktop();
    }
    let mut window = builder
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

    // Input forwarding: last time we sent a mouse move, to coalesce.
    let mut last_mouse_move = Instant::now();

    'outer: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => {
                    // Send release all and quit.
                    receiver.send_input(common::InputMsg::ReleaseAll)?;
                    running.store(false, Ordering::Relaxed);
                    break 'outer;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    receiver.send_input(common::InputMsg::ReleaseAll)?;
                    running.store(false, Ordering::Relaxed);
                    break 'outer;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::F11),
                    ..
                } => {
                    // Toggle fullscreen state on F11 press
                    fullscreen ^= true;
                    eprintln!("F11 pressed - fullscreen {}", if fullscreen { "enabled" } else { "disabled" });
                    if fullscreen {
                        window.set_fullscreen(sdl2::video::FullscreenType::Desktop);
                    }
                }
                _ => {}
            }
        }

        // --- Capture SDL mouse events and forward them ---
        // Coalesce mouse moves: only send at most every MOUSE_MOVE_MAX_INTERVAL_MS.
        let mouse_state = event_pump.mouse_state();
        if last_mouse_move.elapsed().as_millis() >= 8 {
            // Normalize mouse coordinates to u16 0..65535 relative to the window.
            let mx = mouse_state.x();
            let my = mouse_state.y();
            let norm_x = ((mx as f64 / window_width as f64) * 65535.0) as u16;
            let norm_y = ((my as f64 / window_height as f64) * 65535.0) as u16;

            // Send mouse move. We don't check if the mouse is "inside the video
            // area" separately because the window IS the video area.
            if let Err(e) = receiver.send_input(common::InputMsg::MouseMove) {
                eprintln!("failed to send mouse move: {e}");
            }
            last_mouse_move = Instant::now();
        }

        // --- Mouse button state ---
        // Check which buttons are pressed.
        let left_pressed = mouse_state.left();
        let right_pressed = mouse_state.right();
        let middle_pressed = mouse_state.middle();
        let btn_mask = (if left_pressed { 0x01 } else { 0 })
            | (if right_pressed { 0x02 } else { 0 })
            | (if middle_pressed { 0x04 } else { 0 });

        // Only send if button state changed (we could track prev mask, but for
        // simplicity send on every poll iteration when the interval has passed
        // - the server coalesces anyway).
        if last_mouse_move.elapsed().as_millis() >= 8 {
            // Reuse the same interval to send button state; alternatively we could
            // track previous state, but keeping it simple.
            if let Err(e) = receiver.send_input(common::InputMsg::MouseBtn) {
                eprintln!("failed to send mouse button: {e}");
            }
            last_mouse_move = Instant::now();
        }

        // --- Mouse wheel ---
        // Track mouse wheel delta via events polled in the event loop.
        let mut wheel_delta: i16 = 0;
        for event in event_pump.poll_iter() {
            if let sdl2::event::Event::MouseWheel { y, .. } = event {
                wheel_delta = y as i16;
                break;
            }
        }
        if wheel_delta != 0 {
            // Send mouse wheel delta. The payload is i16 big-endian.
            if let Err(e) = receiver.send_input(common::InputMsg::MouseWheel) {
                eprintln!("failed to send mouse wheel: {e}");
            }
        }

        // --- Keyboard state: send currently pressed SDL scancodes ---
        // We'll send a key-down for each key currently held down, but only every
        // so often to avoid flooding. For now, send key events on every poll
        // iteration when the mouse move interval has passed.
        let keyboard_state = event_pump.keyboard_state();
        let mut any_key_sent = false;
        for i in 0..256 {
            if keyboard_state.is_scancode_pressed(unsafe { std::mem::transmute::<u8, Scancode>(i as u8) }) {
                // Send key down (bit 7 clear)
                if let Err(e) = receiver.send_input(common::InputMsg::Key) {
                    eprintln!("failed to send key down: {e}");
                }
                any_key_sent = true;
                // Only send one key per poll iteration to avoid flooding.
                break;
            }
        }
        // Also send "release all" for keys that are no longer pressed compared
        // to the previous state. For simplicity, on each iteration we send a
        // ReleaseAll if no keys are currently down (or we could track previous
        // state). Here we just send ReleaseAll on quit/focus loss (handled above).

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

    // On clean exit, release all keys and mouse buttons.
    let _ = receiver.send_input(common::InputMsg::ReleaseAll);

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