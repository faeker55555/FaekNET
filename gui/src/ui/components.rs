//! Shared visual primitives for consistent settings and domain panels.
use eframe::egui;
use crate::theme;

pub fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::BG_PANEL)
        .corner_radius(10)
        .stroke(egui::Stroke::new(1.0f32, theme::LINE))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(title).color(theme::TEXT_BRIGHT).size(14.0).strong());
            ui.add_space(10.0);
            body(ui);
        });
}

pub fn kv_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).color(theme::TEXT_BRIGHT).monospace());
        });
    });
    ui.add_space(4.0);
}
