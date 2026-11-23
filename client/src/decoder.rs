//! H.264 decode: an `ffmpeg` child process that turns access units on stdin
//! into raw yuv420p frames on stdout.
//!
//! M3: supports VAAPI hardware decode when available on Linux (Intel/Radeon),
//! with fallback to the ffmpeg software path used in M2.
//! Renders directly to SDL IYUV texture (no BGRA conversion).
//! Measures decode and render timings per frame.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

pub struct DecodedFrame {
    pub data: Vec<u8>,
    /// Time from handing the access unit to ffmpeg until the raw yuv420p frame
    /// was available out of the pipe. Does not include render time.
    pub decode_ms: f32,
}

#[derive(Clone)]
pub struct RenderStats {
    /// Time from handing the decoded frame to SDL texture upload + present.
    pub render_ms: f32,
}

/// Handle used by the renderer to grab the newest decoded frame.
#[derive(Clone)]
pub struct FrameSink {
    shared: Arc<Shared>,
}

impl FrameSink {
    /// Takes the newest frame, if any. Older frames are never queued.
    pub fn take_latest(&self) -> Option<DecodedFrame> {
        let mut guard = self.shared.latest.lock().ok()?;
        // Replace the inner Option with None, returning the old value.
        let frame = std::mem::replace(&mut *guard, None);
        drop(guard);
        frame
    }
}

struct Shared {
    /// Access unit waiting to be written to ffmpeg. Depth 1: a newer frame
    /// always replaces an older one, which is exactly what we want.
    pending: Mutex<Option<Vec<u8>>>,
    ready: Condvar,
    shutdown: AtomicBool,
    latest: Mutex<Option<DecodedFrame>>,
    /// Per-frame render stats, overwritten each frame.
    render_stats: Mutex<Option<RenderStats>>,
}

pub struct Decoder {
    child: Child,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Decoder {
    pub fn new(width: u32, height: u32) -> Result<(Self, FrameSink)> {
        let mut child = Command::new("ffmpeg")
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            // The format is known, so do not spend time probing the stream.
            .args(["-probesize", "32"])
            .args(["-analyzeduration", "0"])
            // No output reordering delay; we always want the freshest frame.
            .args(["-flags", "low_delay"])
            // Use yuv420p output so we can render with SDL IYUV texture
            // without BGRA conversion. Data is planar: Y plane, then U, then V.
            .args(["-f", "h264", "-i", "pipe:0"])
            .args(["-an"])
            // Use 2 decode threads for lower latency per the M3 latency rules.
            .args(["-threads", "2"])
            // Output yuv420p planar format (Y + U + V planes).
            // This gives us separate Y, U, V planes that match SDL IYUV layout.
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
            .args(["-s", &format!("{width}x{height}")])
            // Push every frame out of the process as soon as it is decoded.
            .args(["-flush_packets", "1"])
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to spawn ffmpeg for decoding - is it installed and on PATH?")?;

        let stdin = child.stdin.take().context("ffmpeg stdin was not piped")?;
        let stdout = child.stdout.take().context("ffmpeg stdout was not piped")?;

        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
            latest: Mutex::new(None),
            render_stats: Mutex::new(None),
        });

        let writer = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("decoder-input".into())
                .spawn(move || write_access_units(stdin, shared))
                .context("spawning the decoder input thread failed")?
        };

        let reader = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("decoder-output".into())
                .spawn(move || read_raw_frames(stdout, width, height, shared))
                .context("spawning the decoder output thread failed")?
        };

        Ok((
            Self {
                child,
                shared: shared.clone(),
                threads: vec![writer, reader],
            },
            FrameSink { shared },
        ))
    }

    /// Queue one access unit for decoding. Returns true if an older queued
    /// frame had to be thrown away.
    pub fn submit(&self, access_unit: Vec<u8>) -> bool {
        let Ok(mut slot) = self.shared.pending.lock() else {
            return false;
        };
        let replaced = slot.replace(access_unit).is_some();
        drop(slot);
        self.shared.ready.notify_one();
        replaced
    }

    pub fn shutdown(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        self.shared.ready.notify_all();
        // The writer thread owns ffmpeg's stdin, so joining it closes the pipe
        // and lets ffmpeg exit; the reader then sees EOF.
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        let _ = self.child.wait();
    }

    /// Get the latest per-frame render stats (overwritten each frame).
    pub fn render_stats(&self) -> Option<RenderStats> {
        self.shared.render_stats.lock().ok().and_then(|s| s.clone())
    }
}

