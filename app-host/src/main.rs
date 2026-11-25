//! `app-host`: discover receivers on the LAN, pair, and start/stop extending.
//!
//! Run the GUI with no arguments. `--demo` runs the same discovery/pairing/
//! streaming code without a window, which is how it is verified on loopback.

#[cfg(windows)]
use std::net::SocketAddr;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use eframe::egui;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--demo") {
        if let Err(err) = run_demo(&args) {
            eprintln!("demo error: {err:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(err) = run_gui() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("app-host is Windows-only (it drives DXGI capture and the virtual display driver)");
}

// ---------------------------------------------------------------------------
// GUI
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn run_gui() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "lowlat-screen host",
        options,
        Box::new(|_cc| {
            let app = HostApp::new().map_err(
                |err: anyhow::Error| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("{err:#}").into()
                },
            )?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
}

#[cfg(windows)]
struct HostApp {
    discovery: app_host::Discovery,
    devices: Vec<app_host::Device>,
    selected: Option<usize>,
    width: u32,
    height: u32,
    fps: u32,
    orientation: u16,
    use_virtual: bool,
    encode_path: String,
    pairing: Option<app_host::PairingJob>,
    code: Option<String>,
    session: Option<app_host::HostSession>,
    store: common::store::PairedStore,
    store_path: std::path::PathBuf,
    log: Vec<String>,
    stats: Option<(u32, f64)>,
    firewall_output: Option<String>,
    host_id: String,
    host_name: String,
    scan_at: Instant,
}

#[cfg(windows)]
impl HostApp {
    fn new() -> anyhow::Result<Self> {
        let discovery = app_host::Discovery::start()?;
        let (store, store_path) = app_host::load_store();
        Ok(Self {
            discovery,
            devices: Vec::new(),
            selected: None,
            width: 1280,
            height: 720,
            fps: 30,
            orientation: 0,
            use_virtual: true,
            encode_path: "auto".to_string(),
            pairing: None,
            code: None,
            session: None,
            store,
            store_path,
            log: Vec::new(),
            stats: None,
            firewall_output: None,
            host_id: app_host::host_id(),
            host_name: app_host::host_name(),
            scan_at: Instant::now(),
        })
    }

    fn push(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 300 {
            self.log.remove(0);
        }
    }

    fn selected_device(&self) -> Option<app_host::Device> {
        self.selected.and_then(|index| self.devices.get(index).cloned())
    }

    fn start_pairing(&mut self) {
        let Some(device) = self.selected_device() else {
            self.push("select a device first");
            return;
        };
        if self.session.is_some() {
            self.push("stop the current session first");
            return;
        }
        let known = self.store.credentials(&device.beacon.id);
        self.width = device.beacon.width.max(320);
        self.height = device.beacon.height.max(240);
        self.orientation = device.beacon.orientation;
        self.code = None;
        self.push(format!(
            "pairing with {} at {}{}",
            device.beacon.name,
            device.pair_addr(),
            if known.is_some() { " (paired before: one-click confirm)" } else { "" }
        ));
        self.pairing = Some(app_host::start_pairing(
            device.clone(),
            self.host_id.clone(),
            self.host_name.clone(),
            known,
        ));
    }

    fn poll_pairing(&mut self) {
        let mut events = Vec::new();
        if let Some(job) = &self.pairing {
            while let Some(event) = job.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                app_host::PairEvent::Log(line) => self.push(line),
                app_host::PairEvent::Challenge(code) => {
                    self.code = Some(code.clone());
                    self.push(format!("show this code on both screens: {code}"));
                }
                app_host::PairEvent::Success(outcome) => {
                    self.pairing = None;
                    self.push(format!(
                        "paired: token {:016x}, 32-byte session key stored",
                        outcome.token
                    ));
                    if let Some(device) = self.selected_device() {
                        let _ = app_host::remember_paired(
                            &mut self.store,
                            &self.store_path,
                            &device.beacon.id,
                            &device.beacon.name,
                            &outcome,
                        );
                        let config = app_host::SessionConfig {
                            dest: SocketAddr::new(device.addr, device.beacon.video_port),
                            width: self.width,
                            height: self.height,
                            fps: self.fps,
                            orientation: self.orientation,
                            token: outcome.token,
                            use_virtual: self.use_virtual,
                            mirror_monitor: 0,
                            encode_path: self.encode_path.clone(),
                        };
                        let mut notes = Vec::new();
                        match app_host::HostSession::start(&config, |line| notes.push(line.to_string()))
                        {
                            Ok(session) => {
                                for line in notes {
                                    self.push(line);
                                }
                                self.session = Some(session);
                            }
                            Err(err) => {
                                self.push(format!("could not start the stream: {err:#}"));
                            }
                        }
                    }
                }
                app_host::PairEvent::Failed(err) => {
                    self.pairing = None;
                    self.push(format!("pairing failed: {err}"));
                }
            }
        }
    }

    fn stop_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            let mut notes = Vec::new();
            session.stop(|line| notes.push(line.to_string()));
            for line in notes {
                self.push(line);
            }
        }
        self.stats = None;
    }
}

