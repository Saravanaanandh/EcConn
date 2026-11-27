//! Shared EcConn visual theme and custom vector widgets matching `UI_template`.

use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, Rect, Stroke, Vec2};

pub const BG_CANVAS: Color32 = Color32::from_rgb(245, 249, 253); // #F5F9FD
pub const CARD_WHITE: Color32 = Color32::WHITE;
pub const CARD_BLUE: Color32 = Color32::from_rgb(235, 243, 252); // #EBF3FC
pub const CARD_BORDER: Color32 = Color32::from_rgb(214, 231, 249); // #D6E7F9
pub const SKY_CIRCLE: Color32 = Color32::from_rgb(203, 228, 250); // #CBE4FA
pub const WOOD_DESK: Color32 = Color32::from_rgb(232, 204, 178); // #E8CCB2

pub const BRAND_BLUE: Color32 = Color32::from_rgb(21, 101, 192); // #1565C0
pub const BRAND_BLUE_LIGHT: Color32 = Color32::from_rgb(33, 150, 243);
pub const NAVY: Color32 = Color32::from_rgb(13, 37, 63); // #0D253F
pub const SLATE: Color32 = Color32::from_rgb(84, 110, 122); // #546E7A
pub const MUTED: Color32 = Color32::from_rgb(144, 164, 174); // #90A4AE
pub const DANGER_RED: Color32 = Color32::from_rgb(220, 76, 76); // #DC4C4C
pub const DANGER_BG: Color32 = Color32::from_rgb(255, 247, 247);

pub fn apply_light_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BG_CANVAS;
    visuals.window_fill = CARD_WHITE;
    visuals.extreme_bg_color = CARD_BLUE;
    visuals.faint_bg_color = CARD_BLUE;
    visuals.widgets.noninteractive.bg_fill = CARD_WHITE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, NAVY);
    visuals.widgets.inactive.bg_fill = CARD_WHITE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, CARD_BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, NAVY);
    visuals.widgets.hovered.bg_fill = CARD_BLUE;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.2, BRAND_BLUE);
    visuals.widgets.active.bg_fill = BRAND_BLUE;
    visuals.selection.bg_fill = CARD_BLUE;
    visuals.selection.stroke = Stroke::new(1.0, BRAND_BLUE);
    ctx.set_visuals(visuals);
}

pub fn soft_blue_card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD_BLUE)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(16))
}

pub fn white_tile_card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD_WHITE)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 10))
}

pub fn primary_pill_button(text: &str, min_size: Vec2) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .size(14.5)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(BRAND_BLUE)
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(22))
    .min_size(min_size)
}

pub fn outline_pill_button(text: &str, min_size: Vec2) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .size(14.5)
            .strong()
            .color(BRAND_BLUE),
    )
    .fill(CARD_WHITE)
    .stroke(Stroke::new(1.4, BRAND_BLUE))
    .corner_radius(CornerRadius::same(22))
    .min_size(min_size)
}

pub fn danger_pill_button(text: &str, min_size: Vec2) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .size(14.0)
            .strong()
            .color(DANGER_RED),
    )
    .fill(DANGER_BG)
    .stroke(Stroke::new(1.2, DANGER_RED))
    .corner_radius(CornerRadius::same(20))
    .min_size(min_size)
}

pub fn draw_wifi_icon(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    painter.circle_filled(center + egui::vec2(0.0, size * 0.30), size * 0.10, color);
    for (r_frac, deg) in [(0.34, 42.0_f32), (0.58, 46.0_f32), (0.82, 50.0_f32)] {
        let r = size * r_frac;
        let half = deg.to_radians();
        let mut pts = Vec::with_capacity(17);
        for i in 0..=16 {
            let t = i as f32 / 16.0;
            let a = -std::f32::consts::FRAC_PI_2 - half + t * 2.0 * half;
            pts.push(center + egui::vec2(0.0, size * 0.30) + egui::vec2(a.cos() * r, a.sin() * r));
        }
        painter.add(egui::Shape::line(pts, Stroke::new(size * 0.11, color)));
    }
}

const LOGO_RGBA: &[u8] = include_bytes!("../assets/ecconn_logo.rgba");
const HERO_RGBA: &[u8] = include_bytes!("../assets/ecconn_hero.rgba");

fn cached_texture(
    ctx: &egui::Context,
    key: &str,
    size: [usize; 2],
    rgba: &[u8],
) -> egui::TextureHandle {
    let id = egui::Id::new(key);
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let img = egui::ColorImage::from_rgba_unmultiplied(size, rgba);
    let tex = ctx.load_texture(key, img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

pub fn draw_ecconn_header(ui: &mut egui::Ui, title: &str) {
    let logo = cached_texture(ui.ctx(), "ecconn_logo_tex", [128, 128], LOGO_RGBA);
    ui.horizontal(|ui| {
        ui.add(
            egui::Image::new(&logo)
                .fit_to_exact_size(egui::vec2(46.0, 46.0))
                .corner_radius(CornerRadius::same(10)),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(title)
                .size(32.0)
                .strong()
                .color(NAVY),
        );
    });
}

pub fn draw_wifi_banner(ui: &mut egui::Ui) {
    soft_blue_card().show(ui, |ui| {
        ui.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
            draw_wifi_icon(ui.painter(), icon_rect.center(), 26.0, BRAND_BLUE);
            ui.add_space(6.0);
            let (div_rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 34.0), egui::Sense::hover());
            ui.painter().line_segment(
                [div_rect.center_top(), div_rect.center_bottom()],
                Stroke::new(1.0, Color32::from_rgb(176, 202, 230)),
            );
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Check your Wi-Fi connection")
                        .size(14.0)
                        .strong()
                        .color(NAVY),
                );
                ui.label(
                    egui::RichText::new("Both devices must use the same Wi-Fi.")
                        .size(12.0)
                        .color(SLATE),
                );
            });
        });
    });
}

/// Renders the generated EcConn hero illustration (`ecconn_hero.rgba`).
pub fn draw_right_illustration(ui: &mut egui::Ui, _show_all_devices: bool) {
    let hero = cached_texture(ui.ctx(), "ecconn_hero_tex", [480, 360], HERO_RGBA);
    let avail = ui.available_size();
    let max_w = avail.x.clamp(220.0, 440.0);
    let max_h = (max_w * 0.75).min(avail.y.max(220.0));
    let w = max_h / 0.75;
    ui.add_space(((avail.y - max_h) * 0.45).max(8.0));
    ui.add(
        egui::Image::new(&hero)
            .fit_to_exact_size(egui::vec2(w, max_h))
            .corner_radius(CornerRadius::same(18)),
    );
}