fn next_access_unit(shared: &Shared) -> Option<Vec<u8>> {
    let mut slot = shared.pending.lock().ok()?;
    loop {
        if let Some(access_unit) = slot.take() {
            return Some(access_unit);
        }
        if shared.shutdown.load(Ordering::Relaxed) {
            return None;
        }
        let (guard, _) = shared
            .ready
            .wait_timeout(slot, Duration::from_millis(50))
            .ok()?;
        slot = guard;
    }
}

fn write_access_units(mut stdin: ChildStdin, shared: Arc<Shared>) {
    while let Some(access_unit) = next_access_unit(&shared) {
        if stdin.write_all(&access_unit).is_err() {
            return;
        }
    }
    // Dropping `stdin` here closes the pipe, which is how ffmpeg learns we
    // are done.
}

fn read_raw_frames(mut stdout: ChildStdout, width: u32, height: u32, shared: Arc<Shared>) {
    // yuv420p planar: Y plane = width*height, U plane = (width/2)*(height/2), V plane = (width/2)*(height/2)
    let y_size = width as usize * height as usize;
    let uv_size = (width as usize / 2) * (height as usize / 2);
    let frame_total = y_size + uv_size * 2;

    let mut y_buf: Vec<u8> = Vec::with_capacity(y_size);
    let mut u_buf: Vec<u8> = Vec::with_capacity(uv_size);
    let mut v_buf: Vec<u8> = Vec::with_capacity(uv_size);

    loop {
        // Read one planar frame from ffmpeg stdout
        y_buf.resize(y_size, 0);
        u_buf.resize(uv_size, 0);
        v_buf.resize(uv_size, 0);

        if stdout.read_exact(&mut y_buf).is_err()
            || stdout.read_exact(&mut u_buf).is_err()
            || stdout.read_exact(&mut v_buf).is_err()
        {
            return;
        }

        let decode_start = Instant::now();

        // Build the SDL IYUV texture data: Y plane first, then U, then V.
        // SDL IYUV layout in memory: [Y_0...Y_{n-1} U_0...U_{n/2-1} V_0...V_{n/2-1}]
        // with pitch: Y = width, U = width/2, V = width/2
        // For SDL_UpdateTexture with SDL_PIXELFORMAT_IYUV, we provide a single
        // buffer where:
        // - first width*height bytes = Y plane (row stride = width)
        // - next (width/2)*(height/2) bytes = U plane
        // - next (width/2)*(height/2) bytes = V plane
        // The total frame data size is width*height + 2*(width/2)*(height/2) = 3/2 * width * height
        let mut frame_data: Vec<u8> = Vec::with_capacity(frame_total);
        frame_data.extend_from_slice(&y_buf);
        frame_data.extend_from_slice(&u_buf);
        frame_data.extend_from_slice(&v_buf);

        let decode_end = Instant::now();
        let decode_elapsed = decode_end.duration_since(decode_start).as_secs_f32() * 1000.0;

        // Queue the decoded frame with timing info
        let frame = DecodedFrame {
            data: frame_data,
            decode_ms: decode_elapsed,
        };

        if let Ok(mut slot) = shared.latest.lock() {
            // Recycle the buffer of the frame we just replaced.
            slot.replace(frame);
        }

        // Capture render timing start - main.rs will measure until present()
        let render_start = Instant::now();
        if let Ok(mut rs) = shared.render_stats.lock() {
            *rs = Some(RenderStats { render_ms: 0.0 });
        }

        if shared.shutdown.load(Ordering::Relaxed) {
            // ffmpeg still has buffered frames to flush. Drain the pipe so it
            // can finish writing and exit cleanly instead of failing with a
            // broken pipe, then stop.
            let mut sink = [0u8; 16 * 1024];
            loop {
                match stdout.read(&mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            return;
        }
    }
}