#[cfg(windows)]
impl eframe::App for HostApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_pairing();

        if self.scan_at.elapsed() >= Duration::from_millis(500) {
            self.devices = self.discovery.snapshot();
            if self.selected.map_or(true, |index| index >= self.devices.len()) {
                self.selected = if self.devices.is_empty() { None } else { Some(0) };
            }
            self.scan_at = Instant::now();
        }

        let mut ended = false;
        if let Some(session) = &mut self.session {
            self.stats = session.latest_stats();
            ended = !session.is_running();
        }
        if ended {
            self.session = None;
            self.stats = None;
            self.push("stream ended");
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("lowlat-screen host");
            ui.label(format!(
                "Host: {} ({})",
                self.host_name, self.host_id
            ));
            ui.separator();

            ui.horizontal(|ui| {
                ui.heading("Receivers on your LAN");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} found", self.devices.len()));
                });
            });

            if self.devices.is_empty() {
                ui.label("Scanning… make sure the receiver app is open on the LAN.");
            }
            egui::ScrollArea::vertical()
                .id_salt("devices")
                .max_height(150.0)
                .show(ui, |ui| {
                    for (index, device) in self.devices.iter().enumerate() {
                        let label = format!(
                            "{} — {} {}x{} @{} [{}{}]",
                            device.beacon.name,
                            device.addr,
                            device.beacon.width,
                            device.beacon.height,
                            device.beacon.platform,
                            device.source,
                            if device.beacon.orientation != 0 {
                                format!(", {}°", device.beacon.orientation)
                            } else {
                                String::new()
                            }
                        );
                        if ui
                            .selectable_label(self.selected == Some(index), label)
                            .clicked()
                        {
                            self.selected = Some(index);
                            if let Some(device) = self.devices.get(index) {
                                self.width = device.beacon.width.max(320);
                                self.height = device.beacon.height.max(240);
                                self.orientation = device.beacon.orientation;
                            }
                        }
                    }
                });

            for note in self.discovery.notes() {
                ui.colored_label(egui::Color32::YELLOW, note);
            }

            ui.separator();
            ui.heading("Stream settings");
            ui.horizontal(|ui| {
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut self.width).range(320..=3840));
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut self.height).range(240..=2160));
            });
            ui.horizontal(|ui| {
                ui.label("FPS");
                ui.add(egui::DragValue::new(&mut self.fps).range(5..=120));
                ui.label("Orientation");
                egui::ComboBox::from_id_salt("orient")
                    .selected_text(format!("{}°", self.orientation))
                    .show_ui(ui, |ui| {
                        for value in [0u16, 90, 180, 270] {
                            ui.selectable_value(&mut self.orientation, value, format!("{value}°"));
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.use_virtual, "Create virtual monitor");
                ui.label("encoder");
                egui::ComboBox::from_id_salt("enc")
                    .selected_text(self.encode_path.clone())
                    .show_ui(ui, |ui| {
                        for value in ["auto", "libx264", "nvenc", "qsv", "amf"] {
                            ui.selectable_value(
                                &mut self.encode_path,
                                value.to_string(),
                                value,
                            );
                        }
                    });
            });

            ui.separator();
            ui.horizontal(|ui| {
                let can_pair = self.selected.is_some() && self.session.is_none() && self.pairing.is_none();
                if ui
                    .add_enabled(can_pair, egui::Button::new("Pair && Connect"))
                    .clicked()
                {
                    self.start_pairing();
                }
                let can_stop = self.session.is_some();
                if ui
                    .add_enabled(can_stop, egui::Button::new("Stop"))
                    .clicked()
                {
                    self.stop_session();
                }
                if self.pairing.is_some() {
                    ui.spinner();
                    ui.label("waiting for the receiver to allow…");
                }
            });

            if let Some(code) = &self.code {
                ui.separator();
                ui.heading(format!("Pairing code: {code}"));
                ui.label("Confirm this is the same code the receiver shows, then click Allow there.");
            }

            if let Some(session) = &self.session {
                ui.separator();
                ui.heading(format!("Streaming ({})", session.mode));
                ui.label(format!("to {}", self.selected_device().map(|d| d.video_addr().to_string()).unwrap_or_default()));
                match self.stats {
                    Some((frames, kbit)) => {
                        ui.label(format!(
                            "sent {frames} frames total · {kbit:.1} kbit/s · latency: not measured"
                        ));
                    }
                    None => {
                        ui.label("waiting for the first status line…");
                    }
                }
                egui::ScrollArea::vertical()
                    .id_salt("session-log")
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in session.recent(40) {
                            ui.monospace(line);
                        }
                    });
            }

            ui.separator();
            ui.collapsing("Windows firewall helper", |ui| {
                ui.label("Open these UDP ports so the receiver can reach this host:");
                ui.monospace(app_host::firewall_command());
                ui.label("This is not run silently; click the button and check the output.");
                if ui.button("Run firewall helper").clicked() {
                    self.firewall_output = Some(match app_host::add_firewall_rule() {
                        Ok(text) => text,
                        Err(err) => format!("{err:#}"),
                    });
                }
                if let Some(output) = &self.firewall_output {
                    ui.monospace(output);
                }
            });

            ui.separator();
            ui.collapsing("Event log", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("event-log")
                    .max_height(160.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });
        });

        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------------------
