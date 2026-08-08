//! Local mesh domain names screen: shows every peer's `<name>.<suffix>`
//! address, whether hosts-file sync / the built-in DNS resolver are
//! enabled, and one-click shortcuts to open a name in the in-app browser
//! (a separate `lan_mesh_browser` process -- see that crate's Cargo.toml
//! for why it isn't an embedded egui panel).

use eframe::egui;
use lan_mesh_core::config::Config;

use crate::app_state::{App, AppMode};
use crate::theme;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 32.0).min(720.0));
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
                 virtual IP. No internet DNS involved; this is purely local to your mesh.",
                cfg.me.domain_suffix
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
            "Every application on this machine can resolve mesh names -- no config needed.",
        );
        let dns_detail = if cfg.me.dns_server {
            format!(
                "Listening on 127.0.0.1:{} -- point another device's DNS at this \
                 machine's mesh IP to resolve names from it too.",
                cfg.me.dns_port
            )
        } else {
            "Off. Enable in mesh.toml (dns_server = true) for subdomain support / \
             resolving mesh names from other devices on your LAN."
                .to_string()
        };
        mechanism_row(ui, "BUILT-IN DNS RESOLVER", cfg.me.dns_server, &dns_detail);
    });

    ui.add_space(14.0);

    let names = collect_names(app, &cfg);

    section(ui, &format!("NAMES ({})", names.len()), |ui| {
        if names.is_empty() {
            ui.label(egui::RichText::new("No peers yet.").color(theme::TEXT_DIM));
            return;
        }
        for (name, ip, is_me) in &names {
            ui.horizontal(|ui| {
                ui.set_min_height(28.0);
                let color = if *is_me { theme::TEAL } else { theme::TEXT_BRIGHT };
                ui.label(egui::RichText::new(name).monospace().color(color).strong());
                ui.label(egui::RichText::new(format!("-> {ip}")).monospace().color(theme::TEXT_DIM).size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OPEN IN BROWSER").clicked() {
                        launch_browser(app, name);
                    }
                    if ui.button("COPY").clicked() {
                        ui.ctx().copy_text(name.clone());
                        app.show_toast(format!("Copied '{name}'"));
                    }
                });
            });
            ui.add_space(2.0);
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

/// Builds the (name, virtual_ip, is_self) list to display: prefers the
/// live mesh's own view (accurate to the second) when running, falls
/// back to reconstructing it from the static config when stopped/in
/// setup so the screen is still useful before the mesh is started.
fn collect_names(app: &App, cfg: &Config) -> Vec<(String, std::net::Ipv4Addr, bool)> {
    if let AppMode::Running { handle, .. } = &app.mode {
        return handle
            .domain_snapshot()
            .into_iter()
            .map(|(name, ip)| {
                let is_me = ip == cfg.me.virtual_ip;
                (name, ip, is_me)
            })
            .collect();
    }

    let mut raw: Vec<(String, std::net::Ipv4Addr)> = vec![(cfg.me.name.clone(), cfg.me.virtual_ip)];
    for p in &cfg.peers {
        raw.push((p.name.clone(), p.virtual_ip));
    }
    lan_mesh_core::hosts::build_entries(&cfg.me.domain_suffix, &raw)
        .into_iter()
        .map(|e| {
            let is_me = e.virtual_ip == cfg.me.virtual_ip;
            (e.hostname, e.virtual_ip, is_me)
        })
        .collect()
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
