//! Local mesh domain names screen: shows every peer's `<name>.<suffix>`
//! address plus any named subdomains (services) hosted under it, whether
//! hosts-file sync / the built-in DNS resolver are enabled, a form for
//! advertising your own services, and one-click shortcuts to open a name
//! in the in-app browser (a separate `lan_mesh_browser` process -- see
//! that crate's Cargo.toml for why it isn't an embedded egui panel).

use eframe::egui;
use lan_mesh_core::config::Config;
use lan_mesh_core::mesh::DomainNameEntry;

use crate::app_state::{App, AppMode};
use crate::theme;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 32.0).min(760.0));
                draw_content(app, ui);
            });
        });
        ui.add_space(16.0);
    });
}

fn draw_content(app: &mut App, ui: &mut egui::Ui) {
    let Some(cfg) = app.current_config() else {
        return;
    };

    section(ui, "LOCAL DOMAIN NAMES", |ui| {
        ui.label(
            egui::RichText::new(format!(
                "Every peer is reachable by name -- <peer>.{} -- instead of just a raw \
                 virtual IP, and any named service they advertise gets its own subdomain \
                 (e.g. game.alice.{}). No internet DNS involved; this is purely local to \
                 your mesh.",
                cfg.me.domain_suffix, cfg.me.domain_suffix
            ))
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
    });

    ui.add_space(14.0);

    section(ui, "RESOLUTION MECHANISMS", |ui| {
        mechanism_row(
            ui,
            "HOSTS FILE SYNC",
            cfg.me.sync_hosts_file,
            "Every application on this machine can resolve mesh names -- no config needed. \
             Registered names only (no wildcard subdomains -- a hosts file can't do that).",
        );
        let dns_detail = if cfg.me.dns_server {
            format!(
                "Listening on 127.0.0.1:{} -- point another device's DNS at this machine's \
                 mesh IP to resolve names from it too. Also answers ANY subdomain of a \
                 peer's own name (e.g. 'whatever.alice.{}') even if it was never \
                 explicitly registered as a service.",
                cfg.me.dns_port, cfg.me.domain_suffix
            )
        } else {
            "Off. Enable in mesh.toml (dns_server = true) for wildcard subdomain support / \
             resolving mesh names from other devices on your LAN."
                .to_string()
        };
        mechanism_row(ui, "BUILT-IN DNS RESOLVER", cfg.me.dns_server, &dns_detail);
    });

    ui.add_space(14.0);

    let entries = collect_entries(app, &cfg);

    section(ui, &format!("NAMES ({})", entries.len()), |ui| {
        if entries.is_empty() {
            ui.label(egui::RichText::new("No peers yet.").color(theme::TEXT_DIM));
            return;
        }
        for entry in &entries {
            let is_me = entry.virtual_ip == cfg.me.virtual_ip;
            draw_name_row(app, ui, entry, is_me);
            ui.add_space(2.0);
        }
    });

    ui.add_space(14.0);

    section(ui, "YOUR SERVICES", |ui| {
        ui.label(
            egui::RichText::new(
                "Advertise something you host (a game server, a web dashboard, ...) as its \
                 own subdomain of your mesh name. This is gossiped to the whole mesh \
                 automatically -- no manual sharing needed once you're connected.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(8.0);

        if cfg.services.is_empty() {
            ui.label(egui::RichText::new("No services advertised yet.").color(theme::TEXT_DIM).size(11.0));
        } else {
            for service in &cfg.services {
                let hostname = format!(
                    "{}.{}.{}",
                    lan_mesh_core::hosts::sanitize_label(&service.name),
                    lan_mesh_core::hosts::sanitize_label(&cfg.me.name),
                    cfg.me.domain_suffix
                );
                ui.horizontal(|ui| {
                    ui.set_min_height(24.0);
                    ui.label(egui::RichText::new(&hostname).monospace().color(theme::TEAL));
                    ui.label(
                        egui::RichText::new(format!("port {}", service.port))
                            .monospace()
                            .color(theme::TEXT_DIM)
                            .size(11.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("REMOVE").clicked() {
                            remove_service(app, &service.name);
                        }
                    });
                });
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("ADD A SERVICE").color(theme::TEXT_DIM).size(11.0).strong());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("NAME").color(theme::TEXT_DIM).size(10.5));
            ui.add(
                egui::TextEdit::singleline(&mut app.add_service_form.name)
                    .desired_width(120.0)
                    .hint_text("game"),
            );
            ui.label(egui::RichText::new("PORT").color(theme::TEXT_DIM).size(10.5));
            ui.add(
                egui::TextEdit::singleline(&mut app.add_service_form.port)
                    .desired_width(70.0)
                    .hint_text("25565"),
            );
            if ui.button("ADD").clicked() {
                add_service(app);
            }
        });
        if let Some(err) = &app.add_service_form.error {
            ui.colored_label(theme::RED, err);
        }
        if !matches!(app.mode, AppMode::Running { .. }) {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("(Saved to mesh.toml now; takes effect once the mesh starts.)")
                    .color(theme::TEXT_DIM)
                    .size(10.0),
            );
        }
    });

    ui.add_space(14.0);

    section(ui, "IN-APP BROWSER", |ui| {
        ui.label(
            egui::RichText::new(
                "Opens a real embedded browser window (its own visual identity, matching \
                 this app) for viewing anything a peer hosts on the mesh -- a game server's \
                 web admin panel, Plex/Jellyfin, a self-hosted dashboard, etc.",
            )
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(6.0);
        if ui
            .add(egui::Button::new(egui::RichText::new("OPEN BROWSER (MESH HOME)").color(theme::BG_DEEP)).fill(theme::TEAL))
            .clicked()
        {
            launch_browser(app, "");
        }
    });
}

fn draw_name_row(app: &mut App, ui: &mut egui::Ui, entry: &DomainNameEntry, is_me: bool) {
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        // Indent service subdomains slightly under their peer root so the
        // hierarchy is visually obvious at a glance.
        if !entry.is_peer_root {
            ui.add_space(18.0);
        }
        let color = if is_me { theme::TEAL } else { theme::TEXT_BRIGHT };
        let label = if entry.is_peer_root {
            egui::RichText::new(&entry.hostname).monospace().color(color).strong()
        } else {
            egui::RichText::new(format!("↳ {}", entry.hostname)).monospace().color(theme::AMBER)
        };
        ui.label(label);
        ui.label(egui::RichText::new(format!("-> {}", entry.virtual_ip)).monospace().color(theme::TEXT_DIM).size(11.0));
        if let Some(port) = entry.port {
            ui.label(egui::RichText::new(format!(":{port}")).monospace().color(theme::TEXT_DIM).size(11.0));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let target = match entry.port {
                Some(port) => format!("{}:{}", entry.hostname, port),
                None => entry.hostname.clone(),
            };
            if ui.button("OPEN IN BROWSER").clicked() {
                launch_browser(app, &target);
            }
            if ui.button("COPY").clicked() {
                ui.ctx().copy_text(entry.hostname.clone());
                app.show_toast(format!("Copied '{}'", entry.hostname));
            }
        });
    });
}