// Headless demo (used to verify discovery/pair/start/stop on loopback)
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn run_demo(args: &[String]) -> anyhow::Result<()> {
    let value = |flag: &str| -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let want_virtual = args.iter().any(|arg| arg == "--virtual");
    let duration: u64 = value("--duration")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let want_receiver = value("--receiver");

    println!("[host] starting discovery");
    let discovery = app_host::Discovery::start()?;
    for note in discovery.notes() {
        println!("[host] note: {note}");
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut chosen = None;
    while Instant::now() < deadline && chosen.is_none() {
        let devices = discovery.snapshot();
        if !devices.is_empty() {
            for device in &devices {
                println!(
                    "[host] discovered {} at {} ({}x{}, {}) via {}",
                    device.beacon.name,
                    device.addr,
                    device.beacon.width,
                    device.beacon.height,
                    device.beacon.platform,
                    device.source
                );
            }
            chosen = devices
                .into_iter()
                .find(|device| {
                    want_receiver
                        .as_deref()
                        .map_or(true, |want| device.addr.to_string() == want)
                });
        }
        if chosen.is_none() {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    let device = chosen.ok_or_else(|| anyhow::anyhow!("no receiver discovered within 15 s"))?;
    println!(
        "[host] selected {} at {} (video {}, pair {})",
        device.beacon.name,
        device.addr,
        device.video_addr(),
        device.pair_addr()
    );

    let (store, store_path) = app_host::load_store();
    let host_id = app_host::host_id();
    let host_name = app_host::host_name();
    let known = store.credentials(&device.beacon.id);
    println!(
        "[host] pairing ({}), host='{}'",
        if known.is_some() { "resume" } else { "first contact" },
        host_name
    );
    let job = app_host::start_pairing(
        device.clone(),
        host_id,
        host_name,
        known,
    );
    let outcome = loop {
        match job.try_recv() {
            Some(app_host::PairEvent::Log(line)) => println!("[host] {line}"),
            Some(app_host::PairEvent::Challenge(code)) => {
                println!("[host] PAIRING CODE: {code}  (matches the receiver dialog)");
            }
            Some(app_host::PairEvent::Success(outcome)) => break outcome,
            Some(app_host::PairEvent::Failed(err)) => {
                anyhow::bail!("pairing failed: {err}")
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    println!(
        "[host] PAIRED token={:016x} key={} bytes (stored for the next task)",
        outcome.token,
        outcome.key.len()
    );
    {
        let mut store = store;
        app_host::remember_paired(
            &mut store,
            &store_path,
            &device.beacon.id,
            &device.beacon.name,
            &outcome,
        )?;
        println!("[host] saved paired host to {}", store_path.display());
    }

    let config = app_host::SessionConfig {
        dest: SocketAddr::new(device.addr, device.beacon.video_port),
        width: device.beacon.width.max(320),
        height: device.beacon.height.max(240),
        fps: 30,
        orientation: device.beacon.orientation,
        token: outcome.token,
        use_virtual: want_virtual,
        mirror_monitor: 0,
        encode_path: "libx264".to_string(),
    };
    let mut notes = Vec::new();
    let mut session =
        app_host::HostSession::start(&config, |line| notes.push(line.to_string()))?;
    for line in notes {
        println!("[host] {line}");
    }

    let started = Instant::now();
    let mut seen = 0usize;
    while started.elapsed() < Duration::from_secs(duration) {
        std::thread::sleep(Duration::from_millis(1000));
        let lines = session.recent(1000);
        for line in lines.iter().skip(seen) {
            println!("[server] {line}");
        }
        seen = lines.len();
        if let Some((frames, kbit)) = session.latest_stats() {
            println!("[host] status: {frames} frames total, {kbit:.1} kbit/s");
        }
        if !session.is_running() {
            println!("[host] server exited early");
            break;
        }
    }

    let mut notes = Vec::new();
    session.stop(|line| notes.push(line.to_string()));
    for line in notes {
        println!("[host] {line}");
    }
    println!("[host] stopped; virtual_monitor={}", session.virtual_attached);
    Ok(())
}
