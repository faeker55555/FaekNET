use eframe::egui;

use crate::app_state::{App, AppMode, Screen};
use crate::theme;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    draw_nav_rail(app, ctx);
    draw_topbar(app, ctx);

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::BG_DEEP).inner_margin(egui::Margin::same(0)))
        .show(ctx, |ui| match app.screen {
            Screen::Network => super::network_screen::draw(app, ui),
            Screen::Peers => super::peers_screen::draw(app, ui),
            Screen::Domains => super::domains_screen::draw(app, ui),
            Screen::Log => super::log_screen::draw(app, ui),
            Screen::Settings => super::settings_screen::draw(app, ui),
        });

    super::peer_modal::draw(app, ctx);
}

fn draw_nav_rail(app: &mut App, ctx: &egui::Context) {
    egui::SidePanel::left("nav_rail")
        .resizable(false)
        .exact_width(56.0)
        .frame(
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0f32, theme::LINE))
                .inner_margin(egui::Margin::symmetric(0, 12)),
        )
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                nav_button(ui, app, Screen::Network, "N", "Network overview");
                ui.add_space(4.0);
                nav_button(ui, app, Screen::Peers, "P", "Peers");
                ui.add_space(4.0);
                nav_button(ui, app, Screen::Domains, "D", "Local domain names / browser");
                ui.add_space(4.0);
                nav_button(ui, app, Screen::Log, "L", "Activity log");
                ui.add_space(4.0);
                nav_button(ui, app, Screen::Settings, "S", "Settings");
            });
        });
}

fn nav_button(ui: &mut egui::Ui, app: &mut App, screen: Screen, letter: &str, tooltip: &str) {
    let active = app.screen == screen;
    let (bg, fg) = if active {
        (theme::TEAL_DIM, theme::TEAL)
    } else {
        (theme::BG_ELEVATED, theme::TEXT_DIM)
    };
    let btn = egui::Button::new(egui::RichText::new(letter).monospace().size(15.0).color(fg))
        .fill(bg)
        .min_size(egui::vec2(36.0, 36.0));
    if ui.add(btn).on_hover_text(tooltip).clicked() {
        app.screen = screen;
    }
}

fn draw_topbar(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("topbar")
        .frame(
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0f32, theme::LINE))
                .inner_margin(egui::Margin::symmetric(16, 10)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (dot_color, status_text) = match &app.mode {
                    AppMode::Running { .. } => (theme::TEAL, "MESH ACTIVE"),
                    AppMode::Stopped { .. } => (theme::TEXT_DIM, "STOPPED"),
                    AppMode::Setup { .. } => (theme::AMBER, "SETUP"),
                };
                draw_dot(ui, dot_color);
                ui.label(
                    egui::RichText::new(status_text)
                        .color(dot_color)
                        .monospace()
                        .size(12.0)
                        .strong(),
                );

                ui.separator();

                if let Some(cfg) = app.current_config() {
                    ui.label(
                        egui::RichText::new(format!("{}  ·  {}", cfg.me.name, cfg.me.virtual_ip))
                            .color(theme::TEXT_NORMAL)
                            .monospace()
                            .size(12.0),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &app.mode {
                        AppMode::Running { .. } => {
                            if ui.button("■ STOP MESH").clicked() {
                                app.stop_mesh();
                            }
                        }
                        AppMode::Stopped { config } => {
                            let config = config.clone();
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("▶ START MESH").color(theme::BG_DEEP),
                                ).fill(theme::TEAL))
                                .clicked()
                            {
                                app.start_mesh(config);
                            }
                        }
                        AppMode::Setup { .. } => {}
                    }
                    if ui.button("+ ADD PEER").clicked() {
                        app.add_peer_modal.open = true;
                        app.add_peer_modal.error = None;
                    }
                });
            });
        });
}

pub fn draw_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let size = egui::vec2(9.0, 9.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color);
    ui.add_space(6.0);
}
