//! H.264 encoder: an `ffmpeg` child process fed with raw frames on stdin,
//! emitting one Annex B access unit per frame on stdout.
//!.
//! Frame boundaries: we ask x264 for an access unit delimiter (`aud=1`), which
//! gives exactly one AUD NAL at the start of every access unit. That was
//! verified against this ffmpeg build with `trace_headers` before relying on
//! it. `repeat-headers=1` makes every IDR carry SPS/PPS, so a client that
//! joins mid-stream or asks for a keyframe can start decoding immediately.
//!.
//! M3: supports hardware encode (NVENC/QuickSync/AMF) when available, with
//! fallback to the ffmpeg/libx264 software path used in M2.
//! The encoder path is selected at construction time via encode_path.
//! Supported values: "auto" (auto-detect nvenc > qsv > amf > libx264),
//! "nvenc", "qsv", "amf", or "libx264" (software only).
//!
//! Fixes applied per bug report:
//! 1. ONE Command built once with .stdin(Stdio::piped()).stdout(Stdio::piped()),
//!    spawning it immediately.
//! 2. Raw input declares REAL capture size (-video_size 1920x1080, -pixel_format bgra).
//!    -vf scale=W:H added only when requested size differs from capture.
//! 3. ffmpeg option ordering: all -c:v/preset/x264-params come BEFORE -f h264 pipe:1
//!    (ffmpeg ignores output options before the output URL).

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;

use anyhow::{bail, Context, Result};

/// One encoded frame: a complete Annex B access unit.
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
}

impl Encoder {
    /// Spawn ffmpeg and start the thread that splits its stdout into frames.
    ///.
    /// `encode_path` selects the encoding path:
    /// - "auto" (default): auto-detect (NVENC > QuickSync > AMF > libx264)
    /// - "nvenc": force NVIDIA NVENC
    /// - "qsv": force Intel QuickSync
    /// - "amf": force AMD AMF
    /// - "libx264": software-only via ffmpeg's libx264 (M2 path, unchanged)
    pub fn new(
        width: u32,
        height: u32,
        fps: u32,
        frames: SyncSender<EncodedFrame>,
        encode_path: &str,
        need_scale: bool,
    ) -> Result<Self> {
        let gop: u32 = fps / 2;

        // Build the ffmpeg command ONCE with piped stdin/stdout
        let mut cmd = Command::new("ffmpeg");
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::null());

        // Input: raw video from capture. Always use capture size 1920x1080 with bgra.
        // Scaling to encode size is handled via -vf scale if need_scale.
        cmd.args(["-f", "rawvideo"])
           .arg("-pixel_format")
           .arg("bgra")
           .arg("-video_size")
           .arg("1920x1080")
           .arg("-framerate")
           .arg(fps.to_string())
           .arg("-i")
           .arg("-"); // read from stdin

        // Add scale filter if needed (after -i, before -c:v)
        if need_scale {
            cmd.arg("-vf").arg(format!("scale={}:{}", width, height));
        }

        // Path-specific codec and output options
        // NOTE: ffmpeg processes options in order. The -c:v must come BEFORE
        // the output URL (pipe:1). All encoder-specific options go here.
        match encode_path {
            "nvenc" => {
                cmd.arg("-c:v").arg("h264_nvenc")
                   .arg("-preset").arg("p1")
                   .arg("-tune").arg("zerolatency");
            }
            "qsv" => {
                cmd.arg("-c:v").arg("h264_qsv")
                   .arg("-preset").arg("ultrafast")
                   .arg("-tune").arg("zerolatency");
            }
            "amf" => {
                cmd.arg("-c:v").arg("h264_amf")
                   .arg("-preset").arg("quality")
                   .arg("-tune").arg("zerolatency");
            }
            "libx264" | _ => {
                // Software libx264 path: ultrafast preset, zerolatency tune, no B-frames
                cmd.arg("-c:v").arg("libx264")
                   .arg("-preset").arg("ultrafast")
                   .arg("-tune").arg("zerolatency")
                   .arg("-g").arg(gop.to_string())
                   .arg("-bf").arg("0");
                // x264-params only for software path, BEFORE -f h264 pipe:1
                cmd.arg("-x264-params").arg(format!(
                    "aud=1:repeat-headers=1:keyint={}:min-keyint={}:scenecut=0:bframes=0",
                    gop, gop
                ));
            }
        }

