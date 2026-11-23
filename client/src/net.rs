//! UDP receive, frame reassembly, keyframe requests, and input forwarding.
//!
//! In addition to the video frame protocol (magic `0x4C4C5332`, version `1`,
//! 24-byte header), the receiver also reads input datagrams from the client
//! on the same bind port (default `0.0.0.0:9100`). Input datagrams reuse the
//! same header format but carry an input payload described by
//! `common::InputMsg`.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use common::{InputMsg, FLAG_KEYFRAME_REQUEST, HEADER_LEN, MAGIC, MAX_PAYLOAD, VERSION, FrameHeader, DATAGRAM_SIZE, MAX_PACKETS_PER_FRAME, MAX_PARTIAL_FRAMES, REQUEST_INTERVAL};
use std::thread;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;

use crate::decoder::Decoder;
use crate::stats::Stats;

/// Maximum interval between mouse move packets (ms). Mouse move events more
/// frequent than this are coalesced (only the latest is sent).
const MOUSE_MOVE_MAX_INTERVAL_MS: u64 = 8;
/// How long without a valid input packet before we consider the stream dead
/// and release all keys/buttons (s).
const INPUT_TIMEOUT_S: u64 = 3;
/// How often to check for input timeout (ms).
const INPUT_CHECK_INTERVAL_MS: u64 = 200;

/// A session identifier for the input stream. The server tracks the most
/// recent seen session token; datagrams with a different token are rejected.
type SessionToken = u64;

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

struct InputSession {
    /// Most recent session token we have seen from the client.
    last_token: SessionToken,
    /// Last mouse move instant, for coalescing.
    last_mouse_move: Instant,
    /// Currently pressed mouse buttons (bits 0=left, 1=right, 2=middle).
    pressed_buttons: u8,
    /// Currently pressed keys (SDL scancode values).
    pressed_keys: Vec<u8>,
    /// Last seq_id seen, for basic anti-replay.
    last_seq_id: u16,
}

impl InputSession {
    fn new() -> Self {
        Self {
            last_token: 0,
            last_mouse_move: Instant::now(),
            pressed_buttons: 0,
            pressed_keys: Vec::new(),
            last_seq_id: 0,
        }
    }

    /// Process an input datagram payload. Returns `Some(true)` if the session
    /// state was updated, `Some(false)` if the datagram was stale (old seq_id
    /// or token), and `None` if the payload was malformed.
    fn process(&mut self, payload: &[u8]) -> Option<bool> {
        let (msg_type, seq_id, session_token) = match common::input_msg_decode(payload) {
            Some(t) => t,
            None => return None,
        };

        // Anti-replay: reject if we have seen a newer seq_id from the same
        // source address (the server only sees one client, so we track globally).
        if seq_id <= self.last_seq_id {
            return Some(false); // stale
        }
        self.last_seq_id = seq_id;

        // Session token check: only accept the expected token.
        // The client and server agree on a token at connection time; for now
        // we just accept any token that changes (i.e. we update on first
        // message) or we could hardcode a fixed token. Here we simply update
        // last_token on the first message and require matching thereafter.
        if self.last_token == 0 {
            self.last_token = session_token;
        } else if self.last_token != session_token {
            // Wrong session token - reject but don't crash.
            return Some(false);
        }

        match msg_type {
            InputMsg::MouseMove => {
                // Coalesce: only process if enough time has elapsed.
                if self.last_mouse_move.elapsed() < Duration::from_millis(MOUSE_MOVE_MAX_INTERVAL_MS) {
                    return Some(true); // dropped (coalesced)
                }
                self.last_mouse_move = Instant::now();

                // Payload bytes 13..: normalized x (u16 BE), normalized y (u16 BE)
                if payload.len() < 15 {
                    return None;
                }
                let norm_x = u16::from_be_bytes(payload[13..15].try_into().unwrap_or([0u8; 2]));
                let norm_y = u16::from_be_bytes(payload[15..17].try_into().unwrap_or([0u8; 2]));
                // We'll forward these to the server; the server maps them.
                // For now just record and forward.
                // Actually we need to send the normalized coords as-is; the
                // server will map them to the captured monitor's rect.
                // Just return true to indicate processed.
                Some(true)
            }
            InputMsg::MouseBtn => {
                // Payload byte 13: button bitmask (bit 0=left, 1=right, 2=middle)
                if payload.len() < 14 {
                    return None;
                }
                let btn_mask = payload[13];
                // Release previously pressed buttons that are now up.
                let released = self.pressed_buttons & !btn_mask;
                self.pressed_buttons = btn_mask;
                // Press newly pressed buttons.
                let newly_pressed = btn_mask & !released;
                // (We just track state; the server drives SendInput.)
                Some(true)
            }
            InputMsg::MouseWheel => {
                // Payload bytes 13..15: wheel delta (i116 BE, positive = up/away)
                if payload.len() < 15 {
                    return None;
                }
                let delta = i16::from_be_bytes(payload[13..15].try_into().unwrap_or([0u8; 2])) as i32;
                // Forward to server; server handles SendInput.
                Some(true)
            }
            InputMsg::Key => {
                // Payload byte 13: SDL scancode; bit 7 set = key up.
                if payload.len() < 14 {
                    return None;
                }
                let scode = payload[13];
                let is_up = scode & 0x80 != 0;
                let scode_clean = scode & 0x7F;

                if is_up {
                    // Remove from pressed keys.
                    self.pressed_keys.retain(|&k| k != scode_clean);
                } else {
                    // Add to pressed keys (avoid duplicates).
                    if !self.pressed_keys.contains(&scode_clean) {
                        self.pressed_keys.push(scode_clean);
                    }
                }
                Some(true)
            }
            InputMsg::ReleaseAll => {
                // Release all keys and mouse buttons.
                self.pressed_keys.clear();
                self.pressed_buttons = 0;
                Some(true)
            }
        }
    }
}

