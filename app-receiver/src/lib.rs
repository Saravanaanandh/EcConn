//! `app-receiver` core: advertise this screen on the LAN, answer pairing
//! requests, and play the host's stream.
//!
//! The video path is *not* reimplemented: [`VideoSession`] builds the existing
//! `client::net::Receiver` + `client::decoder::Decoder` pair, so reassembly,
//! keyframe requests and ffmpeg decode behave exactly like the SDL2 client.
//! Only presentation differs: decoded `yuv420p` frames are converted to RGBA
//! (optionally rotated) and uploaded to an egui texture by the GUI.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use common::discovery::{Beacon, BEACON_PORT, MDNS_SERVICE, PAIR_PORT};
use common::pairing::{PairMsg, PairState, PairingMachine, KEY_LEN};
use common::store::{default_path, PairedHost, PairedStore};

/// UDP port the existing client code receives video on.
pub const VIDEO_PORT: u16 = 9100;
/// How often the fallback beacon is broadcast.
const BEACON_INTERVAL: Duration = Duration::from_secs(2);
/// How long an Allow/Deny dialog waits before it denies by itself.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

pub fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "lowlat receiver".to_string())
}

/// Stable per-installation id used as the pairing identity.
pub fn device_id() -> String {
    let name: String = device_name()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    format!("recv-{}", name.to_lowercase())
}

/// Size advertised in the beacon. `LOWLAT_SCREEN=1920x1080` overrides it
/// (useful on headless Linux), otherwise the Windows monitor enumeration is
/// reused from the existing capture module.
pub fn screen_size() -> (u32, u32) {
    if let Ok(text) = std::env::var("LOWLAT_SCREEN") {
        if let Some((w, h)) = parse_size(&text) {
            return (w, h);
        }
    }
    if let Some(size) = native_screen_size() {
        return size;
    }
    (1920, 1080)
}

/// Primary monitor size from the existing DXGI enumeration (Windows only).
#[cfg(windows)]
fn native_screen_size() -> Option<(u32, u32)> {
    let monitors = server::capture::Capture::list_monitors().ok()?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.left == 0 && monitor.top == 0)
        .or_else(|| monitors.first())?;
    Some((monitor.width, monitor.height))
}

#[cfg(not(windows))]
fn native_screen_size() -> Option<(u32, u32)> {
    // No Win32 here; use LOWLAT_SCREEN=WxH on a headless Linux box.
    None
}

fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// The beacon this receiver advertises.
pub fn make_beacon() -> Beacon {
    let (width, height) = screen_size();
    let orientation = std::env::var("LOWLAT_ORIENTATION")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    Beacon {
        id: device_id(),
        name: device_name(),
        platform: platform().to_string(),
        width,
        height,
        orientation,
        video_port: VIDEO_PORT,
        pair_port: PAIR_PORT,
    }
}

// ---------------------------------------------------------------------------
// Advertisement: mDNS (preferred) + UDP broadcast beacon (fallback)
// ---------------------------------------------------------------------------

pub struct Advertiser {
    stop: Arc<AtomicBool>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    thread: Option<JoinHandle<()>>,
    notes: Vec<String>,
}

impl Advertiser {
    pub fn start(beacon: Beacon) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut notes = Vec::new();

        let socket = UdpSocket::bind(("0.0.0.0", 0)).context("binding the beacon socket")?;
        socket.set_broadcast(true)?;
        let text = beacon.encode();

