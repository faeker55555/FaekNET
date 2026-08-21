// Suppress the console window that would otherwise briefly flash / stay
// open behind the GUI on Windows (release builds only, so `cargo run` in
// debug still shows println!/log output in a terminal during development).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod autostart;
mod gui_settings;
mod theme;
mod tray;
mod ui;
mod updater;

use std::sync::atomic::Ordering;

use app_state::App;
use eframe::egui;

/// mesh.toml lives in the process working directory (portable layout).
/// When lan_mesh is launched from elsewhere -- e.g. an autostart entry
/// that runs the binary by absolute path -- fall back to the executable's
/// own directory so the existing mesh.toml is still found.
fn ensure_config_cwd() {
    use std::path::Path;
    if Path::new("mesh.toml").exists() {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("mesh.toml").exists() {
                let _ = std::env::set_current_dir(dir);
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    ensure_config_cwd();

    // --minimized: used by the autostart entry -- start in the tray
    // without showing a window.
    let start_hidden = std::env::args().any(|a| a == "--minimized");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("мяу-мяу"),
        ..Default::default()
    };

    eframe::run_native(
        "мяу-мяу",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new(start_hidden)))
        }),
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll the live mesh state roughly 4x/second -- cheap (just reads
        // atomics/locks a couple of RwLocks briefly) and keeps peer
        // status/RTT/log display current without needing any async
        // plumbing between the mesh's background threads and the GUI.
        if self.last_refresh.elapsed() > std::time::Duration::from_millis(250) {
            self.refresh_snapshot();
            self.last_refresh = std::time::Instant::now();
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        // ---- Updater finished: the swap script is running, exit now ----
        if self.updater_quit.load(Ordering::Relaxed) && !self.pending_quit {
            self.pending_quit = true;
            self.show_toast("Update installed -- restarting...");
        }
        if self.pending_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // ---- Tray menu / click commands ----
        if let Some(tray) = &self.tray {
            while let Ok(cmd) = tray.commands.try_recv() {
                match cmd {
                    tray::TrayCommand::Show => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    }
                    tray::TrayCommand::Quit => {
                        self.pending_quit = true;
                    }
                }
            }
        }

        // ---- Close to tray: X hides the window, mesh keeps running ----
        if ctx.input(|i| i.viewport().close_requested()) && !self.pending_quit {
            if self.gui_settings.close_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // ---- Minimize to tray (same setting as close-to-tray) ----
        // Note: under Wayland, hiding a window is not supported by the
        // compositor protocol, so this degrades to a normal minimize.
        if self.gui_settings.close_to_tray && self.tray.is_some() {
            if let Some(true) = ctx.input(|i| i.viewport().minimized) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            }
        }

        // ---- Autostart launch: begin hidden in the tray ----
        if self.start_hidden && !self.hide_done {
            self.hide_done = true;
            if self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                // No tray on this system -- hiding would strand the user,
                // so stay visible instead.
                self.start_hidden = false;
            }
        }

        // ---- One startup update check, if enabled in gui.toml ----
        if !self.update_check_started {
            self.update_check_started = true;
            if self.gui_settings.check_updates_on_start {
                updater::start_check(self.updater.clone());
            }
        }

        // ---- Toast once when an update turns out to be available ----
        {
            let status = self.updater.lock().unwrap().status.clone();
            match status {
                updater::UpdateStatus::Available { version } => {
                    if !self.updater_toast_shown {
                        self.updater_toast_shown = true;
                        self.show_toast(format!(
                            "Update {version} available -- see Settings > Updates"
                        ));
                    }
                }
                _ => self.updater_toast_shown = false,
            }
        }

        ui::draw(self, ctx);
    }
}
