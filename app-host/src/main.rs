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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 840.0])
            .with_min_inner_size([720.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EcConn",
        options,
        Box::new(|cc| {
            apply_obsidian_theme(&cc.egui_ctx);
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
mod theme {
    use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};

    pub const BG_OBSIDIAN: Color32 = Color32::from_rgb(17, 18, 25);
    pub const CARD_SURFACE: Color32 = Color32::from_rgb(27, 29, 40);
    pub const CARD_BORDER: Color32 = Color32::from_rgb(43, 46, 63);
    pub const INSET_WELL: Color32 = Color32::from_rgb(19, 20, 28);
    pub const INSET_BORDER: Color32 = Color32::from_rgb(35, 38, 53);

    pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
    pub const VIOLET_LIGHT: Color32 = Color32::from_rgb(167, 139, 250);
    pub const VIOLET_DIM: Color32 = Color32::from_rgb(59, 45, 102);
    pub const CYAN: Color32 = Color32::from_rgb(56, 189, 248);
    pub const MINT: Color32 = Color32::from_rgb(52, 211, 153);
    pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);
    pub const DANGER_BG: Color32 = Color32::from_rgb(59, 25, 34);
    pub const DANGER_FG: Color32 = Color32::from_rgb(248, 113, 113);

    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(243, 244, 246);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(142, 149, 168);

    pub fn card_frame() -> egui::Frame {
        egui::Frame::new()
            .fill(CARD_SURFACE)
            .stroke(Stroke::new(1.0, CARD_BORDER))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(Margin::same(16))
    }

    pub fn inset_frame() -> egui::Frame {
        egui::Frame::new()
            .fill(INSET_WELL)
            .stroke(Stroke::new(1.0, INSET_BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(10))
    }
}

#[cfg(windows)]
fn apply_obsidian_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = theme::BG_OBSIDIAN;
    visuals.window_fill = theme::CARD_SURFACE;
    visuals.extreme_bg_color = theme::INSET_WELL;
    visuals.faint_bg_color = theme::CARD_SURFACE;
    visuals.widgets.noninteractive.bg_fill = theme::CARD_SURFACE;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_SECONDARY);
    visuals.widgets.inactive.bg_fill = theme::INSET_WELL;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, theme::INSET_BORDER);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    visuals.widgets.hovered.bg_fill = theme::VIOLET_DIM;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, theme::VIOLET_LIGHT);
    visuals.widgets.active.bg_fill = theme::VIOLET;
    visuals.selection.bg_fill = theme::VIOLET_DIM;
    visuals.selection.stroke = egui::Stroke::new(1.0, theme::VIOLET_LIGHT);
    ctx.set_visuals(visuals);
}