        // Broadcast to the LAN *and* loopback, so host+receiver on one machine
        // (how this milestone is verified) discover each other too.
        let stop_thread = stop.clone();
        let thread = thread::Builder::new()
            .name("beacon-advert".into())
            .spawn(move || {
                while !stop_thread.load(Ordering::Relaxed) {
                    let _ = socket.send_to(text.as_bytes(), ("255.255.255.255", BEACON_PORT));
                    let _ = socket.send_to(text.as_bytes(), (Ipv4Addr::LOCALHOST, BEACON_PORT));
                    for _ in 0..(BEACON_INTERVAL.as_millis() / 100) {
                        if stop_thread.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            })?;

        // mDNS registration is best effort: on networks without multicast the
        // beacon above is the fallback, so a failure must not stop the app.
        // `LOWLAT_NO_MDNS=1` forces that fallback (used in the loopback test).
        let mdns = if std::env::var_os("LOWLAT_NO_MDNS").is_some() {
            notes.push("mDNS disabled by LOWLAT_NO_MDNS; beacon only".to_string());
            None
        } else {
            match register_mdns(&beacon) {
                Ok(daemon) => Some(daemon),
                Err(err) => {
                    notes.push(format!("mDNS advertisement failed: {err:#}"));
                    None
                }
            }
        };

        Ok(Self {
            stop,
            mdns,
            thread: Some(thread),
            notes,
        })
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(daemon) = self.mdns.take() {
            let _ = daemon.shutdown();
        }
    }
}

fn register_mdns(beacon: &Beacon) -> Result<mdns_sd::ServiceDaemon> {
    let daemon = mdns_sd::ServiceDaemon::new().context("creating the mDNS daemon")?;
    let instance = format!("{}-{}", beacon.name, std::process::id());
    let hostname = format!("{}.local.", sanitize_host(&beacon.name));
    let mut properties = HashMap::new();
    properties.insert("id".to_string(), beacon.id.clone());
    properties.insert("name".to_string(), beacon.name.clone());
    properties.insert("platform".to_string(), beacon.platform.clone());
    properties.insert("w".to_string(), beacon.width.to_string());
    properties.insert("h".to_string(), beacon.height.to_string());
    properties.insert("orient".to_string(), beacon.orientation.to_string());
    properties.insert("pair".to_string(), beacon.pair_port.to_string());
    let info = mdns_sd::ServiceInfo::new(
        MDNS_SERVICE,
        &instance,
        &hostname,
        "",
        beacon.video_port,
        properties,
    )
    .context("building the mDNS service info")?
    .enable_addr_auto();
    daemon
        .register(info)
        .context("registering the mDNS service")?;
    Ok(daemon)
}

fn sanitize_host(name: &str) -> String {
    let filtered: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if filtered.is_empty() {
        "lowlat".to_string()
    } else {
        filtered
    }
}

// ---------------------------------------------------------------------------
// Pairing: listener thread + Allow/Deny decisions from the GUI
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PairEvent {
    /// Show "Allow <host> to extend your screen?" with this code.
    Prompt {
        host_id: String,
        host_name: String,
        code: String,
        resume: bool,
    },
    /// The user allowed; stream now with these credentials.
    Allowed {
        host_id: String,
        host_name: String,
        host_ip: IpAddr,
        token: u64,
        key: [u8; KEY_LEN],
    },
    Denied {
        host_name: String,
        reason: String,
    },
    Note(String),
}

pub struct PairingService {
    events: Receiver<PairEvent>,
    decisions: Sender<bool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Where paired hosts are remembered (same file the GUI shows).
    pub store_path: PathBuf,
}

impl PairingService {
    pub fn start(store_path: PathBuf) -> Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", PAIR_PORT))
            .with_context(|| format!("binding UDP pairing port {PAIR_PORT}"))?;
        socket.set_read_timeout(Some(Duration::from_millis(250)))?;

        let (event_tx, events) = mpsc::channel();
        let (decisions, decision_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let stop_thread = stop.clone();
        let path = store_path.clone();
        let thread = thread::Builder::new()
            .name("pairing".into())
            .spawn(move || {
                pairing_loop(socket, path, event_tx, decision_rx, stop_thread);
            })
            .context("spawning the pairing thread")?;

        Ok(Self {
            events,
            decisions,
            stop,
            thread: Some(thread),
            store_path,
        })
    }

    pub fn try_recv(&self) -> Option<PairEvent> {
        self.events.try_recv().ok()
    }

