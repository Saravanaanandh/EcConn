//! UDP receive, frame reassembly, keyframe requests, and input sending.
//!
//! Video arrives on the bind port (default `0.0.0.0:9100`). Input events go
//! the other way: `InputSender` owns a clone of the same socket and sends
//! `common::InputMsg` datagrams to the server's control port (default 9000),
//! so the main thread can forward input while the receiver thread runs.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use common::{input_msg_encode, FrameHeader, InputMsg, FLAG_KEYFRAME_REQUEST, HEADER_LEN, MAX_PAYLOAD};

use crate::decoder::Decoder;
use crate::stats::Stats;

/// How many partially received frames we are willing to track at once.
const MAX_PARTIAL_FRAMES: usize = 4;
/// A frame missing packets for this long is written off. Measured from the
/// *last* packet, not the first: a 1080p keyframe is ~150 datagrams and on a
/// busy machine they can take a while to arrive.
const STALE_AFTER: Duration = Duration::from_millis(200);
/// Never ask for a keyframe more often than this. A request restarts the
/// server's encoder, which costs it a few hundred milliseconds, so asking
/// eagerly turns packet loss into a stall.
const REQUEST_INTERVAL: Duration = Duration::from_millis(500);
/// Sanity bound on a header's packet count (about 5.6 MB per frame). Without
/// it a corrupt datagram could ask us to allocate gigabytes.
const MAX_PACKETS_PER_FRAME: u16 = 4096;

/// Payload size of an input datagram: 13 common bytes + 4 data bytes.
/// Every message uses the full 17 bytes (unused data bytes stay zero), so
/// the receiver can always read the type-specific fields at bytes 13..17.
const INPUT_PAYLOAD_LEN: usize = 17;

struct Partial {
    data: Vec<u8>,
    sizes: Vec<u16>,
    received: Vec<bool>,
    got: u16,
    count: u16,
    keyframe: bool,
    last_at: Instant,
}

impl Partial {
    fn new(header: &FrameHeader) -> Self {
        let count = header.packet_count as usize;
        Self {
            data: vec![0u8; count * MAX_PAYLOAD],
            sizes: vec![0u16; count],
            received: vec![false; count],
            got: 0,
            count: header.packet_count,
            keyframe: header.is_keyframe(),
            last_at: Instant::now(),
        }
    }

    fn assemble(&self) -> Vec<u8> {
        let total: usize = self.sizes.iter().map(|size| *size as usize).sum();
        let mut out = Vec::with_capacity(total);
        for index in 0..self.count as usize {
            let start = index * MAX_PAYLOAD;
            out.extend_from_slice(&self.data[start..start + self.sizes[index] as usize]);
        }
        out
    }
}

pub struct Receiver {
    socket: UdpSocket,
    server: SocketAddr,
    width: u32,
    height: u32,
    config_tx: std::sync::mpsc::SyncSender<(u32, u32, u16, u8)>,
    partial: HashMap<u32, Partial>,
    /// Highest frame id we have finished with (submitted or deliberately
    /// dropped). Anything at or below it is too late to be useful.
    newest_done: Option<u32>,
    /// Until we have seen a keyframe we cannot decode anything, because a
    /// mid-GOP P-frame needs the frames before it.
    awaiting_keyframe: bool,
    last_request: Option<Instant>,
    stats: Arc<Stats>,
    running: Arc<AtomicBool>,
}

impl Receiver {
    pub fn new(
        bind: SocketAddr,
        server: SocketAddr,
        width: u32,
        height: u32,
        config_tx: std::sync::mpsc::SyncSender<(u32, u32, u16, u8)>,
        stats: Arc<Stats>,
        running: Arc<AtomicBool>,
    ) -> Result<Self> {
        // A default 64 KB receive buffer is sufficient for our use case.
        // (A 1080p IDR is ~200 KB, well within typical OS defaults.)
        let socket = UdpSocket::bind(bind)
            .with_context(|| format!("binding UDP socket to {bind} failed"))?;
        // A short read timeout lets the receive loop notice `running == false`
        // promptly on shutdown instead of blocking forever in recv_from.
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;

        Ok(Self {
            socket,
            server,
            width,
            height,
            config_tx,
            partial: HashMap::new(),
            newest_done: None,
            awaiting_keyframe: true,
            last_request: None,
            stats,
            running,
        })
    }

    /// Create the input sender before `spawn` consumes the receiver. It uses
    /// a clone of the same socket, so input and video share one local port.
    pub fn input_sender(&self) -> Result<InputSender> {
        self.input_sender_inner(None)
    }

    /// Use the token agreed during pairing. A zero token falls back to the
    /// random per-run token used by [`Receiver::input_sender`].
    pub fn input_sender_with_token(&self, token: u64) -> Result<InputSender> {
        self.input_sender_inner(Some(token))
    }

