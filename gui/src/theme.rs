// Visual identity for meow-meow's native GUI: a dark "network operations
// console" look -- sharp corners, monospace-forward, teal/amber signal
// colors -- deliberately distinct from chat-app UI conventions (no rounded
// bubbles, no colorful avatar-driven layout).
use eframe::egui;

pub const BG_DEEP: egui::Color32 = egui::Color32::from_rgb(0x0b, 0x0d, 0x0f);
pub const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x15, 0x18);
pub const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(0x1a, 0x1e, 0x22);
pub const BG_INPUT: egui::Color32 = egui::Color32::from_rgb(0x0e, 0x10, 0x13);
pub const LINE: egui::Color32 = egui::Color32::from_rgb(0x24, 0x2a, 0x2e);
pub const LINE_BRIGHT: egui::Color32 = egui::Color32::from_rgb(0x35, 0x3d, 0x42);

pub const TEXT_BRIGHT: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xed, 0xf0);
pub const TEXT_NORMAL: egui::Color32 = egui::Color32::from_rgb(0xa8, 0xb4, 0xba);
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x5f, 0x6b, 0x72);

pub const TEAL: egui::Color32 = egui::Color32::from_rgb(0x2d, 0xd4, 0xbf);
pub const TEAL_DIM: egui::Color32 = egui::Color32::from_rgb(0x17, 0x5c, 0x54);
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(0xf5, 0xa6, 0x23);
pub const RED: egui::Color32 = egui::Color32::from_rgb(0xe5, 0x5b, 0x5b);
pub const GREEN: egui::Color32 = TEAL;

pub const MONO: egui::FontFamily = egui::FontFamily::Monospace;

/// Applies the theme to an egui context. Called once at startup.
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.visuals.dark_mode = true;
    style.visuals.window_fill = BG_PANEL;
    style.visuals.panel_fill = BG_PANEL;
    style.visuals.faint_bg_color = BG_ELEVATED;
    style.visuals.extreme_bg_color = BG_INPUT;
    style.visuals.window_stroke = egui::Stroke::new(1.0f32, LINE);
    style.visuals.override_text_color = Some(TEXT_NORMAL);

    // Sharp corners everywhere -- part of the "ops console", not "chat
    // app" visual distinction.
    let zero_rounding = egui::CornerRadius::ZERO;
    style.visuals.widgets.noninteractive.corner_radius = zero_rounding;
    style.visuals.widgets.inactive.corner_radius = zero_rounding;
    style.visuals.widgets.hovered.corner_radius = zero_rounding;
    style.visuals.widgets.active.corner_radius = zero_rounding;
    style.visuals.widgets.open.corner_radius = zero_rounding;
    style.visuals.window_corner_radius = zero_rounding;
    style.visuals.menu_corner_radius = zero_rounding;

    style.visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    style.visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0f32, TEXT_NORMAL);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0f32, LINE);

    style.visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    style.visuals.widgets.inactive.weak_bg_fill = BG_ELEVATED;
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0f32, TEXT_NORMAL);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0f32, LINE);

    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x22, 0x28, 0x2c);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0f32, TEXT_BRIGHT);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0f32, TEAL);

    style.visuals.widgets.active.bg_fill = TEAL_DIM;
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0f32, TEXT_BRIGHT);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0f32, TEAL);

    style.visuals.selection.bg_fill = TEAL_DIM;
    style.visuals.selection.stroke = egui::Stroke::new(1.0f32, TEAL);

    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(0);

    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(17.0, MONO),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(13.5, MONO),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(13.0, MONO),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(11.0, MONO),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(12.5, MONO),
    );

    ctx.set_style(style);
}

/// A small colored square used as a status indicator (online/stale/
/// offline), drawn manually rather than relying on egui's default dot
/// styling, to keep the "console" identity consistent.
pub fn status_color(seconds_since_seen: Option<u64>) -> egui::Color32 {
    match seconds_since_seen {
        None => TEXT_DIM,
        Some(s) if s <= 40 => GREEN,
        Some(s) if s <= 120 => AMBER,
        Some(_) => RED,
    }
}

pub fn rtt_color(rtt_ms: Option<u64>) -> egui::Color32 {
    match rtt_ms {
        None => TEXT_DIM,
        Some(r) if r < 80 => GREEN,
        Some(r) if r < 200 => AMBER,
        Some(_) => RED,
    }
}