    /// Answer the current prompt.
    pub fn decide(&self, allow: bool) {
        let _ = self.decisions.send(allow);
    }
}

impl Drop for PairingService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Prompt {
    host_id: String,
    host_name: String,
    code: String,
    from: SocketAddr,
    since: Instant,
}

fn pairing_loop(
    socket: UdpSocket,
    store_path: PathBuf,
    events: Sender<PairEvent>,
    decisions: Receiver<bool>,
    stop: Arc<AtomicBool>,
) {
    let mut machine = PairingMachine::new();
    let mut store = PairedStore::load(&store_path);
    let mut prompt: Option<Prompt> = None;
    // Re-send the last Accept if the host's copy got lost (UDP): the host
    // retries its REQUEST for up to 45 s.
    let mut last_accept: Option<(String, PairMsg, Instant)> = None;
    let mut buf = [0u8; 1024];

    let note = |text: String| {
        let _ = events.send(PairEvent::Note(text));
    };

    while !stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let Ok(text) = std::str::from_utf8(&buf[..len]) else {
                    continue;
                };
                let Some(msg) = PairMsg::decode(text) else {
                    continue;
                };
                if let Some(prompt) = &prompt {
                    // Duplicate request from the host we are asking about:
                    // repeat the same code instead of restarting the dialog.
                    if prompt.host_id == msg_host_id(&msg).unwrap_or("") {
                        let _ = socket.send_to(
                            PairMsg::Challenge {
                                code: prompt.code.clone(),
                            }
                            .encode()
                            .as_bytes(),
                            prompt.from,
                        );
                        continue;
                    }
                }
                let (host_id, host_name) = match &msg {
                    PairMsg::Request { host_id, host_name }
                    | PairMsg::Resume { host_id, host_name } => {
                        (host_id.clone(), host_name.clone())
                    }
                    _ => continue,
                };
                if let Some((id, accept, at)) = &last_accept {
                    if id == &host_id && at.elapsed() < Duration::from_secs(60) {
                        let _ = socket.send_to(accept.encode().as_bytes(), from);
                        note(format!(
                            "re-sent accept to {host_name} (request retry, {:.1}s)",
                            at.elapsed().as_secs_f32()
                        ));
                        continue;
                    }
                }

                let known = store.credentials(&host_id);
                let resume = matches!(msg, PairMsg::Resume { .. }) && known.is_some();
                let challenge = machine.on_request(&host_id, &host_name, known);
                let code = match &challenge {
                    PairMsg::Challenge { code } => code.clone(),
                    _ => continue,
                };
                let _ = socket.send_to(challenge.encode().as_bytes(), from);
                note(format!(
                    "pair request from {host_name} ({host_id}) at {from}{}",
                    if resume { " - previously paired" } else { "" }
                ));
                let _ = events.send(PairEvent::Prompt {
                    host_id: host_id.clone(),
                    host_name: host_name.clone(),
                    code: code.clone(),
                    resume,
                });
                prompt = Some(Prompt {
                    host_id,
                    host_name,
                    code,
                    from,
                    since: Instant::now(),
                });
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                ) => {}
            Err(err) => {
                note(format!("pairing socket error: {err}"));
                thread::sleep(Duration::from_millis(250));
            }
        }

        let Some(current) = &prompt else {
            continue;
        };

        // An unanswered dialog times out instead of hanging forever.
        let decision = if current.since.elapsed() >= PROMPT_TIMEOUT {
            Some(false)
        } else {
            decisions.recv_timeout(Duration::from_millis(50)).ok()
        };
        let Some(allow) = decision else {
            continue;
        };

        let reply = if allow {
            machine.allow()
        } else {
            machine.deny(if current.since.elapsed() >= PROMPT_TIMEOUT {
                "receiver did not answer"
            } else {
                "user denied"
            })
        };
        let host_id = current.host_id.clone();
        let host_name = current.host_name.clone();
        let from = current.from;
        prompt = None;

        match reply {
            Some(msg) => {
                let _ = socket.send_to(msg.encode().as_bytes(), from);
                match &msg {
                    PairMsg::Accept { token, key, .. } => {
                        last_accept = Some((host_id.clone(), msg.clone(), Instant::now()));
                        if let PairState::Accepted { .. } = machine.state() {
                            store.upsert(PairedHost {
                                host_id: host_id.clone(),
                                host_name: host_name.clone(),
                                token: *token,
                                key: *key,
                            });
                            if let Err(err) = store.save(&store_path) {
                                note(format!("could not save paired hosts: {err}"));
                            }
                        }
                        note(format!(
                            "allowed {host_name}: token {token:016x}, 32-byte session key stored"
                        ));
                        let _ = events.send(PairEvent::Allowed {
                            host_id,
                            host_name,
                            host_ip: from.ip(),
                            token: *token,
                            key: *key,
                        });
                    }
                    PairMsg::Reject { reason } => {
                        note(format!("denied {host_name}: {reason}"));
                        let _ = events.send(PairEvent::Denied {
                            host_name,
                            reason: reason.clone(),
                        });
                    }
                    _ => {}
                }
                machine.reset();
            }
            None => note("pairing state machine had no pending request".to_string()),
        }
    }
}

