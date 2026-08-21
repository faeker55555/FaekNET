use eframe::egui;

use crate::app_state::{App, AppMode};
use crate::theme;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 16.0);
                draw_content(app, ui);
            });
        });
        ui.add_space(16.0);
    });
}

fn draw_content(app: &mut App, ui: &mut egui::Ui) {
    match &app.mode {
        AppMode::Running { snapshot, .. } => {
            draw_stat_grid(ui, snapshot);
            ui.add_space(16.0);
            draw_mesh_map(ui, snapshot);
        }
        AppMode::Stopped { config } => {
            section_frame(ui, "MESH STOPPED", |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Configured as '{}' ({}). Click START MESH in the top bar to bring \
                         the virtual adapter up.",
                        config.me.name, config.me.virtual_ip
                    ))
                    .color(theme::TEXT_NORMAL),
                );
            });
        }
        AppMode::Setup { .. } => {}
    }
}

fn section_frame(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::BG_PANEL)
        .stroke(egui::Stroke::new(1.0f32, theme::LINE))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(title).color(theme::TEXT_DIM).size(11.0).strong());
            ui.add_space(10.0);
            body(ui);
        });
}

fn draw_stat_grid(ui: &mut egui::Ui, snapshot: &meow-meow_core::mesh::MeshSnapshot) {
    let online = snapshot
        .peers
        .iter()
        .filter(|p| p.seconds_since_seen.map(|s| s <= 40).unwrap_or(false))
        .count();
    let discovered = snapshot.peers.iter().filter(|p| p.discovered_via_gossip).count();
    let public_addr = snapshot
        .my_public_addr
        .map(|a| a.to_string())
        .unwrap_or_else(|| "resolving...".to_string());

    ui.columns(4, |cols| {
        stat_box(&mut cols[0], "PEERS ONLINE", &format!("{online} / {}", snapshot.peers.len()), theme::TEAL);
        stat_box(&mut cols[1], "PUBLIC ADDRESS", &public_addr, theme::TEXT_BRIGHT);
        stat_box(&mut cols[2], "LISTEN PORT", &snapshot.listen_port.to_string(), theme::TEXT_BRIGHT);
        stat_box(&mut cols[3], "AUTO-DISCOVERED", &discovered.to_string(), theme::AMBER);
    });
}

fn stat_box(ui: &mut egui::Ui, label: &str, value: &str, value_color: egui::Color32) {
    egui::Frame::new()
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0f32, theme::LINE))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_height(60.0);
            ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(10.5));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(value)
                    .color(value_color)
                    .monospace()
                    .size(16.0)
                    .strong(),
            );
        });
}

/// A schematic hub-and-spoke diagram: "you" in the center, every peer
/// around it, colored by liveness, with a dashed line marking
/// gossip-auto-discovered peers vs. a solid line for manually-added ones.
fn draw_mesh_map(ui: &mut egui::Ui, snapshot: &meow-meow_core::mesh::MeshSnapshot) {
    section_frame(ui, "MESH TOPOLOGY", |ui| {
        let height = 260.0;
        let (rect, _response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::BG_ELEVATED);

        let center = rect.center();
        let n = snapshot.peers.len().max(1);
        let radius = (rect.width().min(rect.height()) / 2.0 - 50.0).max(40.0);

        // Spokes first so node circles paint on top of the lines.
        for (i, peer) in snapshot.peers.iter().enumerate() {
            let angle = (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let pos = center + egui::vec2(angle.cos(), angle.sin()) * radius;
            let color = theme::status_color(peer.seconds_since_seen);
            let stroke = if peer.discovered_via_gossip {
                egui::Stroke::new(1.5f32, color.gamma_multiply(0.6))
            } else {
                egui::Stroke::new(2.0f32, color)
            };
            if peer.discovered_via_gossip {
                draw_dashed_line(&painter, center, pos, stroke);
            } else {
                painter.line_segment([center, pos], stroke);
            }
        }

        // Center node ("you").
        painter.circle_filled(center, 22.0, theme::TEAL_DIM);
        painter.circle_stroke(center, 22.0, egui::Stroke::new(1.5f32, theme::TEAL));
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            "YOU",
            egui::FontId::monospace(11.0),
            theme::TEXT_BRIGHT,
        );

        // Peer nodes.
        for (i, peer) in snapshot.peers.iter().enumerate() {
            let angle = (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let pos = center + egui::vec2(angle.cos(), angle.sin()) * radius;
            let color = theme::status_color(peer.seconds_since_seen);
            painter.circle_filled(pos, 16.0, theme::BG_PANEL);
            painter.circle_stroke(pos, 16.0, egui::Stroke::new(1.5f32, color));
            let label = short_label(&peer.name);
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::monospace(9.5),
                theme::TEXT_BRIGHT,
            );
            painter.text(
                pos + egui::vec2(0.0, 24.0),
                egui::Align2::CENTER_CENTER,
                peer.virtual_ip.to_string(),
                egui::FontId::monospace(9.0),
                theme::TEXT_DIM,
            );
        }

        if snapshot.peers.is_empty() {
            painter.text(
                center + egui::vec2(0.0, 50.0),
                egui::Align2::CENTER_CENTER,
                "no peers yet -- add one to join a mesh",
                egui::FontId::monospace(11.0),
                theme::TEXT_DIM,
            );
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            legend_dot(ui, theme::TEAL, "online");
            legend_dot(ui, theme::AMBER, "stale");
            legend_dot(ui, theme::RED, "offline");
            ui.add_space(12.0);
            ui.label(egui::RichText::new("— manual").color(theme::TEXT_DIM).size(10.5));
            ui.label(egui::RichText::new("┄ auto-discovered").color(theme::TEXT_DIM).size(10.5));
        });
    });
}

fn short_label(name: &str) -> String {
    name.chars().take(3).collect::<String>().to_uppercase()
}

fn draw_dashed_line(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, stroke: egui::Stroke) {
    let dir = to - from;
    let len = dir.length();
    let step = 8.0;
    let n = (len / step).floor() as i32;
    let unit = dir / len.max(0.001);
    let mut i = 0;
    while (i as f32) < n as f32 {
        let start = from + unit * (i as f32 * step);
        let end = from + unit * ((i as f32 * step) + step * 0.6).min(len);
        painter.line_segment([start, end], stroke);
        i += 1;
    }
}

fn legend_dot(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color);
    ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(10.5));
    ui.add_space(8.0);
}
