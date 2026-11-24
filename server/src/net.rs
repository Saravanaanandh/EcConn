//! UDP sender: splits encoded frames into datagrams and sends them, and
//! listens for keyframe requests coming back from the client on the same port.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use common::{
    display_config_encode, input_msg_decode, packet_count, set_orientation_decode,
    FrameHeader, InputMsg, DATAGRAM_SIZE, FLAG_KEYFRAME, HEADER_LEN, MAX_PAYLOAD,
};

use crate::encoder::EncodedFrame;
use crate::input::{InputInjector, MonitorRect};

pub struct NetSender {
    socket: UdpSocket,
    dest: SocketAddr,
    start: Instant,
}

impl NetSender {
    pub fn new(bind: SocketAddr, dest: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind(bind)
            .with_context(|| format!("binding UDP socket to {bind} failed"))?;
        // A short read timeout keeps the socket blocking for sends (so a full
        // send buffer does not silently drop frames) while still letting us
        // poll for keyframe requests.
        socket.set_read_timeout(Some(Duration::from_millis(1)))?;
        Ok(Self {
            socket,
            dest,
            start: Instant::now(),
        })
    }

    pub fn spawn(
        self,
        frames: Receiver<EncodedFrame>,
        force_keyframe: Arc<AtomicBool>,
        pending_orientation: Arc<AtomicU32>,
        config_rx: Receiver<(u32, u32, u16, u8, MonitorRect)>,
        rect: MonitorRect,
        orientation: u16,
        rotation: u8,
        frame_width: u32,
        frame_height: u32,
    ) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("udp-sender".into())
            .spawn(move || self.run(frames, force_keyframe, pending_orientation, config_rx, rect, orientation, rotation, frame_width, frame_height))
            .expect("spawning the udp sender thread failed")
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        self,
        frames: Receiver<EncodedFrame>,
        force_keyframe: Arc<AtomicBool>,
        pending_orientation: Arc<AtomicU32>,
        config_rx: Receiver<(u32, u32, u16, u8, MonitorRect)>,
        rect: MonitorRect,
        orientation: u16,
        rotation: u8,
        frame_width: u32,
        frame_height: u32,
    ) {
        let mut datagram = [0u8; DATAGRAM_SIZE];
        let mut control = [0u8; 64];
        let mut frame_id: u32 = 0;
        let mut sent_frames = 0u64;
        let mut sent_packets = 0u64;
        let mut sent_bytes = 0u64;
        let mut keyframe_requests = 0u64;
        let mut last_report = Instant::now();
        let mut input = InputInjector::new(rect);

        // DisplayConfig describes the encoded frame (what the decoder must
        // output); `rect` is only the monitor rectangle used for input.
        let mut current_config = (frame_width, frame_height, orientation, rotation);
        let mut last_config_sent = Instant::now();

        // Helper closure to send DisplayConfig packet to dest
        let send_config = |socket: &UdpSocket, dest: SocketAddr, w: u32, h: u32, orient: u16, rot: u8, start: Instant| {
            let mut buf = [0u8; HEADER_LEN + 24];
            let hdr = FrameHeader {
                flags: 0,
                frame_id: 0,
                packet_index: 0,
                packet_count: 1,
                timestamp_us: start.elapsed().as_micros() as u64,
                payload_len: 24,
            };
            hdr.encode(&mut buf[..HEADER_LEN]);
            display_config_encode(w, h, orient, rot, 0, 0x4C4C5332, &mut buf[HEADER_LEN..]);
            let _ = socket.send_to(&buf, dest);
        };

        // Send initial DisplayConfig packet
        send_config(&self.socket, self.dest, current_config.0, current_config.1, current_config.2, current_config.3, self.start);

        loop {
            // Check for display configuration updates from the main thread
            while let Ok((w, h, orient, rot, new_rect)) = config_rx.try_recv() {
                current_config = (w, h, orient, rot);
                input.set_rect(new_rect);
                // Send twice for UDP redundancy
                send_config(&self.socket, self.dest, w, h, orient, rot, self.start);
                send_config(&self.socket, self.dest, w, h, orient, rot, self.start);
                last_config_sent = Instant::now();
            }

            // Drain control datagrams. A timeout/would-block error just means
            // nothing is waiting for us.
            loop {
                match self.socket.recv_from(&mut control) {
                    Ok((len, from)) => {
                        if let Some(header) = FrameHeader::decode(&control[..len]) {
                            if header.is_keyframe_request() {
                                keyframe_requests += 1;
                                force_keyframe.store(true, Ordering::Relaxed);
                                println!("keyframe request from {from}");
                                // Also reply with DisplayConfig so client stays in sync
                                send_config(&self.socket, from, current_config.0, current_config.1, current_config.2, current_config.3, self.start);
                            } else if header.flags == 0 && header.payload_len >= 14 {
                                // Input or control datagram
                                let end = HEADER_LEN + header.payload_len as usize;
                                if len >= end {
                                    let payload = &control[HEADER_LEN..end];
                                    if let Some((msg, _seq, _token)) = input_msg_decode(payload) {
                                        if msg == InputMsg::SetOrientation {
                                            if let Some(target_deg) = set_orientation_decode(payload) {
                                                println!("client requested orientation: {target_deg}°");
                                                pending_orientation.store(target_deg as u32, Ordering::SeqCst);
                                                force_keyframe.store(true, Ordering::Relaxed);
                                            }
                                        } else {
                                            input.process(payload);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            // Release held keys/buttons if the client went quiet.
            input.tick();

            let frame = match frames.recv_timeout(Duration::from_millis(5)) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };

            let count = packet_count(frame.data.len());
            let timestamp_us = self.start.elapsed().as_micros() as u64;
            let flags = if frame.keyframe { FLAG_KEYFRAME } else { 0 };

            for index in 0..count {
                let offset = index as usize * MAX_PAYLOAD;
                let len = (frame.data.len() - offset).min(MAX_PAYLOAD);
                let header = FrameHeader {
                    flags,
                    frame_id,
                    packet_index: index,
                    packet_count: count,
                    timestamp_us,
                    payload_len: len as u16,
                };
                header.encode(&mut datagram[..HEADER_LEN]);
                datagram[HEADER_LEN..HEADER_LEN + len]
                    .copy_from_slice(&frame.data[offset..offset + len]);

                match self.socket.send_to(&datagram[..HEADER_LEN + len], self.dest) {
                    Ok(_) => {
                        sent_packets += 1;
                        sent_bytes += (HEADER_LEN + len) as u64;
                    }
                    Err(err) => eprintln!("send_to {} failed: {err}", self.dest),
                }
            }

            sent_frames += 1;
            frame_id = frame_id.wrapping_add(1);

            // Resend the display config once a second. The startup packet can
            // race the client binding its socket; a client that missed it
            // would otherwise decode at the wrong size until it next asks for
            // a keyframe.
            if last_config_sent.elapsed() >= Duration::from_secs(1) {
                send_config(
                    &self.socket,
                    self.dest,
                    current_config.0,
                    current_config.1,
                    current_config.2,
                    current_config.3,
                    self.start,
                );
                last_config_sent = Instant::now();
            }

            if last_report.elapsed() >= Duration::from_secs(1) {
                let seconds = last_report.elapsed().as_secs_f64();
                let (in_recv, in_inj, in_rej) = input.take_counters();
                println!(
                    "sent {:>4} frames, {:>5} packets, {:>7.1} kbit/s, last {} packets, keyframe_requests {} | input received {}/s injected {}/s rejected {}",
                    sent_frames,
                    sent_packets,
                    sent_bytes as f64 * 8.0 / 1000.0 / seconds,
                    count,
                    keyframe_requests,
                    in_recv,
                    in_inj,
                    in_rej
                );
                last_report = Instant::now();
                sent_bytes = 0;
            }
        }

        // Never leave keys or buttons stuck down when the stream ends.
        input.release_all();
        println!(
            "udp sender stopped: {sent_frames} frames, {sent_packets} packets, {keyframe_requests} keyframe requests"
        );
    }
}
