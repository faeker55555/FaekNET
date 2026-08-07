use eframe::egui;

use crate::app_state::App;
use crate::theme;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    if !app.add_peer_modal.open {
        return;
    }

    let mut open = true;
    egui::Window::new("ADD PEER")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0f32, theme::TEAL))
                .inner_margin(egui::Margin::same(18)),
        )
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .fixed_size(egui::vec2(440.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "You only need to do this once per relationship -- everyone else in the \
                     mesh is discovered automatically afterward.",
                )
                .color(theme::TEXT_DIM)
                .size(11.0),
            );
            ui.add_space(12.0);

            ui.label(egui::RichText::new("PASTE THEIR CARD").color(theme::TEXT_DIM).size(11.0).strong());
            ui.add(
                egui::TextEdit::multiline(&mut app.add_peer_modal.paste_buffer)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("LMESH1:..."),
            );

            if let Some(err) = &app.add_peer_modal.error {
                ui.add_space(6.0);
                ui.colored_label(theme::RED, err);
            }

            ui.add_space(8.0);
            if ui
                .add_sized([ui.available_width(), 30.0], egui::Button::new("IMPORT"))
                .clicked()
            {
                import_card(app);
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(10.0);

            ui.label(egui::RichText::new("OR SHARE YOUR CARD").color(theme::TEXT_DIM).size(11.0).strong());
            if ui.button("GENERATE MY CARD").clicked() {
                generate_card(app);
            }
            if let Some(card) = app.add_peer_modal.my_card.clone() {
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(theme::BG_INPUT)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(&card).monospace().size(11.0).color(theme::TEAL));
                        });
                    });
                if ui.button("COPY").clicked() {
                    ui.ctx().copy_text(card);
                    app.show_toast("Card copied to clipboard");
                }
            }
            if let Some(err) = &app.add_peer_modal.card_error {
                ui.colored_label(theme::RED, err);
            }
        });

    if !open {
        app.add_peer_modal.open = false;
        app.add_peer_modal.error = None;
    }
}

fn import_card(app: &mut App) {
    let card = app.add_peer_modal.paste_buffer.trim().to_string();
    if card.is_empty() {
        app.add_peer_modal.error = Some("Paste a card first.".to_string());
        return;
    }
    let Some(cfg) = app.current_config() else {
        return;
    };
    match lan_mesh_core::share::decode(&card) {
        Ok(peer) => {
            if peer.virtual_ip == cfg.me.virtual_ip {
                app.add_peer_modal.error = Some("That's your own card.".to_string());
                return;
            }
            match &app.mode {
                crate::app_state::AppMode::Running { handle, .. } => {
                    // Mesh is live -- add directly to the running peer
                    // table (also persists to disk) so it starts
                    // hole-punching immediately, no restart needed.
                    handle.add_peer_live(peer);
                    app.add_peer_modal.paste_buffer.clear();
                    app.add_peer_modal.error = None;
                    app.add_peer_modal.open = false;
                    app.show_toast("Peer added -- connecting now.");
                }
                _ => {
                    // Mesh isn't running yet -- just persist to disk for
                    // when it starts.
                    let mut cfg = cfg;
                    if let Some(existing) = cfg.peers.iter_mut().find(|p| p.virtual_ip == peer.virtual_ip) {
                        *existing = peer;
                    } else {
                        cfg.peers.push(peer);
                    }
                    let _ = cfg.save();
                    app.add_peer_modal.paste_buffer.clear();
                    app.add_peer_modal.error = None;
                    app.add_peer_modal.open = false;
                    app.show_toast("Peer saved. Start the mesh to connect.");
                }
            }
        }
        Err(e) => app.add_peer_modal.error = Some(e),
    }
}


fn generate_card(app: &mut App) {
    let Some(cfg) = app.current_config() else {
        return;
    };
    super::settings_screen::generate_my_card(app, &cfg);
}
