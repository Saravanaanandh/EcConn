//! `app-host` core: LAN discovery, pairing, and running the existing `server`
//! pipeline for a chosen receiver.
//!
//! This module is Windows-only because it calls `server::virtual_display` and
//! `server::capture` in-process (see `Cargo.toml`). The capture/encode/stream
//! loop itself is *not* rewritten: the host spawns the existing `server.exe`
//! with the appropriate flags, exactly as the CLI does.

#![cfg(windows)]

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use common::discovery::{Beacon, BEACON_PORT, MDNS_SERVICE};
use common::pairing::{PairMsg, KEY_LEN};
use common::store::{default_path, PairedHost, PairedStore};

/// Platform string advertised to receivers.
pub const PLATFORM: &str = "windows";
/// Ports the host opens (documented for the firewall helper). The receiver
/// listens on 9100 for video, so that port belongs to the other machine.
pub const HOST_PORTS: &[u16] = &[9000, BEACON_PORT, common::discovery::PAIR_PORT];

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// One receiver found on the LAN.
#[derive(Clone, Debug)]
pub struct Device {
    pub beacon: Beacon,
    pub addr: IpAddr,
    /// `"beacon"` or `"mdns"`, for display.
    pub source: &'static str,
    pub last_seen: Instant,
}

impl Device {
    pub fn video_addr(&self) -> SocketAddr {
        SocketAddr::new(self.addr, self.beacon.video_port)
    }
    pub fn pair_addr(&self) -> SocketAddr {
        SocketAddr::new(self.addr, self.beacon.pair_port)
    }
}

struct DiscoveryShared {
    devices: HashMap<String, Device>,
    notes: Vec<String>,
}

/// Background mDNS + UDP-broadcast listener. Drop it to stop the threads.
pub struct Discovery {
    shared: Arc<Mutex<DiscoveryShared>>,
    stop: Arc<AtomicBool>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    _threads: Vec<JoinHandle<()>>,
}

impl Discovery {
    pub fn start() -> Result<Self> {
        let shared = Arc::new(Mutex::new(DiscoveryShared {
            devices: HashMap::new(),
            notes: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        // Fallback: UDP broadcast beacon on BEACON_PORT.
        let socket = UdpSocket::bind(("0.0.0.0", BEACON_PORT)).with_context(|| {
            format!("binding UDP port {BEACON_PORT} for discovery (another host may be scanning)")
        })?;
        socket.set_read_timeout(Some(Duration::from_millis(400)))?;
        {
            let shared = shared.clone();
            let stop = stop.clone();
            threads.push(thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while !stop.load(Ordering::Relaxed) {
                    if let Ok((len, from)) = socket.recv_from(&mut buf) {
                        if let Ok(text) = std::str::from_utf8(&buf[..len]) {
                            if let Some(beacon) = Beacon::parse(text) {
                                let mut shared = shared.lock().unwrap();
                                shared.devices.insert(
                                    beacon.id.clone(),
                                    Device {
                                        beacon,
                                        addr: from.ip(),
                                        source: "beacon",
                                        last_seen: Instant::now(),
                                    },
                                );
                            }
                        }
                    }
                }
            }));
        }

        // Preferred: mDNS browse. Best effort; failures are surfaced as notes.
        let mut mdns = None;
        if std::env::var_os("LOWLAT_NO_MDNS").is_some() {
            shared
                .lock()
                .unwrap()
                .notes
                .push("mDNS disabled by LOWLAT_NO_MDNS; beacon only".to_string());
        } else {
            match mdns_sd::ServiceDaemon::new() {
                Ok(daemon) => match daemon.browse(MDNS_SERVICE) {
                    Ok(receiver) => {
                        let shared = shared.clone();
                        threads.push(thread::spawn(move || {
                            while let Ok(event) = receiver.recv() {
                                if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                                    if let Some(device) = device_from_mdns(&info) {
                                        let mut shared = shared.lock().unwrap();
                                        shared.devices.insert(device.beacon.id.clone(), device);
                                    }
                                }
                            }
                        }));
                        mdns = Some(daemon);
                    }
                    Err(err) => shared
                        .lock()
                        .unwrap()
                        .notes
                        .push(format!("mDNS browse failed: {err}")),
                },
                Err(err) => shared
                    .lock()
                    .unwrap()
                    .notes
                    .push(format!("mDNS daemon failed: {err}")),
            }
        }

        Ok(Self {
            shared,
            stop,
            mdns,
            _threads: threads,
        })
    }

