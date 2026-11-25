//! `app-receiver`: wait for a host, allow it once, then show the stream.
//!
//! Run the GUI with no arguments. `--demo` runs the same advertise/pair/stream
//! code headlessly, which is how it is verified on loopback (see README).

use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;

use app_receiver::{
    Advertiser, PairEvent, PairingService, VideoSession, yuv_to_rgba,
};

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

// ---------------------------------------------------------------------------
// GUI
// ---------------------------------------------------------------------------

fn run_gui() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 640.0])
            .with_title("lowlat-screen receiver"),
        ..Default::default()
    };
    eframe::run_native(
        "lowlat-screen receiver",
        options,
        Box::new(|_cc| {
            let app = ReceiverApp::new().map_err(
                |err: anyhow::Error| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("{err:#}").into()
                },
            )?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    Fit,
    Fill,
}

enum Stage {
    Waiting,
    Prompt {
        host_id: String,
        host_name: String,
        code: String,
        resume: bool,
    },
    Streaming {
        host_name: String,
        host_ip: IpAddr,
    },
}

struct ReceiverApp {
    advertiser: Advertiser,
    pairing: PairingService,
    stage: Stage,
    session: Option<VideoSession>,
    texture: Option<egui::TextureHandle>,
    fit: Fit,
    rotate: u16,
    control: bool,
    show_menu: bool,
    fullscreen: bool,
    log: Vec<String>,
    beacon_text: String,
    started_at: Instant,
    last_frame_at: Option<Instant>,
    frame_total: u64,
    button_mask: u8,
}

impl ReceiverApp {
    fn new() -> Result<Self> {
        let beacon = app_receiver::make_beacon();
        let advertiser = Advertiser::start(beacon.clone())?;
        let pairing = PairingService::start(app_receiver::store_path())?;
        let mut log = Vec::new();
        for note in advertiser.notes() {
            log.push(note.clone());
        }
        log.push(format!(
            "advertising as '{}' ({}x{}, {}:{})",
            beacon.name, beacon.width, beacon.height, beacon.id, beacon.video_port
        ));
        let beacon_text = format!(
            "{}x{} · {} · id {} · video :{} · pair :{}",
            beacon.width, beacon.height, beacon.platform, beacon.id, beacon.video_port, beacon.pair_port
        );
        Ok(Self {
            advertiser,
            pairing,
            stage: Stage::Waiting,
            session: None,
            texture: None,
            fit: Fit::Fit,
            rotate: 0,
            control: false,
            show_menu: true,
            fullscreen: false,
            log,
            beacon_text,
            started_at: Instant::now(),
            last_frame_at: None,
            frame_total: 0,
            button_mask: 0,
        })
    }

    fn push(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 300 {
            self.log.remove(0);
        }
    }

    fn poll_pairing(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        while let Some(event) = self.pairing.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                PairEvent::Note(line) => self.push(line),
                PairEvent::Prompt {
                    host_id,
                    host_name,
                    code,
                    resume,
                } => {
                    if matches!(self.stage, Stage::Streaming { .. }) {
                        self.push(format!(
                            "denied {host_name}: another host is already streaming"
                        ));
                        self.pairing.decide(false);
                        continue;
                    }
                    self.push(format!(
                        "allow/deny dialog for {host_name} (code {code}{})",
                        if resume { ", previously paired" } else { "" }
                    ));
                    self.stage = Stage::Prompt {
                        host_id,
                        host_name,
                        code,
                        resume,
                    };
                }
                PairEvent::Allowed {
                    host_id,
                    host_name,
                    host_ip,
                    token,
                    key,
                } => {
                    self.push(format!(
                        "start stream from {host_name} ({host_id}) at {host_ip} (token {token:016x}, key {} bytes)",
                        key.len()
                    ));
                    if self.session.is_some() {
                        self.disconnect(ctx, "a second host was allowed");
                    }
                    match VideoSession::start(
                        host_ip,
                        app_receiver::make_beacon().width,
                        app_receiver::make_beacon().height,
                        token,
                        self.control,
                    ) {
                        Ok(session) => {
                            self.session = Some(session);
                            self.stage = Stage::Streaming {
                                host_name,
                                host_ip,
                            };
                            self.started_at = Instant::now();
                            self.last_frame_at = None;
                            self.frame_total = 0;
                            self.show_menu = true;
                            self.set_fullscreen(ctx, true);
                        }
                        Err(err) => {
                            self.push(format!("could not start the video receiver: {err:#}"));
                            self.stage = Stage::Waiting;
                        }
                    }
                }
                PairEvent::Denied { host_name, reason } => {
                    self.push(format!("denied {host_name}: {reason}"));
                    self.stage = Stage::Waiting;
                }
            }
        }
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, value: bool) {
        if self.fullscreen != value {
            self.fullscreen = value;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(value));
        }
    }

    fn disconnect(&mut self, ctx: &egui::Context, why: &str) {
        if let Some(mut session) = self.session.take() {
            session.release_all();
            session.stop();
            self.push(format!(
                "disconnected ({why}) after {} frames",
                session.frames
            ));
        }
        self.stage = Stage::Waiting;
        self.texture = None;
        self.set_fullscreen(ctx, false);
        self.show_menu = true;
    }

    fn show_waiting(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("Waiting for a device");
            ui.add_space(6.0);
            ui.label("The host app should list this device under \u{201c}Receivers on your LAN\u{201d}.");
            ui.label("Pairing asks for a 6-digit code before anything is streamed.");
            ui.add_space(12.0);
            ui.monospace(&self.beacon_text);
            if self.advertiser.notes().is_empty() {
                ui.label("Advertising over mDNS and a UDP broadcast beacon (port 9200).");
            } else {
                for note in self.advertiser.notes() {
                    ui.colored_label(egui::Color32::YELLOW, note);
                }
            }
            let store = app_receiver::load_store(&self.pairing.store_path);
            ui.add_space(16.0);
            ui.heading("Paired hosts");
            if store.hosts.is_empty() {
                ui.label("None yet. A host you allow will be remembered here.");
            } else {
                for name in app_receiver::paired_names(&store) {
                    ui.label(format!("• {name}"));
                }
            }
            ui.add_space(4.0);
            ui.small(format!("stored in {}", self.pairing.store_path.display()));
        });
    }

    fn show_prompt(&mut self, ui: &mut egui::Ui) {
        let (host_id, host_name, code, resume) = match &self.stage {
            Stage::Prompt {
                host_id,
                host_name,
                code,
                resume,
            } => (
                host_id.clone(),
                host_name.clone(),
                code.clone(),
                *resume,
            ),
            _ => return,
        };
        let mut decision = None;
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.heading(format!("Allow \u{201c}{host_name}\u{201d} to extend your screen?"));
            ui.add_space(8.0);
            ui.label("Check that this code matches the one shown on the host:");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&code)
                    .size(52.0)
                    .strong()
                    .monospace(),
            );
            ui.add_space(8.0);
            ui.label(if resume {
                "This host was allowed before. one click is enough."
            } else {
                "A new 32-byte session key and input token are created when you allow."
            });
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(if resume { "Confirm" } else { "Allow" }))
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui.button("Deny").clicked() {
                    decision = Some(false);
                }
            });
            ui.add_space(12.0);
            ui.small(format!("host id: {host_id}"));
        });
        match decision {
            Some(allow) => {
                self.pairing.decide(allow);
                self.stage = Stage::Waiting;
                self.push(if allow {
                    "sent Allow; waiting for the host's first frame"
                } else {
                    "sent Deny"
                });
            }
            None => {}
        }
    }

    fn show_streaming(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available = ui.max_rect();
        let (host_name, host_ip) = match &self.stage {
            Stage::Streaming {
                host_name,
                host_ip,
            } => (host_name.clone(), *host_ip),
            _ => return,
        };

        // Decode the newest frame into RGBA at the current rotation.
        let mut image: Option<(Vec<u8>, u32, u32)> = None;
        if let Some(session) = &mut self.session {
            if let Some((data, w, h)) = session.take_frame() {
                self.frame_total += 1;
                self.last_frame_at = Some(Instant::now());
                image = yuv_to_rgba(&data, w, h, self.rotate);
            }
        }
        if let Some((rgba, w, h)) = image {
            let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            match &mut self.texture {
                Some(texture) => texture.set(color, egui::TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("video", color, egui::TextureOptions::LINEAR));
                }
            }
        }

        if let Some(texture) = &self.texture {
            let (tw, th) = texture.size_vec2().into();
            let scale = match self.fit {
                Fit::Fit => (available.width() / tw).min(available.height() / th),
                Fit::Fill => (available.width() / tw).max(available.height() / th),
            };
            let size = egui::vec2(tw, th) * scale;
            let rect = egui::Rect::from_center_size(available.center(), size);
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            // Pointer control (off by default, so a loopback demo cannot move
            // the real host cursor by accident).
            let response = ui.interact(
                rect,
                ui.make_persistent_id("video"),
                egui::Sense::click_and_drag(),
            );
            if self.control && response.hovered() {
                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    if let Some(session) = &mut self.session {
                        let x = (pos.x - rect.left()) / rect.width();
                        let y = (pos.y - rect.top()) / rect.height();
                        let (x, y) = match self.rotate % 360 {
                            90 => (y, 1.0 - x),
                            180 => (1.0 - x, 1.0 - y),
                            270 => (1.0 - y, x),
                            _ => (x, y),
                        };
                        session.mouse_move(x, y);
                    }
                }
                let (mask, wheel) = ctx.input(|i| {
                    let mut mask = 0u8;
                    if i.pointer.button_down(egui::PointerButton::Primary) {
                        mask |= 1;
                    }
                    if i.pointer.button_down(egui::PointerButton::Secondary) {
                        mask |= 2;
                    }
                    if i.pointer.button_down(egui::PointerButton::Middle) {
                        mask |= 4;
                    }
                    (mask, i.smooth_scroll_delta.y.round() as i16)
                });
                if mask != self.button_mask {
                    self.button_mask = mask;
                    if let Some(session) = &mut self.session {
                        session.mouse_buttons(mask);
                    }
                }
                if wheel != 0 {
                    if let Some(session) = &mut self.session {
                        session.wheel(wheel);
                    }
                }
            }
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("Connecting…");
                ui.label("waiting for the first keyframe from the host");
            });
        }

        // Compact overlay menu (Esc toggles it).
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_menu = !self.show_menu;
        }
        let mut disconnect = false;
        if self.show_menu {
            egui::Window::new("receiver controls")
                .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(format!("streaming from {host_name} ({host_ip})"));
                    let stats = self.session.as_ref();
                    let frames = stats.map(|s| s.frames).unwrap_or(0);
                    let packets = stats.map(|s| s.packets()).unwrap_or(0);
                    let dropped = stats.map(|s| s.dropped()).unwrap_or(0);
                    ui.label(format!(
                        "decoded {frames} frames · {packets} packets · {dropped} dropped"
                    ));
                    ui.small(if self.last_frame_at.is_some() {
                        "latency: not measured by the GUI"
                    } else {
                        "no frame yet"
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("fit mode");
                        ui.selectable_value(&mut self.fit, Fit::Fit, "Fit");
                        ui.selectable_value(&mut self.fit, Fit::Fill, "Fill");
                    });
                    ui.horizontal(|ui| {
                        ui.label("rotate");
                        for value in [0u16, 90, 180, 270] {
                            ui.selectable_value(&mut self.rotate, value, format!("{value}°"));
                        }
                    });
                    ui.checkbox(&mut self.control, "Control host (paired token)");
                    ui.horizontal(|ui| {
                        if ui.button("Windowed").clicked() {
                            self.set_fullscreen(ctx, false);
                        }
                        if ui.button("Fullscreen").clicked() {
                            self.set_fullscreen(ctx, true);
                        }
                    });
                    ui.separator();
                    if ui.button("Disconnect").clicked() {
                        disconnect = true;
                    }
                });
        } else {
            ui.painter().text(
                egui::pos2(available.left() + 12.0, available.bottom() - 12.0),
                egui::Align2::LEFT_BOTTOM,
                "Esc: controls",
                egui::FontId::proportional(13.0),
                egui::Color32::from_white_alpha(120),
            );
        }
        if disconnect {
            self.disconnect(ctx, "user clicked Disconnect");
        }
    }
}

