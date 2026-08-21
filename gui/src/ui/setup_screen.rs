use eframe::egui;
use meow-meow_core::config::{Config, MeConfig};
use meow-meow_core::crypto::Cipher;
use meow-meow_core::share;

use crate::app_state::{App, AppMode, SetupStage};
use crate::theme;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::BG_DEEP))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading(
                    egui::RichText::new("MEOW_MEOW").color(theme::TEAL).size(28.0).monospace(),
                );
                ui.label(
                    egui::RichText::new("pure peer-to-peer virtual subnet -- no central server")
                        .color(theme::TEXT_DIM)
                        .size(12.0),
                );
                ui.add_space(32.0);

                egui::Frame::new()
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(440.0);
                        draw_form(app, ui);
                    });
            });
        });
}

fn draw_form(app: &mut App, ui: &mut egui::Ui) {
    let AppMode::Setup { stage, form, saved_config } = &mut app.mode else {
        return;
    };

    match stage {
        SetupStage::Identity => {
            ui.label(egui::RichText::new("01 // IDENTITY").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(6.0);

            labeled_field(ui, "DISPLAY NAME", &mut form.name);
            labeled_field(ui, "VIRTUAL IP", &mut form.virtual_ip);
            labeled_field(ui, "SUBNET PREFIX", &mut form.prefix);
            labeled_field(ui, "LISTEN PORT", &mut form.listen_port);
            labeled_field(ui, "LOCAL DOMAIN SUFFIX (peers become <name>.<suffix>)", &mut form.domain_suffix);

            ui.add_space(6.0);
            ui.label(egui::RichText::new("PRE-SHARED KEY").color(theme::TEXT_DIM).size(11.0));
            ui.label(
                egui::RichText::new(
                    "Everyone in your mesh needs the SAME key. First person: leave blank and \
                     generate one, then send it to the others over a channel you trust.",
                )
                .color(theme::TEXT_DIM)
                .size(11.0),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut form.psk_input)
                        .desired_width(300.0)
                        .hint_text("paste key here, or leave blank"),
                );
                if ui.button("GENERATE").clicked() {
                    let key = Cipher::generate_psk_b64();
                    form.psk_input = key.clone();
                    form.generated_psk = Some(key);
                }
            });
            if let Some(k) = &form.generated_psk {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(theme::BG_INPUT)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(k).color(theme::AMBER).monospace().size(11.0));
                        });
                    });
                ui.label(
                    egui::RichText::new("^ send this to your group over a trusted channel")
                        .color(theme::TEXT_DIM)
                        .size(10.5),
                );
            }

            if let Some(err) = &form.error {
                ui.add_space(8.0);
                ui.colored_label(theme::RED, err);
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            if ui
                .add_sized([ui.available_width(), 34.0], egui::Button::new("CONTINUE →"))
                .clicked()
            {
                match build_config(form) {
                    Ok(cfg) => {
                        if let Err(e) = cfg.save() {
                            form.error = Some(format!("Could not save config: {e}"));
                        } else {
                            *saved_config = Some(cfg);
                            *stage = SetupStage::Bootstrap;
                        }
                    }
                    Err(e) => form.error = Some(e),
                }
            }
        }

        SetupStage::Bootstrap => {
            ui.label(egui::RichText::new("02 // JOIN A MESH").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "If a friend is already running meow-meow, paste the card they sent you. \
                     Otherwise start solo -- you can add peers any time from the Peers screen, \
                     and once you're connected to just ONE other member, the rest of the mesh \
                     is discovered automatically.",
                )
                .color(theme::TEXT_NORMAL)
                .size(12.5),
            );
            ui.add_space(10.0);

            ui.label(egui::RichText::new("PEER CARD (optional)").color(theme::TEXT_DIM).size(11.0));
            ui.add(
                egui::TextEdit::multiline(app_bootstrap_buf(app))
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("LMESH1:..."),
            );

            if let Some(err) = &bootstrap_error(app) {
                ui.add_space(6.0);
                ui.colored_label(theme::RED, err.clone());
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui
                    .add_sized([160.0, 34.0], egui::Button::new("START SOLO"))
                    .clicked()
                {
                    finish_setup(app, false);
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::new(
                            egui::RichText::new("IMPORT & START").color(theme::BG_DEEP),
                        )
                        .fill(theme::TEAL),
                    )
                    .clicked()
                {
                    finish_setup(app, true);
                }
            });
        }
    }
}

fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(11.0));
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
    ui.add_space(6.0);
}

fn build_config(form: &crate::app_state::SetupForm) -> Result<Config, String> {
    let virtual_ip = form
        .virtual_ip
        .parse()
        .map_err(|_| "Invalid virtual IP address".to_string())?;
    let prefix: u8 = form.prefix.parse().map_err(|_| "Invalid subnet prefix".to_string())?;
    let listen_port: u16 = form
        .listen_port
        .parse()
        .map_err(|_| "Invalid listen port".to_string())?;

    let psk = if form.psk_input.trim().is_empty() {
        Cipher::generate_psk_b64()
    } else {
        form.psk_input.trim().to_string()
    };
    Cipher::from_psk_b64(&psk).map_err(|e| format!("Invalid pre-shared key: {e}"))?;

    let domain_suffix = if form.domain_suffix.trim().is_empty() {
        "mesh".to_string()
    } else {
        form.domain_suffix.trim().to_string()
    };

    Ok(Config {
        me: MeConfig {
            name: if form.name.trim().is_empty() { "player".to_string() } else { form.name.trim().to_string() },
            virtual_ip,
            prefix,
            listen_port,
            psk,
            mtu: 1400,
            domain_suffix,
            sync_hosts_file: true,
            dns_server: false,
            dns_port: 53,
            dns_auto_configure: false,
            manual_public_ip: None,
            manual_public_port: None,
            warp_compat: true,
            cache_public_addr: false,
            cached_public_ip: None,
            cached_public_port: None,
        },
        peers: Vec::new(),
        services: Vec::new(),
    })
}

// Small helpers to work around borrow-checker friction when reaching into
// `app.mode` for the bootstrap textbox while also needing `&mut app` more
// broadly in the caller.
fn app_bootstrap_buf(app: &mut App) -> &mut String {
    if app.add_peer_modal.paste_buffer.is_empty() {
        // Reuse the same scratch buffer as the main "add peer" modal so
        // there's only one piece of state to manage for "a card the user
        // is typing in".
    }
    &mut app.add_peer_modal.paste_buffer
}

fn bootstrap_error(app: &App) -> Option<String> {
    app.add_peer_modal.error.clone()
}

fn finish_setup(app: &mut App, try_import: bool) {
    let AppMode::Setup { saved_config, .. } = &app.mode else {
        return;
    };
    let Some(mut cfg) = saved_config.clone() else {
        return;
    };

    if try_import {
        let card = app.add_peer_modal.paste_buffer.trim().to_string();
        if !card.is_empty() {
            match share::decode(&card) {
                Ok(peer) => {
                    cfg.peers.push(peer);
                    let _ = cfg.save();
                }
                Err(e) => {
                    app.add_peer_modal.error = Some(e);
                    return;
                }
            }
        }
    }

    app.add_peer_modal.paste_buffer.clear();
    app.add_peer_modal.error = None;
    app.start_mesh(cfg);
}