/// Builds the domain-name entry list to display: prefers the live mesh's
/// own view (accurate to the second, includes gossip-learned peer
/// services) when running, falls back to reconstructing it from the
/// static config when stopped/in setup (own services only -- peers'
/// services aren't known until the mesh actually runs and gossip
/// delivers them) so the screen is still useful before the mesh starts.
fn collect_entries(app: &App, cfg: &Config) -> Vec<DomainNameEntry> {
    if let AppMode::Running { handle, .. } = &app.mode {
        return handle.domain_snapshot();
    }

    let infos = vec![lan_mesh_core::hosts::PeerDomainInfo {
        name: cfg.me.name.clone(),
        virtual_ip: cfg.me.virtual_ip,
        services: cfg.services.iter().map(|s| (s.name.clone(), s.port)).collect(),
    }]
    .into_iter()
    .chain(cfg.peers.iter().map(|p| lan_mesh_core::hosts::PeerDomainInfo {
        name: p.name.clone(),
        virtual_ip: p.virtual_ip,
        services: Vec::new(),
    }))
    .collect::<Vec<_>>();

    lan_mesh_core::hosts::build_entries_with_services(&cfg.me.domain_suffix, &infos)
        .into_iter()
        .map(|e| DomainNameEntry {
            hostname: e.hostname,
            virtual_ip: e.virtual_ip,
            is_peer_root: e.is_peer_root,
            port: e.port,
        })
        .collect()
}