    /// Devices seen in the last 15 s, sorted by name.
    pub fn snapshot(&self) -> Vec<Device> {
        let shared = self.shared.lock().unwrap();
        let now = Instant::now();
        let mut list: Vec<Device> = shared
            .devices
            .values()
            .filter(|device| now.duration_since(device.last_seen) < Duration::from_secs(15))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.beacon.name.cmp(&b.beacon.name));
        list
    }

    /// Non-fatal discovery notes (e.g. mDNS unavailable).
    pub fn notes(&self) -> Vec<String> {
        self.shared.lock().unwrap().notes.clone()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mdns) = self.mdns.take() {
            let _ = mdns.shutdown();
        }
    }
}

fn device_from_mdns(info: &mdns_sd::ResolvedService) -> Option<Device> {
    let addr: IpAddr = info.get_addresses_v4().into_iter().next()?.into();
    let text = |key: &str| info.get_property_val_str(key).map(str::to_string);
    let beacon = Beacon {
        id: text("id")?,
        name: text("name").unwrap_or_else(|| info.get_fullname().to_string()),
        platform: text("platform").unwrap_or_else(|| "unknown".to_string()),
        width: text("w").and_then(|v| v.parse().ok()).unwrap_or(0),
        height: text("h").and_then(|v| v.parse().ok()).unwrap_or(0),
        orientation: text("orient").and_then(|v| v.parse().ok()).unwrap_or(0),
        video_port: info.get_port(),
        pair_port: text("pair")
            .and_then(|v| v.parse().ok())
            .unwrap_or(common::discovery::PAIR_PORT),
    };
    Some(Device {
        beacon,
        addr,
        source: "mdns",
        last_seen: Instant::now(),
    })
}

// ---------------------------------------------------------------------------
// Pairing (host side)
// ---------------------------------------------------------------------------

/// Credentials agreed with a receiver.
#[derive(Clone, Debug)]
pub struct PairOutcome {
    pub code: String,
    pub token: u64,
    pub key: [u8; KEY_LEN],
}

/// Progress of a pairing attempt, delivered on a channel so the GUI stays live.
#[derive(Debug)]
pub enum PairEvent {
    Log(String),
    Challenge(String),
    Success(PairOutcome),
    Failed(String),
}

pub struct PairingJob {
    rx: Receiver<PairEvent>,
}

impl PairingJob {
    pub fn try_recv(&self) -> Option<PairEvent> {
        self.rx.try_recv().ok()
    }
}

/// Pair with `device` on a background thread. `known` is the stored
/// `(token, key)` for a returning host, else `None`.
pub fn start_pairing(
    device: Device,
    host_id: String,
    host_name: String,
    known: Option<(u64, [u8; KEY_LEN])>,
) -> PairingJob {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = pair_with_receiver(&device, &host_id, &host_name, known, &tx);
        if let Err(err) = result {
            let _ = tx.send(PairEvent::Failed(format!("{err:#}")));
        }
    });
    PairingJob { rx }
}

fn pair_with_receiver(
    device: &Device,
    host_id: &str,
    host_name: &str,
    known: Option<(u64, [u8; KEY_LEN])>,
    events: &mpsc::Sender<PairEvent>,
) -> Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).context("binding the pairing socket")?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    let target = device.pair_addr();
    let request = if known.is_some() {
        PairMsg::Resume {
            host_id: host_id.to_string(),
            host_name: host_name.to_string(),
        }
    } else {
        PairMsg::Request {
            host_id: host_id.to_string(),
            host_name: host_name.to_string(),
        }
    };
    socket
        .send_to(request.encode().as_bytes(), target)
        .with_context(|| format!("sending the pairing request to {target}"))?;
    let _ = events.send(PairEvent::Log(format!(
        "sent {} to {target}",
        if known.is_some() { "RESUME" } else { "REQUEST" }
    )));

    // Retry the request a couple of times: UDP has no delivery guarantee.
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut last_resend = Instant::now();
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline {
        if last_resend.elapsed() >= Duration::from_secs(3) {
            let _ = socket.send_to(request.encode().as_bytes(), target);
            last_resend = Instant::now();
        }
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                if from.ip() != device.addr && !from.ip().is_loopback() {
                    continue;
                }
                let Ok(text) = std::str::from_utf8(&buf[..len]) else {
                    continue;
                };
                match PairMsg::decode(text) {
                    Some(PairMsg::Challenge { code }) => {
                        let _ = events.send(PairEvent::Log(format!(
                            "receiver asks to confirm code {code}"
                        )));
                        let _ = events.send(PairEvent::Challenge(code));
                    }
                    Some(PairMsg::Accept { code, token, key }) => {
                        let _ = events.send(PairEvent::Log(format!(
                            "PAIR_ACCEPT code={code} token={token:016x}"
                        )));
                        let _ = events.send(PairEvent::Success(PairOutcome { code, token, key }));
                        return Ok(());
                    }
                    Some(PairMsg::Reject { reason }) => {
                        bail!("receiver rejected pairing: {reason}");
                    }
                    _ => {}
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                ) => {}
            Err(err) => return Err(err).context("receiving the pairing reply"),
        }
    }
    bail!("pairing timed out after 45 s")
}