impl eframe::App for ReceiverApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_pairing(&ctx);

        // A dead receiver thread means the stream failed (e.g. ffmpeg gone).
        if let Some(session) = &self.session {
            if !session.is_running() {
                self.disconnect(&ctx, "receiver thread stopped");
            }
        }

        let black = egui::Frame::default()
            .fill(egui::Color32::BLACK)
            .inner_margin(0.0);
        egui::CentralPanel::default().frame(black).show(ui, |ui| {
            match &self.stage {
                Stage::Waiting => self.show_waiting(ui),
                Stage::Prompt { .. } => self.show_prompt(ui),
                Stage::Streaming { .. } => self.show_streaming(ui, &ctx),
            }
        });

        // Keep the UI ticking so new frames appear even without input events.
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

// ---------------------------------------------------------------------------
// Headless demo
// ---------------------------------------------------------------------------

fn run_demo(args: &[String]) -> Result<()> {
    let value = |flag: &str| -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let seconds: u64 = value("--seconds")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let auto_allow = args.iter().any(|arg| arg == "--auto-allow");
    let control = args.iter().any(|arg| arg == "--control");

    let beacon = app_receiver::make_beacon();
    println!(
        "[receiver] beacon id={} name='{}' {}x{} orientation={} video={} pair={}",
        beacon.id, beacon.name, beacon.width, beacon.height, beacon.orientation, beacon.video_port, beacon.pair_port
    );
    let advertiser = Advertiser::start(beacon.clone())?;
    for note in advertiser.notes() {
        println!("[receiver] note: {note}");
    }
    println!("[receiver] advertising (mDNS + UDP beacon every 2 s on :9200)");

    let store_path = app_receiver::store_path();
    let pairing = PairingService::start(store_path.clone())?;
    println!("[receiver] pairing listener on UDP :{}", beacon.pair_port);
    println!("[receiver] paired-host store: {}", store_path.display());

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut session: Option<VideoSession> = None;
    let mut shown = 0u64;
    let mut last_report = Instant::now();
    let mut prompted = false;

    while Instant::now() < deadline {
        while let Some(event) = pairing.try_recv() {
            match event {
                PairEvent::Note(line) => println!("[receiver] {line}"),
                PairEvent::Prompt {
                    host_id,
                    host_name,
                    code,
                    resume,
                } => {
                    println!(
                        "[receiver] PAIR REQUEST from '{}' ({}) code={} resume={}",
                        host_name, host_id, code, resume
                    );
                    println!("[receiver] dialog: Allow \"{host_name}\" to extend your screen? code {code}");
                    if auto_allow && !prompted {
                        prompted = true;
                        println!("[receiver] --auto-allow: answering Allow");
                        pairing.decide(true);
                    } else if !auto_allow {
                        println!("[receiver] (demo has no window; pass --auto-allow to answer)");
                    }
                }
                PairEvent::Allowed {
                    host_id,
                    host_name,
                    host_ip,
                    token,
                    key,
                } => {
                    println!(
                        "[receiver] ALLOWED '{}' ({}) from {} token={:016x} key={} bytes",
                        host_name,
                        host_id,
                        host_ip,
                        token,
                        key.len()
                    );
                    match VideoSession::start(host_ip, beacon.width, beacon.height, token, control) {
                        Ok(new_session) => {
                            println!("[receiver] video receiver bound to :{} (ffmpeg decoder)", app_receiver::VIDEO_PORT);
                            session = Some(new_session);
                        }
                        Err(err) => println!("[receiver] could not start video: {err:#}"),
                    }
                }
                PairEvent::Denied { host_name, reason } => {
                    println!("[receiver] DENIED '{}': {}", host_name, reason);
                }
            }
        }

        if let Some(current) = &mut session {
            if let Some((data, w, h)) = current.take_frame() {
                shown += 1;
                if shown == 1 {
                    println!(
                        "[receiver] first decoded frame: {} bytes, {}x{} (orientation={} deg, rotation={})",
                        data.len(),
                        w,
                        h,
                        current.orientation,
                        current.rotation
                    );
                }
            }
            if last_report.elapsed() >= Duration::from_secs(1) {
                println!(
                    "[receiver] status: decoded={} packets={} dropped={} running={}",
                    current.frames,
                    current.packets(),
                    current.dropped(),
                    current.is_running()
                );
                last_report = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    if let Some(mut current) = session.take() {
        println!("[receiver] stopping video after {} decoded frames", current.frames);
        current.stop();
    }
    drop(pairing);
    drop(advertiser);
    let store = app_receiver::load_store(&store_path);
    println!(
        "[receiver] store: {} paired host(s): {:?}",
        store.hosts.len(),
        app_receiver::paired_names(&store)
    );
    println!("[receiver] done");
    Ok(())
}
