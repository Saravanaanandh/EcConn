//! `app-host`: discover receivers on the LAN, pair, and start/stop extending.
//!
//! Run the GUI with no arguments. `--demo` runs the same discovery/pairing/
//! streaming code without a window, which is how it is verified on loopback.

mod receiver_pane;
mod ui_template;

#[cfg(windows)]
use std::net::SocketAddr;
use std::time::{Duration, Instant};

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
    if let Err(err) = run_linux_gui() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run_linux_gui() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 840.0])
            .with_min_inner_size([720.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EcConn",
        options,
        Box::new(|cc| {
            ui_template::apply_light_theme(&cc.egui_ctx);
            let mut pane = receiver_pane::ReceiverPane::new();
            pane.start().map_err(
                |err: anyhow::Error| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("{err:#}").into()
                },
            )?;
            Ok(Box::new(LinuxApp { pane }))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
}

#[cfg(not(windows))]
struct LinuxApp {
    pane: receiver_pane::ReceiverPane,
}

#[cfg(not(windows))]
impl eframe::App for LinuxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pane.poll(&ctx);
        let is_streaming = self.pane.is_streaming();
        egui::CentralPanel::default()
            .frame(if is_streaming {
                egui::Frame::new().fill(egui::Color32::BLACK)
            } else {
                egui::Frame::new().fill(ui_template::BG_CANVAS).inner_margin(egui::Margin::same(28))
            })
            .show(ui, |ui| {
                self.pane.ui(ui, &ctx);
            });
        ctx.request_repaint_after(Duration::from_millis(if is_streaming { 16 } else { 200 }));
    }
}