#[cfg(windows)]
fn draw_status_dial(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    fraction: f32,
    accent: egui::Color32,
) {
    let size = egui::vec2(126.0, 126.0);
    let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = 52.0;

    // Inner recessed dial well
    painter.circle_filled(center, radius - 9.0, theme::INSET_WELL);
    painter.circle_stroke(center, radius - 9.0, egui::Stroke::new(1.0, theme::INSET_BORDER));

    // Outer track ring
    painter.circle_stroke(center, radius, egui::Stroke::new(6.0, egui::Color32::from_rgb(35, 38, 56)));

    // Sweeping arc
    let sweep = fraction.clamp(0.06, 1.0) * std::f32::consts::TAU;
    let steps = 40;
    let start_angle = -std::f32::consts::FRAC_PI_2;
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let a = start_angle + t * sweep;
        pts.push(center + egui::vec2(a.cos() * radius, a.sin() * radius));
    }
    painter.add(egui::Shape::line(
        pts.clone(),
        egui::Stroke::new(11.0, egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 45)),
    ));
    painter.add(egui::Shape::line(pts, egui::Stroke::new(5.5, accent)));

    // Knob dot at arc end
    let end_angle = start_angle + sweep;
    let knob = center + egui::vec2(end_angle.cos() * radius, end_angle.sin() * radius);
    painter.circle_filled(knob, 5.0, egui::Color32::WHITE);
    painter.circle_filled(knob, 2.5, theme::VIOLET);

    painter.text(
        center + egui::vec2(0.0, -5.0),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(19.0),
        theme::TEXT_PRIMARY,
    );
    painter.text(
        center + egui::vec2(0.0, 14.0),
        egui::Align2::CENTER_CENTER,
        subtitle,
        egui::FontId::proportional(10.5),
        theme::TEXT_SECONDARY,
    );
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

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG_OBSIDIAN).inner_margin(egui::Margin::same(20)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // ── Top Header ─────────────────────────────────────────
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(theme::VIOLET_LIGHT, "—");
                                ui.label(
                                    egui::RichText::new("ECCONN")
                                        .size(11.0)
                                        .strong()
                                        .color(theme::TEXT_SECONDARY),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("EcConn")
                                        .size(26.0)
                                        .strong()
                                        .color(theme::TEXT_PRIMARY),
                                );
                                ui.label(
                                    egui::RichText::new("Host")
                                        .size(26.0)
                                        .strong()
                                        .color(theme::VIOLET_LIGHT),
                                );
                            });
                            ui.label(
                                egui::RichText::new(format!(
                                    "Wireless Display Hub · {} ({})",
                                    self.host_name, self.host_id
                                ))
                                .size(12.5)
                                .color(theme::TEXT_SECONDARY),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (badge_text, badge_color) = if self.session.is_some() {
                                ("●  LIVE", theme::MINT)
                            } else if self.pairing.is_some() {
                                ("●  PAIRING", theme::AMBER)
                            } else if !self.devices.is_empty() {
                                ("●  READY", theme::CYAN)
                            } else {
                                ("●  SCANNING", theme::VIOLET_LIGHT)
                            };
                            egui::Frame::new()
                                .fill(theme::CARD_SURFACE)
                                .stroke(egui::Stroke::new(1.0, theme::CARD_BORDER))
                                .corner_radius(egui::CornerRadius::same(16))
                                .inner_margin(egui::Margin::symmetric(14, 8))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(badge_text)
                                            .size(12.0)
                                            .strong()
                                            .color(badge_color),
                                    );
                                });
                        });
                    });

                    ui.add_space(14.0);

                    // ── Hero Dial & Primary Control Card ───────────────────
                    theme::card_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dial_title, dial_sub, dial_frac, dial_accent) =
                                if let Some(session) = &self.session {
                                    match self.stats {
                                        Some((frames, kbit)) => (
                                            format!("{kbit:.0}k"),
                                            format!("{frames} frames · {}", session.mode),
                                            0.88,
                                            theme::CYAN,
                                        ),
                                        None => (
                                            "LIVE".to_string(),
                                            format!("{} mode", session.mode),
                                            0.80,
                                            theme::MINT,
                                        ),
                                    }
                                } else if let Some(code) = &self.code {
                                    (code.clone(), "Confirm on phone".to_string(), 0.92, theme::AMBER)
                                } else if self.pairing.is_some() {
                                    ("PAIR".to_string(), "Connecting…".to_string(), 0.65, theme::AMBER)
                                } else {
                                    (
                                        format!("{} FPS", self.fps),
                                        format!("{}×{}", self.width, self.height),
                                        if self.devices.is_empty() { 0.45 } else { 0.72 },
                                        theme::VIOLET_LIGHT,
                                    )
                                };

                            draw_status_dial(ui, &dial_title, &dial_sub, dial_frac, dial_accent);
                            ui.add_space(16.0);

                            ui.vertical(|ui| {
                                if let Some(session) = &self.session {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Streaming ({})",
                                            session.mode.to_uppercase()
                                        ))
                                        .size(16.0)
                                        .strong()
                                        .color(theme::TEXT_PRIMARY),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Target: {}",
                                            self.selected_device()
                                                .map(|d| d.video_addr().to_string())
                                                .unwrap_or_default()
                                        ))
                                        .size(12.5)
                                        .color(theme::CYAN),
                                    );
                                } else if let Some(code) = &self.code {
                                    ui.label(
                                        egui::RichText::new(format!("Pairing Code: {code}"))
                                            .size(18.0)
                                            .strong()
                                            .color(theme::VIOLET_LIGHT),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "Verify this code matches your phone, then tap Allow.",
                                        )
                                        .size(12.5)
                                        .color(theme::TEXT_SECONDARY),
                                    );
                                } else if let Some(dev) = self.selected_device() {
                                    ui.label(
                                        egui::RichText::new(format!("Target: {}", dev.beacon.name))
                                            .size(16.0)
                                            .strong()
                                            .color(theme::TEXT_PRIMARY),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {}×{} · {} mode",
                                            dev.addr,
                                            self.width,
                                            self.height,
                                            if self.use_virtual { "Virtual Extend" } else { "Mirror" }
                                        ))
                                        .size(12.5)
                                        .color(theme::TEXT_SECONDARY),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("Waiting for EcConn Receiver")
                                            .size(16.0)
                                            .strong()
                                            .color(theme::TEXT_PRIMARY),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "Open EcConn on your phone on the same Wi-Fi network.",
                                        )
                                        .size(12.5)
                                        .color(theme::TEXT_SECONDARY),
                                    );
                                }

                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    let can_pair = self.selected.is_some()
                                        && self.session.is_none()
                                        && self.pairing.is_none();
                                    let pair_btn = egui::Button::new(
                                        egui::RichText::new("  Pair & Connect  ")
                                            .size(13.5)
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(if can_pair { theme::VIOLET } else { theme::INSET_WELL })
                                    .corner_radius(egui::CornerRadius::same(14))
                                    .min_size(egui::vec2(140.0, 36.0));
                                    if ui.add_enabled(can_pair, pair_btn).clicked() {
                                        self.start_pairing();
                                    }

                                    let can_stop = self.session.is_some();
                                    let stop_btn = egui::Button::new(
                                        egui::RichText::new("  Stop Stream  ")
                                            .size(13.5)
                                            .strong()
                                            .color(if can_stop {
                                                theme::DANGER_FG
                                            } else {
                                                theme::TEXT_SECONDARY
                                            }),
                                    )
                                    .fill(if can_stop { theme::DANGER_BG } else { theme::INSET_WELL })
                                    .corner_radius(egui::CornerRadius::same(14))
                                    .min_size(egui::vec2(120.0, 36.0));
                                    if ui.add_enabled(can_stop, stop_btn).clicked() {
                                        self.stop_session();
                                    }

                                    if self.pairing.is_some() {
                                        ui.spinner();
                                        ui.colored_label(theme::AMBER, "Awaiting Allow…");
                                    }
                                });
                            });
                        });
                    });

                    ui.add_space(14.0);

                    // ── 2-Column Grid: Receivers & Stream Settings ─────────
                    ui.columns(2, |cols| {
                        // Left Column: Receivers on LAN
                        theme::card_frame().show(&mut cols[0], |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Receivers on LAN")
                                        .size(15.0)
                                        .strong()
                                        .color(theme::TEXT_PRIMARY),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{} found", self.devices.len()))
                                            .size(11.5)
                                            .strong()
                                            .color(theme::CYAN),
                                    );
                                });
                            });
                            ui.add_space(8.0);

                            if self.devices.is_empty() {
                                theme::inset_frame().show(ui, |ui| {
                                    ui.set_min_height(155.0);
                                    ui.vertical_centered(|ui| {
                                        ui.add_space(55.0);
                                        ui.colored_label(
                                            theme::TEXT_SECONDARY,
                                            "Scanning LAN via mDNS & UDP…",
                                        );
                                    });
                                });
                            } else {
                                egui::ScrollArea::vertical()
                                    .id_salt("devices")
                                    .min_scrolled_height(165.0)
                                    .max_height(175.0)
                                    .show(ui, |ui| {
                                        for (index, device) in self.devices.iter().enumerate() {
                                            let is_sel = self.selected == Some(index);
                                            let row_frame = egui::Frame::new()
                                                .fill(if is_sel {
                                                    theme::VIOLET_DIM
                                                } else {
                                                    theme::INSET_WELL
                                                })
                                                .stroke(egui::Stroke::new(
                                                    1.0,
                                                    if is_sel {
                                                        theme::VIOLET_LIGHT
                                                    } else {
                                                        theme::INSET_BORDER
                                                    },
                                                ))
                                                .corner_radius(egui::CornerRadius::same(12))
                                                .inner_margin(egui::Margin::same(10));

                                            let resp = row_frame
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.horizontal(|ui| {
                                                        ui.vertical(|ui| {
                                                            ui.label(
                                                                egui::RichText::new(&device.beacon.name)
                                                                    .size(13.5)
                                                                    .strong()
                                                                    .color(theme::TEXT_PRIMARY),
                                                            );
                                                            ui.label(
                                                                egui::RichText::new(format!(
                                                                    "{} · {}×{} · @{} [{}]",
                                                                    device.addr,
                                                                    device.beacon.width,
                                                                    device.beacon.height,
                                                                    device.beacon.platform,
                                                                    device.source
                                                                ))
                                                                .size(11.0)
                                                                .color(theme::TEXT_SECONDARY),
                                                            );
                                                        });
                                                    });
                                                })
                                                .response
                                                .interact(egui::Sense::click());

                                            if resp.clicked() {
                                                self.selected = Some(index);
                                                if let Some(d) = self.devices.get(index) {
                                                    self.width = d.beacon.width.max(320);
                                                    self.height = d.beacon.height.max(240);
                                                    self.orientation = d.beacon.orientation;
                                                }
                                            }
                                            ui.add_space(6.0);
                                        }
                                    });
                            }

                            for note in self.discovery.notes() {
                                ui.colored_label(theme::AMBER, note);
                            }
                        });

                        // Right Column: Display & Stream Settings
                        theme::card_frame().show(&mut cols[1], |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Display & Stream")
                                        .size(15.0)
                                        .strong()
                                        .color(theme::TEXT_PRIMARY),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.checkbox(&mut self.use_virtual, "Virtual Extend");
                                });
                            });
                            ui.add_space(8.0);

                            theme::inset_frame().show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Resolution").color(theme::TEXT_SECONDARY));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.add(egui::DragValue::new(&mut self.height).range(240..=2160));
                                        ui.label("×");
                                        ui.add(egui::DragValue::new(&mut self.width).range(320..=3840));
                                    });
                                });
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("FPS").color(theme::TEXT_SECONDARY));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.add(egui::DragValue::new(&mut self.fps).range(5..=120));
                                        for preset in [60u32, 30] {
                                            let active = self.fps == preset;
                                            let b = egui::Button::new(format!("{preset}"))
                                                .fill(if active { theme::VIOLET } else { theme::CARD_SURFACE })
                                                .corner_radius(egui::CornerRadius::same(8));
                                            if ui.add(b).clicked() {
                                                self.fps = preset;
                                            }
                                        }
                                    });
                                });
                            });

                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Orientation")
                                    .size(11.5)
                                    .color(theme::TEXT_SECONDARY),
                            );
                            ui.horizontal(|ui| {
                                for value in [0u16, 90, 180, 270] {
                                    let active = self.orientation == value;
                                    let btn = egui::Button::new(
                                        egui::RichText::new(format!("{value}°"))
                                            .size(12.0)
                                            .color(if active { egui::Color32::WHITE } else { theme::TEXT_SECONDARY }),
                                    )
                                    .fill(if active { theme::VIOLET } else { theme::INSET_WELL })
                                    .corner_radius(egui::CornerRadius::same(10));
                                    if ui.add(btn).clicked() {
                                        self.orientation = value;
                                    }
                                }
                            });

                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Encoder")
                                    .size(11.5)
                                    .color(theme::TEXT_SECONDARY),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for value in ["auto", "libx264", "nvenc", "qsv", "amf"] {
                                    let active = self.encode_path == value;
                                    let btn = egui::Button::new(
                                        egui::RichText::new(value)
                                            .size(11.5)
                                            .color(if active { egui::Color32::WHITE } else { theme::TEXT_SECONDARY }),
                                    )
                                    .fill(if active { theme::VIOLET } else { theme::INSET_WELL })
                                    .corner_radius(egui::CornerRadius::same(10));
                                    if ui.add(btn).clicked() {
                                        self.encode_path = value.to_string();
                                    }
                                }
                            });
                        });
                    });

                    ui.add_space(14.0);

                    // ── Session Output & Diagnostics Card ──────────────────
                    if let Some(session) = &self.session {
                        theme::card_frame().show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Live Stream Telemetry")
                                    .size(14.0)
                                    .strong()
                                    .color(theme::CYAN),
                            );
                            ui.add_space(6.0);
                            theme::inset_frame().show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("session-log")
                                    .max_height(110.0)
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        for line in session.recent(40) {
                                            ui.monospace(line);
                                        }
                                    });
                            });
                        });
                        ui.add_space(12.0);
                    }

                    theme::card_frame().show(ui, |ui| {
                        ui.collapsing("Windows Firewall Helper", |ui| {
                            ui.label("Open UDP ports 9000, 9200, 9201 for EcConn:");
                            ui.monospace(app_host::firewall_command());
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
                        ui.add_space(4.0);
                        ui.collapsing("Event Log", |ui| {
                            theme::inset_frame().show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("event-log")
                                    .max_height(130.0)
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        for line in &self.log {
                                            ui.monospace(line);
                                        }
                                    });
                            });
                        });
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
