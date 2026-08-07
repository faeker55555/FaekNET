// Suppress the console window that would otherwise briefly flash / stay
// open behind the GUI on Windows (release builds only, so `cargo run` in
// debug still shows println!/log output in a terminal during development).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod theme;
mod ui;

use app_state::App;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("lan_mesh"),
        ..Default::default()
    };

    eframe::run_native(
        "lan_mesh",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new()))
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

        ui::draw(self, ctx);
    }
}
