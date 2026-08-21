use eframe::egui;
use meow-meow_core::config::Config;
use meow-meow_core::stun;

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
            let card = meow-meow_core::share::encode(
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

    section(ui, "LOCAL DOMAIN NAMES & DNS", |ui| {
        ui.label(
            egui::RichText::new(
                "See the Domains screen for the full list of names and the in-app browser. \
                 These settings live in mesh.toml; changes here apply the next time the mesh \
                 is (re)started.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(8.0);
        kv_row(ui, "DOMAIN SUFFIX", &cfg.me.domain_suffix);
        kv_row(ui, "HOSTS-FILE SYNC", if cfg.me.sync_hosts_file { "enabled" } else { "disabled" });
        kv_row(ui, "BUILT-IN DNS RESOLVER", if cfg.me.dns_server { "enabled" } else { "disabled" });
        if cfg.me.dns_server {
            kv_row(ui, "DNS PORT", &cfg.me.dns_port.to_string());
            kv_row(
                ui,
                "DNS AUTO-CONFIGURE",
                if cfg.me.dns_auto_configure { "enabled" } else { "disabled" },
            );
        }
    });

    ui.add_space(14.0);

    section(ui, "PUBLIC ADDRESS DISCOVERY", |ui| {
        ui.label(
            egui::RichText::new(
                "The mesh normally discovers your public IP:port automatically via STUN so \
                 friends can reach you directly (no relay). If self-STUN never resolves on \
                 your machine, use the tools below to work around it.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(8.0);

        let live_addr = if let AppMode::Running { handle, .. } = &app.mode {
            handle.my_public_addr()
        } else {
            None
        };
        kv_row(
            ui,
            "CURRENT ADDRESS",
            &match (&app.mode, live_addr) {
                (AppMode::Running { .. }, Some(addr)) => addr.to_string(),
                (AppMode::Running { .. }, None) => "resolving...".to_string(),
                _ => "(mesh not running)".to_string(),
            },
        );
        let has_manual = cfg.me.manual_public_ip.is_some() && cfg.me.manual_public_port.is_some();
        kv_row(ui, "SOURCE", if has_manual { "manual override" } else { "self-STUN (automatic)" });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(egui::RichText::new("MANUAL OVERRIDE").color(theme::TEXT_BRIGHT).size(12.0).strong());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Bypass STUN entirely by entering your own public IP and port (e.g. as shown \
                 by whatsmyip.com, forwarded to this machine's listen port on your router).",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);

        if has_manual {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Currently set: {}:{}",
                        cfg.me.manual_public_ip.clone().unwrap_or_default(),
                        cfg.me.manual_public_port.unwrap_or_default()
                    ))
                    .color(theme::TEAL)
                    .monospace(),
                );
                if ui.button("CLEAR OVERRIDE").clicked() {
                    clear_manual_addr(app);
                }
            });
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("IP").color(theme::TEXT_DIM).size(11.0));
            ui.add(
                egui::TextEdit::singleline(&mut app.manual_addr_form.ip)
                    .hint_text("203.0.113.5")
                    .desired_width(140.0),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new("PORT").color(theme::TEXT_DIM).size(11.0));
            ui.add(
                egui::TextEdit::singleline(&mut app.manual_addr_form.port)
                    .hint_text(cfg.me.listen_port.to_string())
                    .desired_width(80.0),
            );
            ui.add_space(8.0);
            if ui.button("SET").clicked() {
                set_manual_addr(app);
            }
        });
        if let Some(err) = &app.manual_addr_form.error {
            ui.colored_label(theme::RED, err);
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(egui::RichText::new("CACHE PUBLIC ADDRESS").color(theme::TEXT_BRIGHT).size(12.0).strong());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "When enabled, the last successfully discovered (or manually set) public \
                 address is saved to mesh.toml, so future launches can start using it right \
                 away instead of waiting on STUN. Applies the next time the mesh is (re)started.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);
        let mut cache_enabled = cfg.me.cache_public_addr;
        if ui.checkbox(&mut cache_enabled, "Cache public address to mesh.toml").changed() {
            set_cache_public_addr(app, cache_enabled);
        }
        ui.add_space(6.0);
        if ui.button("RESET PUBLIC ADDRESS").clicked() {
            reset_public_addr_action(app);
        }
        ui.label(
            egui::RichText::new("Clears any cached address and forces a fresh self-STUN discovery.")
                .color(theme::TEXT_DIM)
                .size(11.0),
        );

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(egui::RichText::new("WARP COMPATIBILITY").color(theme::TEXT_BRIGHT).size(12.0).strong());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "By default the mesh pins its socket to a non-VPN network interface so it \
                 still works while Cloudflare WARP or another VPN is active. If self-STUN \
                 never resolves and you are NOT using a VPN, try disabling this -- it lets \
                 the OS pick the route normally, like any other application. Takes effect \
                 the next time the mesh is (re)started.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);
        let mut warp_compat = cfg.me.warp_compat;
        if ui.checkbox(&mut warp_compat, "Enable WARP/VPN-compatible interface pinning").changed() {
            set_warp_compat(app, warp_compat);
        }
    });

    ui.add_space(14.0);

    section(ui, "START ON LOGIN", |ui| {
        ui.label(
            egui::RichText::new(
                "Launch meow-meow when you log in, starting minimized in the system tray so the \
                 mesh is up immediately without a window popping up. Uses the XDG autostart \
                 entry on Linux and the Run registry key on Windows.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);
        let mut enabled = crate::autostart::is_enabled();
        if ui
            .checkbox(&mut enabled, "Start meow-meow automatically on system startup")
            .changed()
        {
            let res = if enabled {
                crate::autostart::enable()
            } else {
                crate::autostart::disable()
            };
            match res {
                Ok(()) => app.show_toast(if enabled {
                    "Autostart enabled -- meow-meow will start with your session."
                } else {
                    "Autostart disabled."
                }),
                Err(e) => app.show_toast(format!("Autostart failed: {e}")),
            }
        }
    });

    ui.add_space(14.0);

    section(ui, "SYSTEM TRAY", |ui| {
        ui.label(
            egui::RichText::new(
                "Closing or minimizing the window keeps the mesh running in the tray. \
                 Use the tray's Quit item (or the checkbox below, off) to exit for real.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);
        let mut close_to_tray = app.gui_settings.close_to_tray;
        if ui
            .checkbox(&mut close_to_tray, "Minimize / close to tray instead of quitting")
            .changed()
        {
            app.gui_settings.close_to_tray = close_to_tray;
            app.gui_settings.save();
        }
        kv_row(
            ui,
            "TRAY ICON",
            if app.tray.is_some() {
                "active"
            } else {
                "unavailable on this system"
            },
        );
    });

    ui.add_space(14.0);

    section(ui, "UPDATES", |ui| {
        kv_row(ui, "CURRENT VERSION", crate::updater::current_version());
        let mut chk = app.gui_settings.check_updates_on_start;
        if ui
            .checkbox(&mut chk, "Check for updates when meow-meow starts")
            .changed()
        {
            app.gui_settings.check_updates_on_start = chk;
            app.gui_settings.save();
        }

        ui.add_space(8.0);
        let status = app.updater.lock().unwrap().status.clone();
        let busy = matches!(
            &status,
            crate::updater::UpdateStatus::Checking
                | crate::updater::UpdateStatus::Downloading { .. }
                | crate::updater::UpdateStatus::Verifying
                | crate::updater::UpdateStatus::Extracting
                | crate::updater::UpdateStatus::Applying
        );
        if ui
            .add_enabled(!busy, egui::Button::new("CHECK FOR UPDATES"))
            .clicked()
        {
            crate::updater::start_check(app.updater.clone());
        }
        ui.add_space(6.0);

        match &status {
            crate::updater::UpdateStatus::Idle => {
                ui.colored_label(theme::TEXT_DIM, "No update check run yet.");
            }
            crate::updater::UpdateStatus::Checking => {
                ui.colored_label(theme::TEXT_NORMAL, "Checking for updates...");
            }
            crate::updater::UpdateStatus::UpToDate { version } => {
                ui.colored_label(theme::TEAL, format!("Up to date (latest release: {version})."));
            }
            crate::updater::UpdateStatus::Available { version } => {
                ui.colored_label(theme::AMBER, format!("Version {version} is available."));
                if let Some(info) = &app.updater.lock().unwrap().info {
                    if !info.notes.trim().is_empty() {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(theme::BG_INPUT)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&info.notes)
                                            .color(theme::TEXT_NORMAL)
                                            .size(11.0),
                                    )
                                    .wrap(),
                                );
                            });
                    }
                }
                ui.add_space(8.0);
                if ui.button("DOWNLOAD & INSTALL").clicked() {
                    if let Some(info) = app.updater.lock().unwrap().info.clone() {
                        crate::updater::start_install(
                            app.updater.clone(),
                            info,
                            app.updater_quit.clone(),
                        );
                    }
                }
            }
            crate::updater::UpdateStatus::NoRelease => {
                ui.colored_label(
                    theme::TEXT_DIM,
                    "No newer release with a package for this platform.",
                );
            }
            crate::updater::UpdateStatus::Downloading { bytes, total } => {
                let (bytes, total) = (*bytes, *total);
                let pct = if total > 0 {
                    (bytes as f32 / total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ui.add(egui::ProgressBar::new(pct).show_percentage());
                ui.colored_label(
                    theme::TEXT_NORMAL,
                    format!(
                        "Downloading... {:.1} / {:.1} MB",
                        bytes as f64 / 1_000_000.0,
                        total as f64 / 1_000_000.0
                    ),
                );
            }
            crate::updater::UpdateStatus::Verifying => {
                ui.colored_label(theme::TEXT_NORMAL, "Verifying checksum...");
            }
            crate::updater::UpdateStatus::Extracting => {
                ui.colored_label(theme::TEXT_NORMAL, "Extracting update...");
            }
            crate::updater::UpdateStatus::Applying => {
                ui.colored_label(theme::TEAL, "Applying update -- restarting...");
            }
            crate::updater::UpdateStatus::Failed(e) => {
                ui.colored_label(theme::RED, format!("Update failed: {e}"));
            }
        }
    });

    ui.add_space(14.0);

    section(ui, "ABOUT", |ui| {
        kv_row(ui, "CONFIG FILE", "mesh.toml (current directory)");
        kv_row(ui, "TRANSPORT", "UDP, ChaCha20-Poly1305 encrypted");
        kv_row(ui, "DISCOVERY", "gossip (no central server)");
    });
}

