//! Receiver (Client) mode pane for the unified EcConn Desktop application.
//! Cross-platform (Windows & Linux): advertises this desktop on the LAN,
//! handles pairing prompts, decodes incoming H.264 streams via `app_receiver`,
//! and forwards mouse/keyboard input back to the host.

use std::net::IpAddr;
use std::time::Instant;

use anyhow::Result;
use app_receiver::{
    yuv_to_rgba, Advertiser, PairEvent, PairingService, VideoSession,
};
use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};

use crate::ui_template;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Fit,
    Fill,
}

pub enum Stage {
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

pub struct ReceiverPane {
    advertiser: Option<Advertiser>,
    pairing: Option<PairingService>,
    pub stage: Stage,
    session: Option<VideoSession>,
    texture: Option<egui::TextureHandle>,
    fit: Fit,
    rotate: u16,
    control: bool,
    show_menu: bool,
    fullscreen: bool,
    pub log: Vec<String>,
    beacon_text: String,
    started_at: Instant,
    last_frame_at: Option<Instant>,
    frame_total: u64,
    button_mask: u8,
    just_disconnected: bool,
}

impl ReceiverPane {
    pub fn new() -> Self {
        Self {
            advertiser: None,
            pairing: None,
            stage: Stage::Waiting,
            session: None,
            texture: None,
            fit: Fit::Fit,
            rotate: 0,
            control: true,
            show_menu: false,
            fullscreen: false,
            log: Vec::new(),
            beacon_text: String::new(),
            started_at: Instant::now(),
            last_frame_at: None,
            frame_total: 0,
            button_mask: 0,
            just_disconnected: false,
        }
    }

    pub fn take_just_disconnected(&mut self) -> bool {
        let v = self.just_disconnected;
        self.just_disconnected = false;
        v
    }

    pub fn is_active(&self) -> bool {
        self.advertiser.is_some()
    }

    pub fn is_streaming(&self) -> bool {
        matches!(self.stage, Stage::Streaming { .. })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.advertiser.is_some() {
            return Ok(());
        }
        let beacon = app_receiver::make_beacon();
        let advertiser = Advertiser::start(beacon.clone())?;
        let pairing = PairingService::start(app_receiver::store_path())?;
        for note in advertiser.notes() {
            self.push(note.clone());
        }
        self.push(format!(
            "Receiver active: '{}' ({}x{}, {}:{})",
            beacon.name, beacon.width, beacon.height, beacon.id, beacon.video_port
        ));
        self.beacon_text = format!(
            "{}x{} · {} · id {} · video :{} · pair :{}",
            beacon.width, beacon.height, beacon.platform, beacon.id, beacon.video_port, beacon.pair_port
        );
        self.advertiser = Some(advertiser);
        self.pairing = Some(pairing);
        self.stage = Stage::Waiting;
        Ok(())
    }

    pub fn stop(&mut self, ctx: &egui::Context) {
        self.disconnect(ctx, "switched mode");
        self.advertiser = None;
        self.pairing = None;
    }

    fn push(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 300 {
            self.log.remove(0);
        }
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, value: bool) {
        if self.fullscreen != value {
            self.fullscreen = value;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(value));
        }
    }