    fn input_sender_inner(&self, forced_token: Option<u64>) -> Result<InputSender> {
        let socket = self
            .socket
            .try_clone()
            .context("cloning the UDP socket for input failed")?;
        let token = match forced_token {
            Some(token) if token != 0 => token,
            _ => {
                // A non-zero session token unique to this client run. The
                // server accepts the first token it sees unless paired.
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                (nanos ^ ((std::process::id() as u64) << 32)) | 1
            }
        };
        Ok(InputSender {
            socket,
            server: self.server,
            seq_id: 1,
            token,
        })
    }

    pub fn spawn(self, decoder: Decoder) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("udp-receiver".into())
            .spawn(move || {
                let mut decoder = decoder;
                self.run(&mut decoder);
                decoder.shutdown();
            })
            .expect("spawning the udp receiver thread failed")
    }

    fn run(mut self, decoder: &mut Decoder) {
        let mut datagram = [0u8; 2048];
        // We join mid-GOP at best: ask for a keyframe right away.
        self.request_keyframe();
        let mut last_packet_at: Option<Instant> = None;

        while self.running.load(Ordering::Relaxed) {
            match self.socket.recv_from(&mut datagram) {
                Ok((len, _from)) => {
                    if &datagram[..len] == b"ECCONN_BYE" {
                        self.running.store(false, Ordering::Relaxed);
                        break;
                    }
                    last_packet_at = Some(Instant::now());
                    self.handle_datagram(&datagram[..len], decoder);
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            // On Windows a UDP socket reports an earlier ICMP
                            // port-unreachable (e.g. a keyframe request sent
                            // before the server was up) as a receive error.
                            // That is not fatal for a stream socket.
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    if let Some(at) = last_packet_at {
                        if at.elapsed() >= Duration::from_secs(3) {
                            self.running.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                Err(err) => {
                    eprintln!("receive failed: {err}");
                    break;
                }
            }
            self.drop_stale();
        }
    }

    fn handle_datagram(&mut self, datagram: &[u8], decoder: &mut Decoder) {
        let Some(header) = FrameHeader::decode(datagram) else {
            return;
        };
        if header.is_keyframe_request() {
            return;
        }

        // Check for DisplayConfig control datagram from server
        if header.flags == 0 && header.frame_id == 0 && header.packet_count == 1 && header.payload_len >= 24 {
            let payload = &datagram[HEADER_LEN..HEADER_LEN + header.payload_len as usize];
            if let Some((msg, _, _)) = common::input_msg_decode(payload) {
                if msg == common::InputMsg::DisplayConfig {
                    if let Some((w, h, orient, rot)) = common::display_config_decode(payload) {
                        self.newest_done = None;
                        if w != self.width || h != self.height {
                            println!("client: server display config changed to {w}x{h} (orientation {orient}°, rotation {rot})");
                            if let Err(err) = decoder.reset(w, h) {
                                eprintln!("resetting decoder failed: {err:#}");
                            }
                            self.width = w;
                            self.height = h;
                            self.partial.clear();
                            self.awaiting_keyframe = true;
                            self.request_keyframe();
                        }
                        // Always inform the render thread: a 180° rotation
                        // keeps the same size but updates the orientation
                        // state used by the R key.
                        let _ = self.config_tx.send((w, h, orient, rot));
                        return;
                    }
                }
            }
        }
        if header.packet_count == 0
            || header.packet_count > MAX_PACKETS_PER_FRAME
            || header.packet_index >= header.packet_count
        {
            return;
        }

        let payload_len = header.payload_len as usize;
        if payload_len > MAX_PAYLOAD || datagram.len() < HEADER_LEN + payload_len {
            return;
        }
        let payload = &datagram[HEADER_LEN..HEADER_LEN + payload_len];

        self.stats.packets.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes
            .fetch_add(datagram.len() as u64, Ordering::Relaxed);

        // A frame we already finished with cannot be used any more.
        if let Some(newest) = self.newest_done {
            if header.frame_id <= newest {
                return;
            }
        }

        if !self.partial.contains_key(&header.frame_id)
            && self.partial.len() >= MAX_PARTIAL_FRAMES
        {
            self.evict_oldest();
        }

        let partial = self
            .partial
            .entry(header.frame_id)
            .or_insert_with(|| Partial::new(&header));
        let index = header.packet_index as usize;
        partial.last_at = Instant::now();
        if partial.received[index] {
            return;
        }
        partial.received[index] = true;
        partial.sizes[index] = payload_len as u16;
        partial.data[index * MAX_PAYLOAD..index * MAX_PAYLOAD + payload_len]
            .copy_from_slice(payload);
        partial.got += 1;

        if partial.got < partial.count {
            return;
        }

        let partial = self
            .partial
            .remove(&header.frame_id)
            .expect("frame was just completed");
        let data = partial.assemble();
        self.newest_done = Some(header.frame_id);
        self.stats.frames_received.fetch_add(1, Ordering::Relaxed);
        self.stats
            .record_assembly(partial.last_at.elapsed().as_secs_f32() * 1000.0);

        // Decoding a frame whose references are missing produces garbage, so
        // wait for the next keyframe instead.
        if self.awaiting_keyframe && !partial.keyframe {
            self.stats.frames_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.awaiting_keyframe = false;

        if decoder.submit(data) {
            // The decoder still had an older frame queued and threw it away,
            // so that frame will never be decoded and everything after it
            // references a hole. Reset so the next frame starts fresh.
            self.awaiting_keyframe = true;
        }
    }

    /// Give up on frames that will never complete (lost packets).
    fn drop_stale(&mut self) {
        let stale: Vec<u32> = self
            .partial
            .iter()
            .filter(|(_, partial)| partial.last_at.elapsed() > STALE_AFTER)
            .map(|(frame_id, _)| *frame_id)
            .collect();
        if stale.is_empty() {
            return;
        }
        let mut lost_current = false;
        for frame_id in &stale {
            self.partial.remove(frame_id);
            if self.newest_done.is_none_or(|newest| *frame_id > newest) {
                lost_current = true;
            }
        }
        self.stats
            .frames_dropped
            .fetch_add(stale.len() as u64, Ordering::Relaxed);
        if lost_current {
            self.awaiting_keyframe = true;
            self.request_keyframe();
        }
    }

    fn evict_oldest(&mut self) {
        let Some(oldest) = self.partial.keys().min().copied() else {
            return;
        };
        self.partial.remove(&oldest);
        self.stats.frames_dropped.fetch_add(1, Ordering::Relaxed);
        self.awaiting_keyframe = true;
        self.request_keyframe();
    }

    fn request_keyframe(&mut self) {
        if let Some(last) = self.last_request {
            if last.elapsed() < REQUEST_INTERVAL {
                return;
            }
        }
        let header = FrameHeader {
            flags: FLAG_KEYFRAME_REQUEST,
            frame_id: 0,
            packet_index: 0,
            packet_count: 0,
            timestamp_us: 0,
            payload_len: 0,
        };
        let mut datagram = [0u8; HEADER_LEN];
        header.encode(&mut datagram);
        match self.socket.send_to(&datagram, self.server) {
            Ok(_) => {
                self.stats.keyframe_requests.fetch_add(1, Ordering::Relaxed);
                self.last_request = Some(Instant::now());
            }
            Err(err) => eprintln!("sending a keyframe request failed: {err}"),
        }
    }
}

/// Sends input events to the server over the UDP control channel.
///
/// Each datagram reuses the 24-byte video frame header (magic `0x4C4C5332`,
/// version 1, flags 0) followed by a 17-byte input payload: 13 common bytes
/// (`msg_type`, reserved, `seq_id`, `session_token`) plus 4 type-specific
/// data bytes at offset 13. The server validates token and sequence and
/// injects the events on the captured monitor.
pub struct InputSender {
    socket: UdpSocket,
    server: SocketAddr,
    seq_id: u16,
    token: u64,
}

impl InputSender {
    fn send(&mut self, msg: InputMsg, data: &[u8]) -> Result<()> {
        debug_assert!(data.len() <= 4);
        let mut buf = [0u8; HEADER_LEN + INPUT_PAYLOAD_LEN];
        let header = FrameHeader {
            flags: 0,
            frame_id: 0,
            packet_index: 0,
            packet_count: 1,
            timestamp_us: 0,
            payload_len: INPUT_PAYLOAD_LEN as u16,
        };
        header.encode(&mut buf[..HEADER_LEN]);
        let payload = &mut buf[HEADER_LEN..];
        input_msg_encode(msg, self.seq_id, self.token, payload);
        payload[13..13 + data.len()].copy_from_slice(data);
        self.socket.send_to(&buf, self.server)?;
        self.seq_id = self.seq_id.wrapping_add(1);
        Ok(())
    }

    /// Mouse position normalized to 0..65535 over the video window.
    pub fn mouse_move(&mut self, x: u16, y: u16) -> Result<()> {
        let mut data = [0u8; 4];
        data[..2].copy_from_slice(&x.to_be_bytes());
        data[2..].copy_from_slice(&y.to_be_bytes());
        self.send(InputMsg::MouseMove, &data)
    }

    /// Full button state: bit 0=left, 1=right, 2=middle.
    pub fn mouse_buttons(&mut self, mask: u8) -> Result<()> {
        self.send(InputMsg::MouseBtn, &[mask])
    }

    /// Wheel notches, positive = up/away.
    pub fn wheel(&mut self, delta: i16) -> Result<()> {
        self.send(InputMsg::MouseWheel, &delta.to_be_bytes())
    }

    /// SDL scancode (must fit in 7 bits); `up` sets bit 7 of the payload.
    pub fn key(&mut self, scancode: u8, up: bool) -> Result<()> {
        self.send(InputMsg::Key, &[scancode | if up { 0x80 } else { 0 }])
    }

    /// Request monitor orientation change from the server (0, 90, 180, 270 degrees).
    pub fn set_orientation(&mut self, orientation: u16) -> Result<()> {
        let mut data = [0u8; 4];
        data[..2].copy_from_slice(&orientation.to_be_bytes());
        self.send(InputMsg::SetOrientation, &data)
    }

    /// Release every held key and button on the server.
    pub fn release_all(&mut self) -> Result<()> {
        self.send(InputMsg::ReleaseAll, &[])
    }
}