/// Parses and applies the manual public IP/port override from
/// `app.manual_addr_form`, following the same branch-on-`&app.mode`
/// pattern as `domains_screen.rs`'s `add_service`: live mesh gets the
/// change applied immediately via `MeshHandle::set_manual_public_addr`
/// (which also wakes the self-STUN thread so it takes effect without
/// waiting up to 25s), anything else just persists to mesh.toml for the
/// next start.
fn set_manual_addr(app: &mut App) {
    let ip_str = app.manual_addr_form.ip.trim().to_string();
    let port_str = app.manual_addr_form.port.trim().to_string();

    let Ok(ip) = ip_str.parse::<std::net::Ipv4Addr>() else {
        app.manual_addr_form.error = Some("Invalid IPv4 address.".to_string());
        return;
    };
    let Ok(port) = port_str.parse::<u16>() else {
        app.manual_addr_form.error = Some("Invalid port (must be 0-65535).".to_string());
        return;
    };

    match &app.mode {
        AppMode::Running { handle, .. } => {
            handle.set_manual_public_addr(ip, port);
            app.show_toast(format!("Public address manually set to {ip}:{port}."));
        }
        _ => {
            if let Some(mut cfg) = app.current_config() {
                cfg.me.manual_public_ip = Some(ip.to_string());
                cfg.me.manual_public_port = Some(port);
                app.save_and_sync_config(cfg);
                app.show_toast(format!("Public address manually set to {ip}:{port}."));
            }
        }
    }
    app.manual_addr_form.ip.clear();
    app.manual_addr_form.port.clear();
    app.manual_addr_form.error = None;
}

