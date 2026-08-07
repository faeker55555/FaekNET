use eframe::egui;
use lan_mesh_core::config::Config;
use lan_mesh_core::stun;

use crate::app_state::{App, AppMode};
use crate::theme;

/// Builds a peer card for the current identity. If the mesh is already
/// running, its own background self-STUN thread has (usually) already
/// discovered our public address -- reuse that instead of trying a fresh
/// STUN probe, which would otherwise fail: the mesh's UDP socket already
/// owns `listen_port`, so a second, independent probe attempting to bind
/// the same port is guaranteed to lose the race for it.
pub fn generate_my_card(app: &mut App, cfg: &Config) {
    let known_addr = if let AppMode::Running { handle, .. } = &app.mode {
        handle.snapshot().my_public_addr
    } else {
        None
    };

    let addr = known_addr.or_else(|| stun::discover_external_addr_any(cfg.me.listen_port));

    match addr {
        Some(addr) => {
            let card = lan_mesh_core::share::encode(
                &cfg.me.name,
                cfg.me.virtual_ip,
                &addr.ip().to_string(),
                addr.port(),
            );
            app.add_peer_modal.my_card = Some(card);
            app.add_peer_modal.card_error = None;
        }
        None => {
            app.add_peer_modal.card_error = Some(
                "Could not determine your public address yet -- if the mesh just started, \
                 wait a few seconds for it to finish discovering it, then try again."
                    .to_string(),
            );
        }
    }
}

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 32.0).min(560.0));
                draw_content(app, ui);
            });
        });
    });
}

fn draw_content(app: &mut App, ui: &mut egui::Ui) {
    let Some(cfg) = app.current_config() else {
        return;
    };

    section(ui, "IDENTITY", |ui| {
        kv_row(ui, "NAME", &cfg.me.name);
        kv_row(ui, "VIRTUAL IP", &cfg.me.virtual_ip.to_string());
        kv_row(ui, "SUBNET PREFIX", &format!("/{}", cfg.me.prefix));
        kv_row(ui, "LISTEN PORT", &cfg.me.listen_port.to_string());
    });

    ui.add_space(14.0);

    section(ui, "PRE-SHARED KEY", |ui| {
        ui.label(
            egui::RichText::new("Anyone with this key can join your mesh. Treat it like a password.")
                .color(theme::TEXT_DIM)
                .size(11.0),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut cfg.me.psk.clone())
                    .password(true)
                    .desired_width(300.0),
            );
            if ui.button("COPY").clicked() {
                ui.ctx().copy_text(cfg.me.psk.clone());
                app.show_toast("Key copied to clipboard");
            }
        });
    });

    ui.add_space(14.0);

    section(ui, "MY PEER CARD", |ui| {
        ui.label(
            egui::RichText::new(
                "Send this to a friend so they can add you with one paste \
                 (does NOT include the pre-shared key -- send that separately).",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);
        if ui.button("GENERATE CARD").clicked() {
            generate_my_card(app, &cfg);
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
            if ui.button("COPY CARD").clicked() {
                ui.ctx().copy_text(card);
                app.show_toast("Card copied to clipboard");
            }
        }
        if let Some(err) = &app.add_peer_modal.card_error {
            ui.colored_label(theme::RED, err);
        }
    });

    ui.add_space(14.0);

    section(ui, "ABOUT", |ui| {
        kv_row(ui, "CONFIG FILE", "mesh.toml (current directory)");
        kv_row(ui, "TRANSPORT", "UDP, ChaCha20-Poly1305 encrypted");
        kv_row(ui, "DISCOVERY", "gossip (no central server)");
    });
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
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

fn kv_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).color(theme::TEXT_BRIGHT).monospace());
        });
    });
    ui.add_space(4.0);
}
