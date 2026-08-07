use eframe::egui;

use crate::app_state::App;
use crate::theme;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("ACTIVITY LOG").color(theme::TEXT_DIM).size(11.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("CLEAR").clicked() {
                        app.logs.lock().unwrap().clear();
                    }
                });
            });
            ui.add_space(8.0);

            egui::Frame::new()
                .fill(theme::BG_INPUT)
                .stroke(egui::Stroke::new(1.0f32, theme::LINE))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    let height = ui.available_height();
                    egui::ScrollArea::vertical()
                        .max_height(height)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            let logs = app.logs.lock().unwrap();
                            if logs.is_empty() {
                                ui.colored_label(theme::TEXT_DIM, "(no log lines yet)");
                            }
                            for line in logs.iter() {
                                let color = line_color(line);
                                ui.label(egui::RichText::new(line).monospace().size(11.5).color(color));
                            }
                        });
                });
        });
}

fn line_color(line: &str) -> egui::Color32 {
    if line.contains("Discovered new peer") || line.contains("via gossip") {
        theme::AMBER
    } else if line.to_lowercase().contains("error") || line.to_lowercase().contains("warning") {
        theme::RED
    } else if line.contains("reachable") {
        theme::TEAL
    } else {
        theme::TEXT_NORMAL
    }
}