/// Clears a previously-set manual override. See `set_manual_addr` for
/// the live-vs-persisted branch rationale.
fn clear_manual_addr(app: &mut App) {
    match &app.mode {
        AppMode::Running { handle, .. } => {
            handle.clear_manual_public_addr();
        }
        _ => {
            if let Some(mut cfg) = app.current_config() {
                cfg.me.manual_public_ip = None;
                cfg.me.manual_public_port = None;
                app.save_and_sync_config(cfg);
            }
        }
    }
    app.show_toast("Manual public address override cleared.");
}

/// Toggles whether the discovered/manual public address is cached to
/// mesh.toml. There is no live `MeshHandle` equivalent for this: the
/// flag is only consulted at self-STUN-success time and at mesh
/// startup, so a running mesh's config is updated on disk but only
/// takes full effect (both stopping/starting caching behavior) the next
/// time the mesh is (re)started.
fn set_cache_public_addr(app: &mut App, enabled: bool) {
    if let Some(mut cfg) = app.current_config() {
        cfg.me.cache_public_addr = enabled;
        if let AppMode::Running { handle, .. } = &app.mode {
            let _ = handle;
            // Live mesh keeps running with its already-loaded config;
            // persist to disk directly instead of save_and_sync_config
            // (which is a no-op for Running) so the new value is picked
            // up on the next (re)start.
            let _ = cfg.save();
        } else {
            app.save_and_sync_config(cfg);
        }
    }
    app.show_toast(if enabled {
        "Public address caching enabled -- applies on next (re)start."
    } else {
        "Public address caching disabled -- applies on next (re)start."
    });
}

/// Toggles WARP/VPN-compatible interface pinning. Like
/// `set_cache_public_addr`, this only takes effect when the mesh's UDP
/// socket is (re)created, so a running mesh's config is updated on disk
/// for the next (re)start rather than applied live.
fn set_warp_compat(app: &mut App, enabled: bool) {
    if let Some(mut cfg) = app.current_config() {
        cfg.me.warp_compat = enabled;
        if matches!(app.mode, AppMode::Running { .. }) {
            let _ = cfg.save();
        } else {
            app.save_and_sync_config(cfg);
        }
    }
    app.show_toast("WARP compatibility setting saved -- applies on next (re)start.");
}

/// Clears the cached public address and forces a fresh self-STUN
/// discovery. Named `reset_public_addr_action` (rather than
/// `reset_public_addr`) to avoid colliding with
/// `MeshHandle::reset_public_addr`, which this calls in the live case.
fn reset_public_addr_action(app: &mut App) {
    match &app.mode {
        AppMode::Running { handle, .. } => {
            handle.reset_public_addr();
            app.show_toast("Public address reset -- re-running self-STUN discovery now.");
        }
        _ => {
            if let Some(mut cfg) = app.current_config() {
                cfg.clear_cached_public_addr();
                app.save_and_sync_config(cfg);
                app.show_toast("Cached public address cleared -- will re-run self-STUN on next start.");
            }
        }
    }
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
