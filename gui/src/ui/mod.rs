mod domains_screen;
mod log_screen;
mod network_screen;
mod peer_modal;
mod peers_screen;
mod settings_screen;
mod setup_screen;
mod shell;

use crate::app_state::{App, AppMode};
use eframe::egui;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    match &app.mode {
        AppMode::Setup { .. } => setup_screen::draw(app, ctx),
        AppMode::Running { .. } | AppMode::Stopped { .. } => shell::draw(app, ctx),
    }

    if let Some((msg, at)) = &app.toast {
        if at.elapsed() < std::time::Duration::from_secs(4) {
            draw_toast(ctx, msg);
        } else {
            app.toast = None;
        }
    }
}

fn draw_toast(ctx: &egui::Context, msg: &str) {
    egui::Area::new(egui::Id::new("toast"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(crate::theme::BG_ELEVATED)
                .stroke(egui::Stroke::new(1.0f32, crate::theme::AMBER))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.colored_label(crate::theme::TEXT_BRIGHT, msg);
                });
        });
}
