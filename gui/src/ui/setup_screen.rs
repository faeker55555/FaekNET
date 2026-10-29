use std::net::Ipv4Addr;
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui;
use meow_meow_core::config::Config;
use meow_meow_core::setup::{self, Live, Location};

use crate::app_state::{App, AppMode, SetupOutcome, SetupStage};
use crate::theme;

/// What a button asked for. Applied after drawing, so no borrows overlap.
enum Action {
    None,
    Go(SetupStage),
    Create,
    Join,
    Copy(String),
    StartSaved,
}

pub fn draw(app: &mut App, ctx: &egui::Context) {
    // Finish a background job before drawing, so this frame already shows its result.
    if let Some(config) = poll_job(app) {
        app.start_mesh(config);
    }
    if matches!(&app.mode, AppMode::Setup { form, .. } if form.job.is_some()) {
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    let mut action = Action::None;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::BG_DEEP))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.heading(egui::RichText::new("MEOW_MEOW").color(theme::TEAL).size(28.0).monospace());
                ui.label(
                    egui::RichText::new("pure peer-to-peer virtual subnet -- no central server")
                        .color(theme::TEXT_DIM)
                        .size(12.0),
                );
                ui.add_space(28.0);
                egui::Frame::new()
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(480.0);
                        action = draw_card(app, ui);
                    });
            });
        });

    apply(app, ctx, action);
}

fn draw_card(app: &mut App, ui: &mut egui::Ui) -> Action {
    let AppMode::Setup { stage, form, .. } = &mut app.mode else {
        return Action::None;
    };

    match *stage {
        SetupStage::Welcome => {
            ui.label(egui::RichText::new("WELCOME").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Create a network for your friends, or join one they made.")
                    .color(theme::TEXT_NORMAL)
                    .size(13.0),
            );
            ui.add_space(18.0);
            if big_button(ui, "CREATE A NETWORK", theme::TEAL, theme::BG_DEEP) {
                return Action::Go(SetupStage::Create);
            }
            ui.label(
                egui::RichText::new("You become the first member and get an invite to send to friends.")
                    .color(theme::TEXT_DIM)
                    .size(11.0),
            );
            ui.add_space(12.0);
            if big_button(ui, "JOIN A NETWORK", theme::AMBER, theme::BG_DEEP) {
                return Action::Go(SetupStage::Join);
            }
            ui.label(
                egui::RichText::new("Paste the invite a friend sent you. Everything else is fetched automatically.")
                    .color(theme::TEXT_DIM)
                    .size(11.0),
            );
            Action::None
        }

        SetupStage::Create => {
            ui.label(egui::RichText::new("CREATE A NETWORK").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(8.0);
            labeled_field(ui, "YOUR NAME", &mut form.name, "how friends see you");
            labeled_field(ui, "VIRTUAL IP (optional)", &mut form.virtual_ip, "leave empty for 10.66.0.1");
            error_line(ui, &form.error);
            ui.add_space(14.0);
            let mut out = Action::None;
            ui.horizontal(|ui| {
                if ui.add_sized([120.0, 36.0], egui::Button::new("BACK")).clicked() {
                    out = Action::Go(SetupStage::Welcome);
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::new(egui::RichText::new("CREATE").color(theme::BG_DEEP)).fill(theme::TEAL),
                    )
                    .clicked()
                {
                    out = Action::Create;
                }
            });
            out
        }

        SetupStage::Join => {
            ui.label(egui::RichText::new("JOIN A NETWORK").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(8.0);
            ui.label(egui::RichText::new("INVITE").color(theme::TEXT_DIM).size(11.0));
            ui.add(
                egui::TextEdit::multiline(&mut form.invite_input)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("FNNET1:..."),
            );
            ui.add_space(6.0);
            labeled_field(ui, "YOUR NAME", &mut form.name, "how friends see you");
            labeled_field(ui, "VIRTUAL IP (optional)", &mut form.virtual_ip, "leave empty and the app picks a free one");
            error_line(ui, &form.error);
            ui.add_space(14.0);
            let mut out = Action::None;
            ui.horizontal(|ui| {
                if ui.add_sized([120.0, 36.0], egui::Button::new("BACK")).clicked() {
                    out = Action::Go(SetupStage::Welcome);
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::new(egui::RichText::new("JOIN").color(theme::BG_DEEP)).fill(theme::AMBER),
                    )
                    .clicked()
                {
                    out = Action::Join;
                }
            });
            out
        }

        SetupStage::Working => {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.add_space(10.0);
                let text = match form.return_to {
                    SetupStage::Create => "Creating your network...",
                    _ => "Fetching the network members...",
                };
                ui.label(egui::RichText::new(text).color(theme::TEXT_NORMAL).size(13.0));
                ui.label(
                    egui::RichText::new("Nothing is saved until this succeeds.").color(theme::TEXT_DIM).size(11.0),
                );
            });
            ui.add_space(20.0);
            Action::None
        }

        SetupStage::Created => {
            ui.label(egui::RichText::new("YOUR NETWORK IS READY").color(theme::TEAL).size(11.0));
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Send this invite to your friends. It is the only thing they need.")
                    .color(theme::TEXT_NORMAL)
                    .size(12.5),
            );
            ui.add_space(8.0);
            let invite = form.invite_out.clone().unwrap_or_default();
            egui::Frame::new()
                .fill(theme::BG_INPUT)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&invite).color(theme::AMBER).monospace().size(11.0))
                            .wrap()
                            .selectable(true),
                    );
                });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "The invite includes the network key and the write key for the member list. \
                     Share it only with people you trust. Keep a copy: it cannot be shown again.",
                )
                .color(theme::TEXT_DIM)
                .size(10.5),
            );
            ui.add_space(14.0);
            let mut out = Action::None;
            ui.horizontal(|ui| {
                if ui.add_sized([140.0, 36.0], egui::Button::new("COPY INVITE")).clicked() {
                    out = Action::Copy(invite.clone());
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::new(egui::RichText::new("START NETWORK").color(theme::BG_DEEP)).fill(theme::TEAL),
                    )
                    .clicked()
                {
                    out = Action::StartSaved;
                }
            });
            out
        }
    }
}

