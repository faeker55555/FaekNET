use eframe::egui;

use crate::app_state::{App, AppMode};
use crate::theme;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let AppMode::Running { snapshot, selected_peer, .. } = &mut app.mode else {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.colored_label(theme::TEXT_DIM, "Mesh is not running.");
            });
        });
        return;
    };

    egui::SidePanel::left("peer_list_panel")
        .resizable(false)
        .exact_width(320.0)
        .frame(
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0f32, theme::LINE))
                .inner_margin(egui::Margin::same(0)),
        )
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(12.0);
                header_row(ui, snapshot.peers.len());
                ui.add_space(6.0);

                if snapshot.peers.is_empty() {
                    ui.add_space(16.0);
                    ui.vertical_centered(|ui| {
                        ui.colored_label(theme::TEXT_DIM, "No peers yet.");
                        ui.colored_label(theme::TEXT_DIM, "Use + ADD PEER above.");
                    });
                }

                for peer in snapshot.peers.clone() {
                    let is_selected = *selected_peer == Some(peer.virtual_ip);
                    draw_peer_row(ui, &peer, is_selected, || {
                        *selected_peer = Some(peer.virtual_ip);
                    });
                }
                ui.add_space(12.0);
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::new().inner_margin(egui::Margin::same(20)))
        .show_inside(ui, |ui| {
            let sel = selected_peer.or_else(|| snapshot.peers.first().map(|p| p.virtual_ip));
            match sel.and_then(|ip| snapshot.peers.iter().find(|p| p.virtual_ip == ip)) {
                Some(peer) => draw_peer_detail(ui, peer),
                None => {
                    ui.colored_label(theme::TEXT_DIM, "Select a peer to see details.");
                }
            }
        });
}

fn header_row(ui: &mut egui::Ui, count: usize) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(format!("PEERS ({count})"))
                .color(theme::TEXT_DIM)
                .size(11.0)
                .strong(),
        );
    });
}

fn draw_peer_row(
    ui: &mut egui::Ui,
    peer: &lan_mesh_core::mesh::PeerSnapshot,
    selected: bool,
    mut on_click: impl FnMut(),
) {
    let bg = if selected { theme::BG_ELEVATED } else { theme::BG_PANEL };
    let frame = egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(14, 8));
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(dot_rect, 0.0, theme::status_color(peer.seconds_since_seen));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&peer.name).color(theme::TEXT_BRIGHT).strong(),
                        );
                        if peer.discovered_via_gossip {
                            egui::Frame::new()
                                .fill(theme::TEAL_DIM)
                                .inner_margin(egui::Margin::symmetric(4, 1))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("AUTO").color(theme::TEAL).size(9.0),
                                    );
                                });
                        }
                    });
                    ui.label(
                        egui::RichText::new(peer.virtual_ip.to_string())
                            .color(theme::TEXT_DIM)
                            .monospace()
                            .size(11.0),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let rtt_text = peer.rtt_ms.map(|r| format!("{r}ms")).unwrap_or_else(|| "--".to_string());
                    ui.label(
                        egui::RichText::new(rtt_text)
                            .color(theme::rtt_color(peer.rtt_ms))
                            .monospace()
                            .size(11.0),
                    );
                });
            });
        })
        .response;
    let resp = resp.interact(egui::Sense::click());
    if resp.clicked() {
        on_click();
    }
    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            0.0,
            egui::Stroke::new(1.0f32, theme::LINE_BRIGHT),
            egui::StrokeKind::Inside,
        );
    }
}

fn draw_peer_detail(ui: &mut egui::Ui, peer: &lan_mesh_core::mesh::PeerSnapshot) {
    ui.label(
        egui::RichText::new(&peer.name)
            .color(theme::TEXT_BRIGHT)
            .size(20.0)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(peer.virtual_ip.to_string())
            .color(theme::TEXT_DIM)
            .monospace(),
    );
    ui.add_space(16.0);

    egui::Frame::new()
        .fill(theme::BG_PANEL)
        .stroke(egui::Stroke::new(1.0f32, theme::LINE))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            detail_row(ui, "STATUS", match peer.seconds_since_seen {
                Some(s) if s <= 40 => "ONLINE".to_string(),
                Some(s) => format!("STALE ({s}s ago)"),
                None => "NEVER SEEN".to_string(),
            }, theme::status_color(peer.seconds_since_seen));
            detail_row(
                ui,
                "PUBLIC ADDRESS",
                peer.addr.map(|a| a.to_string()).unwrap_or_else(|| "unresolved".to_string()),
                theme::TEXT_BRIGHT,
            );
            detail_row(
                ui,
                "LATENCY",
                peer.rtt_ms.map(|r| format!("{r} ms")).unwrap_or_else(|| "--".to_string()),
                theme::rtt_color(peer.rtt_ms),
            );
            detail_row(
                ui,
                "DISCOVERY",
                if peer.discovered_via_gossip { "AUTO (GOSSIP)".to_string() } else { "MANUAL".to_string() },
                if peer.discovered_via_gossip { theme::AMBER } else { theme::TEXT_NORMAL },
            );
        });
}

fn detail_row(ui: &mut egui::Ui, label: &str, value: String, value_color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).color(value_color).monospace());
        });
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
}