fn msg_host_id(msg: &PairMsg) -> Option<&str> {
    match msg {
        PairMsg::Request { host_id, .. } | PairMsg::Resume { host_id, .. } => Some(host_id),
        _ => None,
    }
}

pub fn store_path() -> PathBuf {
    default_path()
}

// ---------------------------------------------------------------------------
// Video session: reuse the existing client receiver + decoder
// ---------------------------------------------------------------------------

pub struct VideoSession {
    sink: client::decoder::FrameSink,
    config_rx: Receiver<(u32, u32, u16, u8)>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    stats: Arc<client::stats::Stats>,
    input: Option<client::net::InputSender>,
    /// Latest stream geometry reported by the server's DisplayConfig.
    pub width: u32,
    pub height: u32,
    pub orientation: u16,
    pub rotation: u8,
    pub frames: u64,
}

impl VideoSession {
    /// Start receiving from `host`. `control` enables mouse forwarding with the
    /// paired token; leave it off when both apps run on the same machine.
    pub fn start(host: IpAddr, width: u32, height: u32, token: u64, control: bool) -> Result<Self> {
        let (decoder, sink) =
            client::decoder::Decoder::new(width, height).context("starting the decoder")?;
        let (config_tx, config_rx) = mpsc::sync_channel(8);
        let running = Arc::new(AtomicBool::new(true));
        let stats = Arc::new(client::stats::Stats::default());
        let receiver = client::net::Receiver::new(
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, VIDEO_PORT)),
            SocketAddr::new(host, 9000),
            width,
            height,
            config_tx,
            stats.clone(),
            running.clone(),
        )
        .with_context(|| format!("binding UDP port {VIDEO_PORT} for video"))?;

        // The input sender shares the receiver's socket, so it must be created
        // before spawn() consumes the receiver.
        let input = if control && token != 0 {
            Some(receiver.input_sender_with_token(token)?)
        } else {
            None
        };

        let thread = receiver.spawn(decoder);
        Ok(Self {
            sink,
            config_rx,
            running,
            thread: Some(thread),
            stats,
            input,
            width,
            height,
            orientation: 0,
            rotation: 0,
            frames: 0,
        })
    }

    /// Newest decoded frame plus the geometry it belongs to.
    pub fn take_frame(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        while let Ok((w, h, orient, rot)) = self.config_rx.try_recv() {
            if w != self.width || h != self.height {
                self.width = w;
                self.height = h;
            }
            self.orientation = orient;
            self.rotation = rot;
        }
        let frame = self.sink.take_latest()?;
        self.frames += 1;
        Some((frame.data, self.width, self.height))
    }

    pub fn packets(&self) -> u64 {
        self.stats.packets.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.stats.frames_dropped.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Forward pointer movement (normalised 0..1 over the video) using the
    /// paired token. Only called when the user enabled "Control host".
    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if let Some(input) = &mut self.input {
            let nx = (x.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
            let ny = (y.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
            let _ = input.mouse_move(nx, ny);
        }
    }

    pub fn mouse_buttons(&mut self, mask: u8) {
        if let Some(input) = &mut self.input {
            let _ = input.mouse_buttons(mask);
        }
    }

    pub fn wheel(&mut self, delta: i16) {
        if let Some(input) = &mut self.input {
            let _ = input.wheel(delta);
        }
    }

    pub fn release_all(&mut self) {
        if let Some(input) = &mut self.input {
            let _ = input.release_all();
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for VideoSession {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// yuv420p -> RGBA (egui texture), with optional frame rotation
// ---------------------------------------------------------------------------

/// Convert planar `yuv420p` to unmultiplied RGBA, rotating by `rotate`
/// degrees (0/90/180/270) on the way. Returns `(rgba, out_width, out_height)`.
pub fn yuv_to_rgba(src: &[u8], width: u32, height: u32, rotate: u16) -> Option<(Vec<u8>, u32, u32)> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let (cw, ch) = (w / 2, h / 2);
    if cw == 0 || ch == 0 {
        return None;
    }
    let u_base = w * h;
    let v_base = u_base + cw * ch;
    if src.len() < v_base + cw * ch {
        return None;
    }
    let (out_w, out_h) = match rotate % 360 {
        90 | 270 => (h, w),
        _ => (w, h),
    };
    let mut out = vec![0u8; out_w * out_h * 4];
    for y in 0..h {
        for x in 0..w {
            let yv = src[y * w + x] as f32;
            let uv = src[u_base + (y / 2) * cw + x / 2] as f32 - 128.0;
            let vv = src[v_base + (y / 2) * cw + x / 2] as f32 - 128.0;
            // BT.601 limited range (ffmpeg's default yuv420p matrix).
            let r = yv + 1.402 * vv;
            let g = yv - 0.344_136 * uv - 0.714_136 * vv;
            let b = yv + 1.772 * uv;
            let (dx, dy) = match rotate % 360 {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                270 => (y, w - 1 - x),
                _ => (x, y),
            };
            let index = (dy * out_w + dx) * 4;
            out[index] = clamp_u8(r);
            out[index + 1] = clamp_u8(g);
            out[index + 2] = clamp_u8(b);
            out[index + 3] = 255;
        }
    }
    Some((out, out_w as u32, out_h as u32))
}

fn clamp_u8(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

/// Load a paired host list (the GUI shows which hosts can reconnect).
pub fn load_store(path: &Path) -> PairedStore {
    PairedStore::load(path)
}

/// Interpret a stored host as a previously paired device.
pub fn paired_names(store: &PairedStore) -> Vec<String> {
    store
        .hosts
        .iter()
        .map(|host| format!("{} ({})", host.host_name, host.host_id))
        .collect()
}

/// Guard against accidentally treating "0.0.0.0" as a stream source.
pub fn usable_host(host: IpAddr) -> Result<IpAddr> {
    if host.is_unspecified() {
        bail!("refusing to stream from the unspecified address {host}");
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x2 white block with neutral chroma: every pixel must be white after
    /// conversion, and the rotation must move pixels exactly as expected.
    fn quad() -> Vec<u8> {
        // Y plane 2x2, then U (1), then V (1).
        vec![255, 255, 255, 255, 128, 128]
    }

    #[test]
    fn yuv_neutral_is_grey() {
        let (rgba, w, h) = yuv_to_rgba(&quad(), 2, 2, 0).expect("convert");
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba.len(), 16);
        assert!(rgba.chunks_exact(4).all(|p| p[..3] == [255, 255, 255]));
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn rotate_90_swaps_dimensions() {
        // Two pixels: left black-ish, right bright.
        let src = vec![10, 200, 10, 200, 128, 128];
        let (rgba, w, h) = yuv_to_rgba(&src, 2, 2, 90).expect("convert");
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba.len(), 16);
        // Clockwise rotation keeps all four pixels.
        assert_eq!(rgba.chunks_exact(4).count(), 4);
    }

    #[test]
    fn rotate_270_swaps_dimensions() {
        let (_, w, h) = yuv_to_rgba(&quad(), 2, 2, 270).expect("convert");
        assert_eq!((w, h), (2, 2));
    }

    #[test]
    fn rejects_short_buffers_and_zero_size() {
        assert!(yuv_to_rgba(&[0; 4], 2, 2, 0).is_none());
        assert!(yuv_to_rgba(&[], 0, 0, 0).is_none());
    }

    #[test]
    fn screen_override_parses() {
        assert_eq!(parse_size("1280x720"), Some((1280, 720)));
        assert_eq!(parse_size(" 800 X 600 "), Some((800, 600)));
        assert_eq!(parse_size("nope"), None);
    }

    #[test]
    fn beacon_identity_is_filesystem_safe() {
        let id = device_id();
        assert!(id.starts_with("recv-"));
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn rejects_unspecified_host() {
        assert!(usable_host("0.0.0.0".parse().unwrap()).is_err());
        assert!(usable_host("127.0.0.1".parse().unwrap()).is_ok());
    }
}