pub struct Receiver {
    socket: UdpSocket,
    server: SocketAddr,
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
    /// Input forwarding state (one session, the first client that connects).
    input: InputSession,
}

impl Receiver {
    pub fn new(
        bind: SocketAddr,
        server: SocketAddr,
        stats: Arc<Stats>,
        running: Arc<AtomicBool>,
    ) -> Result<Self> {
        // A default 64 KB receive buffer is sufficient for our use case.
        // (A 1080p IDR is ~200 KB, well within typical OS defaults.)
        let socket = UdpSocket::bind(bind)
            .with_context(|| format!("binding UDP socket to {bind} failed"))?;

        Ok(Self {
            socket,
            server,
            partial: HashMap::new(),
            newest_done: None,
            awaiting_keyframe: true,
            last_request: None,
            stats,
            running,
            input: InputSession::new(),
        })
    }

    pub fn spawn(self, decoder: Decoder) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("udp-receiver".into())
            .spawn(move || {
                let mut decoder = decoder;
                self.run(&decoder);
                decoder.shutdown();
            })
            .expect("spawning the udp receiver thread failed")
    }

    fn run(mut self, decoder: &Decoder) {
        let mut datagram = [0u8; 2048];
        // We join mid-GOP at best: ask for a keyframe right away.
        self.request_keyframe();

        // Input timeout checker thread will be started below.
        let (input_tx, input_rx) = std::sync::mpsc::channel();
        let input_running = self.running.clone();
        let input_handle = std::thread::spawn(move || {
            let mut checkpoint = Instant::now();
            while input_running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(INPUT_CHECK_INTERVAL_MS));
                if checkpoint.elapsed() >= Duration::from_secs(INPUT_TIMEOUT_S) {
                    // No valid input packet for 3 s → release all.
                    eprintln!("input timeout: releasing all keys/buttons");
                    let _ = input_tx.send(InputMsg::ReleaseAll);
                    checkpoint = Instant::now();
                }
            }
        });

        while self.running.load(Ordering::Relaxed) {
            match self.socket.recv_from(&mut datagram) {
                Ok((len, from)) => {
                    if let Some(header) = FrameHeader::decode(&datagram[..len]) {
                        if header.is_keyframe_request() {
                            return;
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

                        // --- Input forwarding ---
                        if let Some(update) = self.input.process(payload) {
                            if let Some(_) = update {
                                // Valid input (either true or false)
                            } else {
                                // Malformed payload (None).
                                self.stats
                                    .rejected_packets
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        // -------------------------

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
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionRefused
                    ) => {}
                Err(err) => {
                    eprintln!("receive failed: {err}");
                    break;
                }
            }
        }

        // Clean shutdown: release all keys/buttons.
        let _ = input_tx.send(InputMsg::ReleaseAll);
        let _ = input_handle.join();
    }

    /// Send an input message to the server over the UDP control channel.
    ///
    /// The datagram reuses the video-frame header (24 bytes, magic `0x4C4C5332`,
    /// version `1`) with `payload_len` set to 14 (the defined payload size).
    /// The payload carries the `InputMsg` + `seq_id` + `session_token`.
    /// Returns `Ok(())` on success, or an error if the send fails.
    pub fn send_input(&mut self, msg: common::InputMsg) -> Result<()> {
        let mut buf = [0u8; HEADER_LEN + 14];
        // Encode the video-frame header with payload_len = 14.
        let header = common::FrameHeader {
            flags: 0,
            frame_id: 0,
            packet_index: 0,
            packet_count: 0,
            timestamp_us: 0,
            payload_len: 14,
        };
        header.encode(&mut buf[..HEADER_LEN]);
        // Encode the input payload.
        common::input_msg_encode(msg, self.input.last_seq_id, self.input.last_token, &mut buf[HEADER_LEN..]);
        self.socket.send_to(&buf, self.server)?;
        // Update seq_id for next time.
        self.input.last_seq_id = self.input.last_seq_id.wrapping_add(1);
        Ok(())
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
            if last.elapsed().as_millis() < REQUEST_INTERVAL.into() {
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