fn apply(app: &mut App, ctx: &egui::Context, action: Action) {
    match action {
        Action::None => {}
        Action::Go(stage) => {
            if let AppMode::Setup { stage: s, form, .. } = &mut app.mode {
                form.error = None;
                *s = stage;
            }
        }
        Action::Copy(text) => {
            ctx.copy_text(text);
            app.show_toast("Invite copied");
        }
        Action::StartSaved => {
            let config = match &app.mode {
                AppMode::Setup { saved_config: Some(c), .. } => Some(c.clone()),
                _ => None,
            };
            if let Some(config) = config {
                app.start_mesh(config);
            }
        }
        Action::Create => start_create(app),
        Action::Join => start_join(app),
    }
}

/// Parses the optional virtual IP field. Empty means "let the app pick".
fn parse_vip(text: &str) -> Result<Option<Ipv4Addr>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<Ipv4Addr>()
        .map(Some)
        .map_err(|_| "The virtual IP is not valid. Leave it empty or type something like 10.66.0.5.".to_string())
}

fn start_create(app: &mut App) {
    let AppMode::Setup { stage, form, .. } = &mut app.mode else {
        return;
    };
    let vip = match parse_vip(&form.virtual_ip) {
        Ok(v) => v,
        Err(e) => {
            form.error = Some(e);
            return;
        }
    };
    let name = form.name.clone();
    let (tx, rx) = mpsc::channel();
    form.job = Some(rx);
    form.return_to = SetupStage::Create;
    form.error = None;
    *stage = SetupStage::Working;
    std::thread::spawn(move || {
        let outcome = match setup::create_network(&Live, &Location::current_dir(), &name, vip) {
            Ok(invite) => match Config::load() {
                Ok(config) => SetupOutcome::Created { invite, config },
                Err(e) => SetupOutcome::Failed(format!(
                    "The network was created, but reading {} failed ({e}). Keep this invite: {invite}",
                    meow_meow_core::config::CONFIG_PATH
                )),
            },
            Err(e) => SetupOutcome::Failed(e),
        };
        let _ = tx.send(outcome);
    });
}

fn start_join(app: &mut App) {
    let AppMode::Setup { stage, form, .. } = &mut app.mode else {
        return;
    };
    let vip = match parse_vip(&form.virtual_ip) {
        Ok(v) => v,
        Err(e) => {
            form.error = Some(e);
            return;
        }
    };
    let invite = form.invite_input.clone();
    let name = form.name.clone();
    let (tx, rx) = mpsc::channel();
    form.job = Some(rx);
    form.return_to = SetupStage::Join;
    form.error = None;
    *stage = SetupStage::Working;
    std::thread::spawn(move || {
        let outcome = match setup::join_network(&Live, &Location::current_dir(), &invite, &name, vip) {
            Ok(config) => SetupOutcome::Joined { config },
            Err(e) => SetupOutcome::Failed(e),
        };
        let _ = tx.send(outcome);
    });
}

/// Takes a finished background job, if any. Returns a config when the mesh
/// should start right away (a successful join).
fn poll_job(app: &mut App) -> Option<Config> {
    let AppMode::Setup { stage, form, saved_config } = &mut app.mode else {
        return None;
    };
    let rx = form.job.as_ref()?;
    let outcome = match rx.try_recv() {
        Ok(outcome) => outcome,
        Err(mpsc::TryRecvError::Empty) => return None,
        Err(mpsc::TryRecvError::Disconnected) => {
            SetupOutcome::Failed("Setup stopped unexpectedly. Nothing was saved.".to_string())
        }
    };
    form.job = None;
    match outcome {
        SetupOutcome::Created { invite, config } => {
            form.invite_out = Some(invite);
            *saved_config = Some(config);
            *stage = SetupStage::Created;
            None
        }
        SetupOutcome::Joined { config } => {
            form.invite_input.clear();
            form.error = None;
            Some(config)
        }
        SetupOutcome::Failed(e) => {
            form.error = Some(e);
            *stage = form.return_to;
            None
        }
    }
}

fn big_button(ui: &mut egui::Ui, label: &str, fill: egui::Color32, text: egui::Color32) -> bool {
    ui.add_sized(
        [ui.available_width(), 44.0],
        egui::Button::new(egui::RichText::new(label).color(text).strong().size(14.0)).fill(fill),
    )
    .clicked()
}

fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(egui::RichText::new(label).color(theme::TEXT_DIM).size(11.0));
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .hint_text(hint),
    );
    ui.add_space(6.0);
}

fn error_line(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(err) = error {
        ui.add_space(6.0);
        ui.colored_label(theme::RED, err);
    }
}
