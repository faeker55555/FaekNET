// Desktop visual language: warm graphite surfaces, copper accents, readable proportional
// text, with monospace reserved for network addresses and measurements.
use eframe::egui;

pub const BG_DEEP: egui::Color32 = egui::Color32::from_rgb(0x16, 0x15, 0x14);
pub const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(0x20, 0x1e, 0x1d);
pub const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x26, 0x24);
pub const BG_INPUT: egui::Color32 = egui::Color32::from_rgb(0x12, 0x11, 0x10);
pub const LINE: egui::Color32 = egui::Color32::from_rgb(0x3a, 0x33, 0x2f);
pub const LINE_BRIGHT: egui::Color32 = egui::Color32::from_rgb(0x4b, 0x40, 0x3a);

pub const TEXT_BRIGHT: egui::Color32 = egui::Color32::from_rgb(0xf2, 0xec, 0xe7);
pub const TEXT_NORMAL: egui::Color32 = egui::Color32::from_rgb(0xb9, 0xae, 0xa6);
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x9f, 0x94, 0x8b);

pub const TEAL: egui::Color32 = egui::Color32::from_rgb(0xd3, 0x85, 0x6d);
pub const TEAL_DIM: egui::Color32 = egui::Color32::from_rgb(0x3e, 0x38, 0x35);
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xa0, 0x5d);
pub const RED: egui::Color32 = egui::Color32::from_rgb(0xe5, 0x74, 0x5b);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(0x91, 0xbf, 0xa0);

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

    // Softer controls separate actions from the surrounding network data.
    let rounding = egui::CornerRadius::same(7);
    style.visuals.widgets.noninteractive.corner_radius = rounding;
    style.visuals.widgets.inactive.corner_radius = rounding;
    style.visuals.widgets.hovered.corner_radius = rounding;
    style.visuals.widgets.active.corner_radius = rounding;
    style.visuals.widgets.open.corner_radius = rounding;
    style.visuals.window_corner_radius = rounding;
    style.visuals.menu_corner_radius = rounding;

    style.visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    style.visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0f32, TEXT_NORMAL);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0f32, LINE);

    style.visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    style.visuals.widgets.inactive.weak_bg_fill = BG_ELEVATED;
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0f32, TEXT_NORMAL);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0f32, LINE);

    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x32, 0x2c, 0x29);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0f32, TEXT_BRIGHT);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0f32, TEAL);

    style.visuals.widgets.active.bg_fill = TEAL_DIM;
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0f32, TEXT_BRIGHT);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0f32, TEAL);

    style.visuals.selection.bg_fill = TEAL_DIM;
    style.visuals.selection.stroke = egui::Stroke::new(1.0f32, TEAL);

    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.window_margin = egui::Margin::same(0);

    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(24.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
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