// ---------------------------------------------------------------------------
// Streaming session (spawns the existing server binary)
// ---------------------------------------------------------------------------

/// Resolve the sibling `server` binary built by this workspace.
pub fn server_exe() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("LOWLAT_SERVER_EXE") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().context("locating the host executable")?;
    let candidate = exe.with_file_name(if cfg!(windows) { "server.exe" } else { "server" });
    if candidate.is_file() {
        return Ok(candidate);
    }
    Ok(PathBuf::from(if cfg!(windows) { "server.exe" } else { "server" }))
}

/// What the user chose in the Connect panel.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub dest: SocketAddr,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub orientation: u16,
    pub token: u64,
    /// Try to create a virtual monitor at the receiver's size.
    pub use_virtual: bool,
    /// Monitor to mirror when virtual is unavailable / disabled.
    pub mirror_monitor: u32,
    pub encode_path: String,
}

pub struct HostSession {
    child: Child,
    pub log: Arc<Mutex<VecDeque<String>>>,
    pub virtual_attached: bool,
    pub mode: &'static str,
}

impl HostSession {
    pub fn start(config: &SessionConfig, mut note: impl FnMut(&str)) -> Result<Self> {
        let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

        // Decide the capture monitor. The existing `virtual_display` module is
        // used in-process; the streaming pipeline stays in server.exe.
        let mut monitor = config.mirror_monitor;
        let mut virtual_attached = false;
        let mut mode = "mirror";
        let mut encode_w = config.width;
        let mut encode_h = config.height;
        if config.use_virtual {
            if server::virtual_display::is_driver_installed() {
                match server::virtual_display::attach(config.width, config.height, config.fps) {
                    Ok(index) => {
                        monitor = index;
                        virtual_attached = true;
                        mode = "virtual";
                        note(&format!(
                            "virtual display attached as monitor {index} ({}x{}@{})",
                            config.width, config.height, config.fps
                        ));
                        // The driver may apply its own preferred mode instead of
                        // the new one. Encode the size the monitor actually has,
                        // otherwise ffmpeg stretches the desktop.
                        if let Ok(monitors) = server::capture::Capture::list_monitors() {
                            if let Some(found) = monitors.iter().find(|m| m.index == index) {
                                if found.width != encode_w || found.height != encode_h {
                                    note(&format!(
                                        "virtual monitor is actually {}x{} (asked for {}x{}); encoding {}x{}",
                                        found.width,
                                        found.height,
                                        encode_w,
                                        encode_h,
                                        found.width,
                                        found.height
                                    ));
                                    encode_w = found.width;
                                    encode_h = found.height;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        note(&format!(
                            "virtual display unavailable ({err:#}); falling back to mirror mode"
                        ));
                    }
                }
            } else {
                note("virtual display driver is not installed; falling back to mirror mode");
            }
        } else {
            note("virtual mode disabled; using mirror mode");
        }

        let mut command = Command::new(server_exe()?);
        command
            .arg("--encode-path")
            .arg(&config.encode_path)
            .arg("--monitor")
            .arg(monitor.to_string())
            .arg("--width")
            .arg(encode_w.to_string())
            .arg("--height")
            .arg(encode_h.to_string())
            .arg("--fps")
            .arg(config.fps.to_string())
            .arg("--dest")
            .arg(config.dest.to_string())
            .arg("--bind")
            .arg("0.0.0.0:9000")
            .arg("--token")
            .arg(config.token.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if config.orientation != 0 && virtual_attached {
            command.arg("--orientation").arg(config.orientation.to_string());
        }

        note(&format!(
            "starting {} (monitor {monitor}, {}x{}@{}) -> {}{}",
            command.get_program().to_string_lossy(),
            encode_w,
            encode_h,
            config.fps,
            config.dest,
            if config.orientation != 0 {
                format!(", {} deg", config.orientation)
            } else {
                String::new()
            }
        ));

        let mut child = command.spawn().with_context(|| {
            format!(
                "starting server.exe ({})",
                command.get_program().to_string_lossy()
            )
        })?;
        if let Some(stdout) = child.stdout.take() {
            spawn_log_reader(stdout, log.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_reader(stderr, log.clone());
        }

        Ok(Self {
            child,
            log,
            virtual_attached,
            mode,
        })
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Last few log lines, newest last.
    pub fn recent(&self, count: usize) -> Vec<String> {
        let log = self.log.lock().unwrap();
        log.iter()
            .rev()
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Parse the most recent server status line: `(fps, kbit/s)`.
    pub fn latest_stats(&self) -> Option<(u32, f64)> {
        let log = self.log.lock().unwrap();
        for line in log.iter().rev() {
            if let Some(rest) = line.strip_prefix("sent") {
                // "sent  120 frames,  137 packets,    70.0 kbit/s, ..."
                let frames = rest
                    .split("frames")
                    .next()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                let kbit = rest
                    .split("packets,")
                    .nth(1)
                    .and_then(|s| s.split("kbit/s").next())
                    .and_then(|s| s.trim().parse::<f64>().ok());
                if let (Some(frames), Some(kbit)) = (frames, kbit) {
                    return Some((frames, kbit));
                }
            }
        }
        None
    }

    pub fn stop(&mut self, mut note: impl FnMut(&str)) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.virtual_attached {
            server::virtual_display::detach();
            note("virtual display detached");
        }
        note("server process stopped");
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.virtual_attached {
            server::virtual_display::detach();
        }
    }
}

fn spawn_log_reader<R: std::io::Read + Send + 'static>(
    source: R,
    log: Arc<Mutex<VecDeque<String>>>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(source);
        for line in reader.lines().map_while(Result::ok) {
            let mut log = log.lock().unwrap();
            log.push_back(line);
            while log.len() > 500 {
                log.pop_front();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Paired-host store helpers
// ---------------------------------------------------------------------------

pub fn load_store() -> (PairedStore, PathBuf) {
    // The receiver stores the hosts that may connect *to it*; the host stores
    // the receivers it met. A separate file keeps the two roles from sharing
    // one list when both apps run on one machine (as in the loopback test).
    let path = default_path().with_file_name("paired_receivers.txt");
    (PairedStore::load(&path), path)
}

pub fn remember_paired(
    store: &mut PairedStore,
    path: &std::path::Path,
    host_id: &str,
    host_name: &str,
    outcome: &PairOutcome,
) -> Result<()> {
    store.upsert(PairedHost {
        host_id: host_id.to_string(),
        host_name: host_name.to_string(),
        token: outcome.token,
        key: outcome.key,
    });
    store.save(path).context("saving paired hosts")?;
    Ok(())
}

/// Stable-ish id for this host installation.
pub fn host_id() -> String {
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "host".to_string());
    format!("host-{name}")
}

pub fn host_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows host".to_string())
}

// ---------------------------------------------------------------------------
// Firewall helper (explicit, never run on startup)
// ---------------------------------------------------------------------------

/// The exact `netsh` command the README documents.
pub fn firewall_command() -> String {
    format!(
        "netsh advfirewall firewall add rule name=\"lowlat-screen\" dir=in action=allow protocol=UDP localport={}",
        HOST_PORTS
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Run the firewall helper. The caller must have shown the user the command
/// first (the GUI does, and this is never called silently on startup).
pub fn add_firewall_rule() -> Result<String> {
    let output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            "name=lowlat-screen",
            "dir=in",
            "action=allow",
            "protocol=UDP",
            &format!(
                "localport={}",
                HOST_PORTS
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ])
        .output()
        .context("running netsh")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(if stdout.is_empty() {
            "firewall rule added".to_string()
        } else {
            stdout
        })
    } else {
        Ok(format!(
            "netsh failed (run as administrator): {}{}{}",
            stdout,
            if stdout.is_empty() || stderr.is_empty() { "" } else { " / " },
            stderr
        ))
    }
}