fn add_service(app: &mut App) {
    let name = app.add_service_form.name.trim().to_string();
    if name.is_empty() {
        app.add_service_form.error = Some("Service name can't be empty.".to_string());
        return;
    }
    let Ok(port) = app.add_service_form.port.trim().parse::<u16>() else {
        app.add_service_form.error = Some("Invalid port.".to_string());
        return;
    };

    match &app.mode {
        AppMode::Running { handle, .. } => {
            handle.add_service_live(&name, port);
            app.show_toast(format!("Service '{name}' added -- announcing to the mesh now."));
        }
        _ => {
            if let Some(mut cfg) = app.current_config() {
                if let Some(existing) = cfg.services.iter_mut().find(|s| s.name.eq_ignore_ascii_case(&name)) {
                    existing.port = port;
                } else {
                    cfg.services.push(lan_mesh_core::config::ServiceConfig { name: name.clone(), port });
                }
                let _ = cfg.save();
                app.show_toast(format!("Service '{name}' saved -- will announce once the mesh starts."));
            }
        }
    }
    app.add_service_form.name.clear();
    app.add_service_form.port.clear();
    app.add_service_form.error = None;
}

fn remove_service(app: &mut App, name: &str) {
    match &app.mode {
        AppMode::Running { handle, .. } => {
            handle.remove_service_live(name);
        }
        _ => {
            if let Some(mut cfg) = app.current_config() {
                cfg.services.retain(|s| !s.name.eq_ignore_ascii_case(name));
                let _ = cfg.save();
            }
        }
    }
    app.show_toast(format!("Removed service '{name}'."));
}

/// Launches the standalone `lan_mesh_browser` binary, expected to sit
/// next to this GUI executable (same directory, matching how the release
/// packaging bundles it -- see `.github/workflows/release.yml`). An
/// empty `target` opens the mesh home page instead of a specific
/// address.
fn launch_browser(app: &mut App, target: &str) {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_ref().and_then(|p| p.parent());
    let browser_name = if cfg!(windows) { "lan_mesh_browser.exe" } else { "lan_mesh_browser" };
    let browser_path = dir.map(|d| d.join(browser_name));

    let mut cmd = match &browser_path {
        Some(p) if p.exists() => std::process::Command::new(p),
        _ => std::process::Command::new(browser_name), // fall back to PATH lookup
    };
    if !target.is_empty() {
        cmd.arg(target);
    }
    match cmd.spawn() {
        Ok(_) => app.show_toast(if target.is_empty() {
            "Opening browser...".to_string()
        } else {
            format!("Opening {target} in browser...")
        }),
        Err(e) => app.show_toast(format!("Could not launch browser: {e}")),
    }
}

fn mechanism_row(ui: &mut egui::Ui, label: &str, enabled: bool, detail: &str) {
    ui.horizontal(|ui| {
        let (color, status) = if enabled { (theme::TEAL, "ENABLED") } else { (theme::TEXT_DIM, "DISABLED") };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, color);
        ui.add_space(4.0);
        ui.label(egui::RichText::new(label).color(theme::TEXT_BRIGHT).monospace().size(11.0).strong());
        ui.label(egui::RichText::new(status).color(color).monospace().size(10.5));
    });
    ui.label(egui::RichText::new(detail).color(theme::TEXT_DIM).size(10.5));
    ui.add_space(8.0);
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