// ---------------------------------------------------------------------------
// GUI
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn run_gui() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([780.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EcConn",
        options,
        Box::new(|cc| {
            ui_template::apply_light_theme(&cc.egui_ctx);
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Welcome,
    Host,
    Receiver,
}

#[cfg(windows)]
struct HostApp {
    mode: AppMode,
    discovery: Option<app_host::Discovery>,
    receiver_pane: receiver_pane::ReceiverPane,
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
    show_connection_info: bool,
    show_settings: bool,
    active_token: Option<u64>,
    active_device: Option<app_host::Device>,
}

#[cfg(windows)]
impl HostApp {
    fn new() -> anyhow::Result<Self> {
        let (store, store_path) = app_host::load_store();
        Ok(Self {
            mode: AppMode::Welcome,
            discovery: None,
            receiver_pane: receiver_pane::ReceiverPane::new(),
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
            show_connection_info: false,
            show_settings: false,
            active_token: None,
            active_device: None,
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
                                self.active_token = Some(outcome.token);
                                self.active_device = Some(device);
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

    fn restart_active_session(&mut self) {
        let (Some(token), Some(device)) = (self.active_token, self.active_device.clone()) else {
            return;
        };
        if let Some(mut session) = self.session.take() {
            let mut notes = Vec::new();
            session.stop(|line| notes.push(line.to_string()));
            for line in notes {
                self.push(line);
            }
        }
        let config = app_host::SessionConfig {
            dest: SocketAddr::new(device.addr, device.beacon.video_port),
            width: self.width,
            height: self.height,
            fps: self.fps,
            orientation: self.orientation,
            token,
            use_virtual: self.use_virtual,
            mirror_monitor: 0,
            encode_path: self.encode_path.clone(),
        };
        let mut notes = Vec::new();
        match app_host::HostSession::start(&config, |line| notes.push(line.to_string())) {
            Ok(session) => {
                for line in notes {
                    self.push(line);
                }
                self.session = Some(session);
            }
            Err(err) => {
                self.push(format!("could not apply runtime stream settings: {err:#}"));
            }
        }
    }

    fn stop_session(&mut self) {
        if let Some(device) = self.active_device.take() {
            if let Ok(sock) = std::net::UdpSocket::bind(("0.0.0.0", 0)) {
                for _ in 0..3 {
                    let _ = sock.send_to(b"ECCONN_BYE", device.video_addr());
                }
            }
        }
        self.active_token = None;
        if let Some(mut session) = self.session.take() {
            let mut notes = Vec::new();
            session.stop(|line| notes.push(line.to_string()));
            for line in notes {
                self.push(line);
            }
        }
        self.stats = None;
    }

    fn switch_mode(&mut self, target: AppMode, ctx: &egui::Context) {
        if self.mode == target {
            return;
        }
        match target {
            AppMode::Welcome => {
                self.stop_session();
                self.pairing = None;
                self.code = None;
                self.discovery = None;
                self.receiver_pane.stop(ctx);
                self.show_connection_info = false;
                self.show_settings = false;
            }
            AppMode::Host => {
                self.receiver_pane.stop(ctx);
                self.show_connection_info = false;
                if self.discovery.is_none() {
                    match app_host::Discovery::start() {
                        Ok(d) => self.discovery = Some(d),
                        Err(err) => self.push(format!("discovery restart failed: {err:#}")),
                    }
                }
            }
            AppMode::Receiver => {
                self.stop_session();
                self.pairing = None;
                self.code = None;
                self.discovery = None;
                self.show_connection_info = false;
                if let Err(err) = self.receiver_pane.start() {
                    self.push(format!("receiver start failed: {err:#}"));
                }
            }
        }
        self.mode = target;
    }
}

#[cfg(windows)]
impl eframe::App for HostApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        match self.mode {
            AppMode::Welcome => {}
            AppMode::Host => {
                self.poll_pairing();
                if self.scan_at.elapsed() >= Duration::from_millis(500) {
                    if let Some(discovery) = &self.discovery {
                        self.devices = discovery.snapshot();
                    }
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
                    self.stop_session();
                    self.push("stream ended");
                    self.switch_mode(AppMode::Welcome, &ctx);
                }
            }
            AppMode::Receiver => {
                self.receiver_pane.poll(&ctx);
                if self.receiver_pane.take_just_disconnected() {
                    self.switch_mode(AppMode::Welcome, &ctx);
                }
            }
        }

        let is_recv_streaming = self.mode == AppMode::Receiver && self.receiver_pane.is_streaming();
        if is_recv_streaming {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                .show(ui, |ui| {
                    self.receiver_pane.ui(ui, &ctx);
                });
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }

        let mut switch_to: Option<AppMode> = None;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ui_template::BG_CANVAS).inner_margin(egui::Margin::same(28)))
            .show(ui, |ui| {
                if self.mode == AppMode::Receiver {
                    ui.horizontal(|ui| {
                        if ui.add(ui_template::outline_pill_button("← Back", egui::vec2(88.0, 32.0))).clicked() {
                            switch_to = Some(AppMode::Welcome);
                        }
                    });
                    ui.add_space(6.0);
                    self.receiver_pane.ui(ui, &ctx);
                } else {
                    ui.columns(2, |cols| {
                        // ── Left Column: UI Controls ───────────────────────
                        cols[0].vertical(|ui| {
                            match self.mode {
                                AppMode::Welcome => {
                                    ui.add_space(12.0);
                                    ui_template::draw_ecconn_header(ui, "EcConn");
                                    ui.add_space(16.0);
                                    ui.label(
                                        egui::RichText::new("Connect and extend\nanything easily.")
                                            .size(20.0)
                                            .strong()
                                            .color(ui_template::NAVY),
                                    );
                                    ui.add_space(28.0);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(ui_template::primary_pill_button("Client", egui::vec2(138.0, 44.0)))
                                            .clicked()
                                        {
                                            switch_to = Some(AppMode::Receiver);
                                        }
                                        ui.add_space(12.0);
                                        if ui
                                            .add(ui_template::outline_pill_button("Host", egui::vec2(138.0, 44.0)))
                                            .clicked()
                                        {
                                            switch_to = Some(AppMode::Host);
                                        }
                                    });
                                    ui.add_space(36.0);
                                    ui_template::draw_wifi_banner(ui);
                                }
                                AppMode::Host if self.session.is_some() => {
                                    let session_mode = self.session.as_ref().unwrap().mode;
                                    ui_template::draw_ecconn_header(ui, "Host");
                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new("Connected Successfully")
                                            .size(20.0)
                                            .strong()
                                            .color(ui_template::NAVY),
                                    );
                                    ui.add_space(4.0);
                                    let info_lbl = if self.show_connection_info {
                                        "Hide Connection Info"
                                    } else {
                                        "Connection Info →"
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(info_lbl)
                                                    .size(13.5)
                                                    .strong()
                                                    .color(ui_template::BRAND_BLUE),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        self.show_connection_info = !self.show_connection_info;
                                    }

                                    let mut settings_changed = false;
                                    ui.add_space(12.0);
                                    ui_template::soft_blue_card().show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(
                                            egui::RichText::new("Stream settings")
                                                .size(16.0)
                                                .strong()
                                                .color(ui_template::NAVY),
                                        );
                                        ui.add_space(2.0);
                                        let peer_name = self
                                            .selected_device()
                                            .map(|d| d.beacon.name.clone())
                                            .unwrap_or_else(|| "Client Device".to_string());
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Connected to {peer_name} · {}",
                                                if session_mode == "virtual" { "Virtual" } else { "Mirror" }
                                            ))
                                            .size(12.5)
                                            .color(ui_template::SLATE),
                                        );
                                        ui.add_space(10.0);

                                        // FPS runtime selector
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("FPS").size(12.5).strong().color(ui_template::NAVY));
                                            ui.add_space(8.0);
                                            for preset in [30u32, 60, 90, 120] {
                                                let active = self.fps == preset;
                                                let label = format!("{preset}");
                                                let btn = if active {
                                                    ui_template::primary_pill_button(&label, egui::vec2(46.0, 28.0))
                                                } else {
                                                    ui_template::outline_pill_button(&label, egui::vec2(46.0, 28.0))
                                                };
                                                if ui.add(btn).clicked() && !active {
                                                    self.fps = preset;
                                                    settings_changed = true;
                                                }
                                            }
                                        });
                                        ui.add_space(6.0);

                                        // Orientation runtime selector
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("Orient").size(12.5).strong().color(ui_template::NAVY));
                                            ui.add_space(6.0);
                                            for deg in [0u16, 90, 180, 270] {
                                                let active = self.orientation == deg;
                                                let label = format!("{deg}°");
                                                let btn = if active {
                                                    ui_template::primary_pill_button(&label, egui::vec2(46.0, 28.0))
                                                } else {
                                                    ui_template::outline_pill_button(&label, egui::vec2(46.0, 28.0))
                                                };
                                                if ui.add(btn).clicked() && !active {
                                                    self.orientation = deg;
                                                    settings_changed = true;
                                                }
                                            }
                                        });
                                        ui.add_space(6.0);

                                        // Resolution runtime selector
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("Res").size(12.5).strong().color(ui_template::NAVY));
                                            ui.add_space(8.0);
                                            for (rw, rh, lbl) in [
                                                (1280u32, 720u32, "720p"),
                                                (1600u32, 900u32, "900p"),
                                                (1920u32, 1080u32, "1080p"),
                                            ] {
                                                let active = self.width == rw && self.height == rh;
                                                let btn = if active {
                                                    ui_template::primary_pill_button(lbl, egui::vec2(58.0, 28.0))
                                                } else {
                                                    ui_template::outline_pill_button(lbl, egui::vec2(58.0, 28.0))
                                                };
                                                if ui.add(btn).clicked() && !active {
                                                    self.width = rw;
                                                    self.height = rh;
                                                    settings_changed = true;
                                                }
                                            }
                                        });
                                    });

                                    if self.show_connection_info {
                                        let session = self.session.as_ref().unwrap();
                                        ui.add_space(10.0);
                                        ui_template::white_tile_card().show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            let target = self
                                                .selected_device()
                                                .map(|d| d.video_addr().to_string())
                                                .unwrap_or_default();
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Target: {target}  ·  {}×{}  ·  {}",
                                                    self.width, self.height, self.encode_path
                                                ))
                                                .size(12.0)
                                                .color(ui_template::SLATE),
                                            );
                                            if let Some((frames, kbit)) = self.stats {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{frames} frames · {kbit:.0} kbps"
                                                    ))
                                                    .size(12.0)
                                                    .strong()
                                                    .color(ui_template::BRAND_BLUE),
                                                );
                                            }
                                            egui::ScrollArea::vertical()
                                                .id_salt("session-log")
                                                .max_height(88.0)
                                                .stick_to_bottom(true)
                                                .show(ui, |ui| {
                                                    for line in session.recent(24) {
                                                        ui.monospace(
                                                            egui::RichText::new(line)
                                                                .size(10.5)
                                                                .color(ui_template::SLATE),
                                                        );
                                                    }
                                                });
                                        });
                                    }

                                    ui.add_space(18.0);
                                    if ui
                                        .add(ui_template::danger_pill_button("Disconnect", egui::vec2(168.0, 42.0)))
                                        .clicked()
                                    {
                                        self.stop_session();
                                        switch_to = Some(AppMode::Welcome);
                                    } else if settings_changed {
                                        self.restart_active_session();
                                    }
                                }
                                AppMode::Host if self.pairing.is_some() => {
                                    ui_template::draw_ecconn_header(ui, "EcConn");
                                    ui.add_space(16.0);
                                    ui.label(
                                        egui::RichText::new("Confirm connection")
                                            .size(21.0)
                                            .strong()
                                            .color(ui_template::NAVY),
                                    );
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Verify the secret code shown on the other device. The code must match on both devices.",
                                        )
                                        .size(13.0)
                                        .color(ui_template::SLATE),
                                    );
                                    ui.add_space(22.0);
                                    let code_str = self.code.as_deref().unwrap_or("··· ···");
                                    ui.label(
                                        egui::RichText::new(code_str)
                                            .size(34.0)
                                            .strong()
                                            .monospace()
                                            .color(ui_template::BRAND_BLUE),
                                    );
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new("Waiting for Allow on client device…")
                                                .size(12.5)
                                                .color(ui_template::SLATE),
                                        );
                                    });
                                    ui.add_space(22.0);
                                    if ui
                                        .add(ui_template::outline_pill_button("Cancel", egui::vec2(140.0, 40.0)))
                                        .clicked()
                                    {
                                        self.pairing = None;
                                        self.code = None;
                                    }
                                }
                                AppMode::Host => {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(ui_template::outline_pill_button("← Back", egui::vec2(84.0, 32.0)))
                                            .clicked()
                                        {
                                            switch_to = Some(AppMode::Welcome);
                                        }
                                        ui.add_space(8.0);
                                        ui_template::draw_ecconn_header(ui, "Host");
                                    });
                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new("Searching for Client Devices")
                                            .size(19.0)
                                            .strong()
                                            .color(ui_template::NAVY),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("Host Device : {}", self.host_name))
                                            .size(12.5)
                                            .color(ui_template::SLATE),
                                    );
                                    ui.add_space(14.0);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Available Devices")
                                                .size(15.0)
                                                .strong()
                                                .color(ui_template::NAVY),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui
                                                .add(ui_template::primary_pill_button("Refresh", egui::vec2(86.0, 30.0)))
                                                .clicked()
                                            {
                                                if let Some(discovery) = &self.discovery {
                                                    self.devices = discovery.snapshot();
                                                }
                                                self.scan_at = Instant::now();
                                            }
                                        });
                                    });
                                    ui.add_space(8.0);

                                    if self.devices.is_empty() {
                                        ui_template::soft_blue_card().show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.spinner();
                                                ui.label(
                                                    egui::RichText::new("Scanning LAN for EcConn clients…")
                                                        .size(13.0)
                                                        .color(ui_template::SLATE),
                                                );
                                            });
                                        });
                                    } else {
                                        let mut connect_idx: Option<usize> = None;
                                        egui::ScrollArea::vertical()
                                            .id_salt("devices")
                                            .max_height(180.0)
                                            .show(ui, |ui| {
                                                for (index, device) in self.devices.iter().enumerate() {
                                                    let is_sel = self.selected == Some(index);
                                                    egui::Frame::new()
                                                        .fill(if is_sel {
                                                            ui_template::CARD_BLUE
                                                        } else {
                                                            ui_template::CARD_WHITE
                                                        })
                                                        .stroke(egui::Stroke::new(
                                                            1.0,
                                                            if is_sel {
                                                                ui_template::BRAND_BLUE
                                                            } else {
                                                                ui_template::CARD_BORDER
                                                            },
                                                        ))
                                                        .corner_radius(egui::CornerRadius::same(12))
                                                        .inner_margin(egui::Margin::symmetric(12, 8))
                                                        .show(ui, |ui| {
                                                            ui.set_width(ui.available_width());
                                                            ui.horizontal(|ui| {
                                                                ui.vertical(|ui| {
                                                                    ui.label(
                                                                        egui::RichText::new(&device.beacon.name)
                                                                            .size(13.5)
                                                                            .strong()
                                                                            .color(ui_template::NAVY),
                                                                    );
                                                                    ui.label(
                                                                        egui::RichText::new(format!(
                                                                            "{} · {}×{} · {}",
                                                                            device.addr,
                                                                            device.beacon.width,
                                                                            device.beacon.height,
                                                                            device.beacon.platform
                                                                        ))
                                                                        .size(11.0)
                                                                        .color(ui_template::SLATE),
                                                                    );
                                                                });
                                                                ui.with_layout(
                                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                                    |ui| {
                                                                        if ui
                                                                            .add(ui_template::outline_pill_button(
                                                                                "connect",
                                                                                egui::vec2(82.0, 30.0),
                                                                            ))
                                                                            .clicked()
                                                                        {
                                                                            connect_idx = Some(index);
                                                                        }
                                                                    },
                                                                );
                                                            });
                                                        });
                                                    ui.add_space(6.0);
                                                }
                                            });
                                        if let Some(idx) = connect_idx {
                                            self.selected = Some(idx);
                                            if let Some(d) = self.devices.get(idx) {
                                                self.width = d.beacon.width.max(320);
                                                self.height = d.beacon.height.max(240);
                                                self.orientation = d.beacon.orientation;
                                            }
                                            self.start_pairing();
                                        }
                                    }

                                    ui.add_space(10.0);
                                    ui.horizontal(|ui| {
                                        let toggle_lbl = if self.show_settings {
                                            "▾ Stream settings & tools"
                                        } else {
                                            "▸ Stream settings & tools"
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(toggle_lbl)
                                                        .size(12.5)
                                                        .strong()
                                                        .color(ui_template::BRAND_BLUE),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                        {
                                            self.show_settings = !self.show_settings;
                                        }
                                    });

                                    if self.show_settings {
                                        ui.add_space(6.0);
                                        ui_template::soft_blue_card().show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.checkbox(&mut self.use_virtual, "Virtual Extend");
                                                ui.add_space(8.0);
                                                ui.label("FPS");
                                                ui.add(egui::DragValue::new(&mut self.fps).range(5..=120));
                                            });
                                            ui.horizontal(|ui| {
                                                ui.label("Resolution");
                                                ui.add(egui::DragValue::new(&mut self.width).range(320..=3840));
                                                ui.label("×");
                                                ui.add(egui::DragValue::new(&mut self.height).range(240..=2160));
                                            });
                                            ui.horizontal(|ui| {
                                                ui.label("Rotate");
                                                for value in [0u16, 90, 180, 270] {
                                                    ui.selectable_value(&mut self.orientation, value, format!("{value}°"));
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                ui.label("Encoder");
                                                for value in ["auto", "libx264", "nvenc", "qsv", "amf"] {
                                                    ui.selectable_value(&mut self.encode_path, value.to_string(), value);
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                if ui.button("Firewall helper").clicked() {
                                                    self.firewall_output = Some(match app_host::add_firewall_rule() {
                                                        Ok(text) => text,
                                                        Err(err) => format!("{err:#}"),
                                                    });
                                                }
                                                if let Some(out) = &self.firewall_output {
                                                    ui.label(egui::RichText::new(out).size(10.5).color(ui_template::SLATE));
                                                }
                                            });
                                        });
                                    }

                                    ui.add_space(12.0);
                                    ui_template::draw_wifi_banner(ui);
                                }
                                AppMode::Receiver => unreachable!(),
                            }
                        });

                        // ── Right Column: EcConn Hero Illustration ─────────
                        cols[1].vertical_centered(|ui| {
                            let show_wifi = !(self.mode == AppMode::Host && self.session.is_some());
                            ui_template::draw_right_illustration(ui, show_wifi);
                        });
                    });
                }
            });

        if let Some(target) = switch_to {
            self.switch_mode(target, &ctx);
        }

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