    pub fn disconnect(&mut self, ctx: &egui::Context, why: &str) {
        if let Stage::Streaming { host_ip, .. } = self.stage {
            if let Ok(sock) = std::net::UdpSocket::bind(("0.0.0.0", 0)) {
                let _ = sock.send_to(b"ECCONN_BYE", (host_ip, 9000));
            }
            self.just_disconnected = true;
        }
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
        self.show_menu = false;
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        if let Some(pairing) = &self.pairing {
            while let Some(event) = pairing.try_recv() {
                events.push(event);
            }
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
                        self.push(format!("denied {host_name}: another host is already streaming"));
                        if let Some(p) = &self.pairing {
                            p.decide(false);
                        }
                        continue;
                    }
                    self.push(format!(
                        "pairing prompt from {host_name} (code {code}{})",
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
                        "stream starting from {host_name} ({host_id}) at {host_ip} (key {} bytes)",
                        key.len()
                    ));
                    if self.session.is_some() {
                        self.disconnect(ctx, "a second host was allowed");
                    }
                    let beacon = app_receiver::make_beacon();
                    match VideoSession::start(
                        host_ip,
                        beacon.width,
                        beacon.height,
                        token,
                        self.control,
                    ) {
                        Ok(session) => {
                            self.session = Some(session);
                            self.stage = Stage::Streaming { host_name, host_ip };
                            self.started_at = Instant::now();
                            self.last_frame_at = None;
                            self.frame_total = 0;
                            self.show_menu = false;
                            self.just_disconnected = false;
                            self.set_fullscreen(ctx, true);
                        }
                        Err(err) => {
                            self.push(format!("could not start receiver: {err:#}"));
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

        if let Some(session) = &self.session {
            if !session.is_running() {
                self.disconnect(ctx, "receiver stream ended");
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match &self.stage {
            Stage::Waiting => self.show_waiting(ui),
            Stage::Prompt { .. } => self.show_prompt(ui),
            Stage::Streaming { .. } => self.show_streaming(ui, ctx),
        }
    }

    fn show_waiting(&mut self, ui: &mut egui::Ui) {
        let dev_name = app_receiver::device_name();
        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                ui_template::draw_ecconn_header(ui, "Client");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Waiting for Host to Connect")
                        .size(18.0)
                        .color(ui_template::NAVY),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("Device Name:  {dev_name}"))
                        .size(14.5)
                        .strong()
                        .color(Color32::BLACK),
                );
                ui.add_space(38.0);
                ui.horizontal(|ui| {
                    ui.add_space(80.0);
                    ui.spinner();
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Loading....")
                            .size(18.0)
                            .strong()
                            .color(ui_template::NAVY),
                    );
                });
                ui.add_space(38.0);
                ui_template::draw_wifi_banner(ui);
                ui.add_space(10.0);
                ui.collapsing("Connection details & log", |ui| {
                    ui.small(&self.beacon_text);
                    if let Some(pairing) = &self.pairing {
                        let store = app_receiver::load_store(&pairing.store_path);
                        if !store.hosts.is_empty() {
                            ui.label(format!(
                                "Remembered hosts: {}",
                                app_receiver::paired_names(&store).join(", ")
                            ));
                        }
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("recv-log")
                        .max_height(100.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &self.log {
                                ui.monospace(line);
                            }
                        });
                });
            });
            ui_template::draw_right_illustration(&mut cols[1], true);
        });
    }

    fn show_prompt(&mut self, ui: &mut egui::Ui) {
        let (host_id, host_name, code, resume) = match &self.stage {
            Stage::Prompt {
                host_id,
                host_name,
                code,
                resume,
            } => (host_id.clone(), host_name.clone(), code.clone(), *resume),
            _ => return,
        };
        let mut decision = None;
        let formatted_code = if code.len() == 6 {
            format!("{} {}", &code[..3], &code[3..])
        } else {
            code.clone()
        };

        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                ui_template::draw_ecconn_header(ui, "EcConn");
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new("Confirm connection")
                        .size(28.0)
                        .strong()
                        .color(ui_template::NAVY),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Host “{host_name}” wants to connect. The code must match on both devices."
                    ))
                    .size(14.0)
                    .color(ui_template::SLATE),
                );
                ui.add_space(26.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(&formatted_code)
                            .size(34.0)
                            .strong()
                            .monospace()
                            .color(ui_template::NAVY),
                    );
                    let (line_rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 4.0), egui::Sense::hover());
                    ui.painter().line_segment(
                        [line_rect.left_center(), line_rect.right_center()],
                        Stroke::new(2.0, ui_template::BRAND_BLUE),
                    );
                });
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(ui_template::primary_pill_button(
                            if resume { "Confirm" } else { "Allow" },
                            egui::vec2(150.0, 42.0),
                        ))
                        .clicked()
                    {
                        decision = Some(true);
                    }
                    ui.add_space(12.0);
                    if ui
                        .add(ui_template::outline_pill_button("Cancel", egui::vec2(150.0, 42.0)))
                        .clicked()
                    {
                        decision = Some(false);
                    }
                });
                ui.add_space(10.0);
                ui.small(format!("Host ID: {host_id}"));
            });
            ui_template::draw_right_illustration(&mut cols[1], true);
        });

        if let Some(allow) = decision {
            if let Some(p) = &self.pairing {
                p.decide(allow);
            }
            self.stage = Stage::Waiting;
            self.push(if allow {
                "sent Allow; waiting for host stream"
            } else {
                "sent Cancel"
            });
        }
    }

    fn show_streaming(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available = ui.max_rect();
        let (host_name, host_ip) = match &self.stage {
            Stage::Streaming { host_name, host_ip } => (host_name.clone(), *host_ip),
            _ => return,
        };

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
                Color32::WHITE,
            );

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

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_menu = !self.show_menu;
        }
        let mut disconnect = false;
        egui::Area::new(egui::Id::new("ecconn_client_inapp_controls"))
            .anchor(egui::Align2::RIGHT_TOP, [-14.0, 14.0])
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let info_btn = if self.show_menu {
                            ui_template::primary_pill_button("Connection info", egui::vec2(138.0, 34.0))
                        } else {
                            ui_template::outline_pill_button("Connection info", egui::vec2(138.0, 34.0))
                        };
                        if ui.add(info_btn).clicked() {
                            self.show_menu = !self.show_menu;
                        }
                        ui.add_space(6.0);
                        if ui
                            .add(ui_template::danger_pill_button("Disconnect", egui::vec2(110.0, 34.0)))
                            .clicked()
                        {
                            disconnect = true;
                        }
                    });

                    if self.show_menu {
                        ui.add_space(8.0);
                        ui_template::soft_blue_card().show(ui, |ui| {
                            ui.set_max_width(300.0);
                            ui.label(
                                egui::RichText::new(format!("Streaming from {host_name} ({host_ip})"))
                                    .size(13.0)
                                    .strong()
                                    .color(ui_template::NAVY),
                            );
                            let stats = self.session.as_ref();
                            let frames = stats.map(|s| s.frames).unwrap_or(0);
                            let packets = stats.map(|s| s.packets()).unwrap_or(0);
                            let dropped = stats.map(|s| s.dropped()).unwrap_or(0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "decoded {frames} frames · {packets} packets · {dropped} dropped"
                                ))
                                .size(11.5)
                                .color(ui_template::SLATE),
                            );
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Fit");
                                ui.selectable_value(&mut self.fit, Fit::Fit, "Fit");
                                ui.selectable_value(&mut self.fit, Fit::Fill, "Fill");
                            });
                            ui.horizontal(|ui| {
                                ui.label("Rotate");
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
                        });
                    }
                });
            });
        if disconnect {
            self.disconnect(ctx, "user clicked Disconnect");
        }
    }
}