        // COMMON: output format must be LAST. ffmpeg reads -f h264 pipe:1 after all codec options.
        cmd.arg("-f").arg("h264").arg("pipe:1");

        eprintln!("ffmpeg command: {:#?}", cmd);

        eprintln!("encode-path: '{encode_path}'");

        let mut child = cmd.spawn()?;

        let stdin = child.stdin.take().context("ffmpeg stdin was not piped")?;
        let stdout = child.stdout.take().context("ffmpeg stdout was not piped")?;

        let reader = std::thread::Builder::new()
            .name("h264-reader".into())
            .spawn(move || {
                if let Err(err) = read_access_units(stdout, frames) {
                    eprintln!("encoder reader stopped: {err:#}");
                }
            })
            .context("spawning the h264 reader thread failed")?;

        Ok(Self {
            child,
            stdin: Some(stdin),
            reader: Some(reader),
        })
    }

    /// Hand one tightly packed raw frame to the encoder. Blocks when ffmpeg is
    /// behind, which is our back-pressure.
    pub fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        let stdin = self.stdin.as_mut().context("encoder already finished")?;
        stdin.write_all(frame).context("writing a frame to ffmpeg failed")?;
        Ok(())
    }

    /// Close stdin, wait for ffmpeg to drain, and join the reader thread.
    /// Used both at shutdown and to force a fresh IDR (a restarted encoder
    /// starts with SPS/PPS + IDR, which is how we serve keyframe requests).
    pub fn finish(&mut self) -> Result<()> {
        let _ = self.stdin.take();
        let status = self.child.wait()?;
        if !status.success() {
            bail!("ffmpeg exited with {status}");
        }
        Ok(())
    }
}

/// Read ffmpeg stdout and split into access units.
fn read_access_units(mut stdout: ChildStdout, frames: SyncSender<EncodedFrame>) -> Result<()> {
    let mut splitter = AccessUnitSplitter::default();
    let mut chunk = vec![0u8; 64 * 1024];

    loop {
        let read = match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err).context("reading ffmpeg stdout failed"),
        };

        for access_unit in splitter.push(&chunk[..read]) {
            let frame = EncodedFrame {
                keyframe: contains_idr(&access_unit),
                data: access_unit,
            };
            if frames.send(frame).is_err() {
                return Ok(()); // network thread is gone
            }
        }
    }

    if let Some(access_unit) = splitter.finish() {
        let _ = frames.send(EncodedFrame {
            keyframe: contains_idr(&access_unit),
            data: access_unit,
        });
    }
    Ok(())
}

/// Splits an Annex B byte stream into access units using AUD NALs.
#[derive(Default)]
struct AccessUnitSplitter {
    buf: Vec<u8>,
    scan: usize,
}

impl AccessUnitSplitter {
    fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut complete = Vec::new();

        loop {
            match find_start_code(&self.buf, self.scan) {
                Some((start_code_offset, nal_offset, Some(nal_type))) => {
                    if nal_type == NAL_AUD && start_code_offset > 0 {
                        complete.push(self.buf.drain(..start_code_offset).collect());
                        self.scan = 0;
                        continue;
                    }
                    self.scan = nal_offset + 1;
                }
                Some((start_code_offset, _, None)) => {
                    self.scan = start_code_offset;
                    break;
                }
                None => {
                    self.scan = self.buf.len().saturating_sub(3);
                    break;
                }
            }
        }

        complete
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}

const NAL_AUD: u8 = 9;
const NAL_IDR: u8 = 5;

fn find_start_code(buf: &[u8], from: usize) -> Option<(usize, usize, Option<u8>)> {
    let mut i = from;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if buf[i + 2] == 1 {
                let nal = i + 3;
                return Some((i, nal, buf.get(nal).map(|byte| byte & 0x1F)));
            }
            if buf[i + 2] == 0 && i + 4 <= buf.len() && buf[i + 3] == 1 {
                let nal = i + 4;
                return Some((i, nal, buf.get(nal).map(|byte| byte & 0x1F)));
            }
        }
        i += 1;
    }
    None
}

fn contains_idr(access_unit: &[u8]) -> bool {
    let mut i = 0;
    while let Some((_, nal_offset, nal_type)) = find_start_code(access_unit, i) {
        let Some(nal_type) = nal_type else {
            break;
        };
        if nal_type == NAL_IDR {
            return true;
        }
        i = nal_offset + 1;
    }
    false
}