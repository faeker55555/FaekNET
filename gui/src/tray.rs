// System tray icon (Linux + Windows), built on the `tray-icon` crate.
//
// The tray is created once at startup, on the GUI's main thread, and is
// kept alive for the whole process lifetime (dropping the `TrayIcon`
// removes the icon). Menu / click events arrive on the tray library's
// own threads, so they are marshalled into `TrayCommand`s through a
// plain std channel that the eframe update loop drains every frame.
//
// - Linux: tray-icon drives libappindicator (or ayatana-appindicator,
//   whichever the desktop provides), which runs its own GTK main loop in
//   a background thread -- no interaction with eframe/winit's loop.
// - Windows: a hidden message-only window on the GUI thread receives the
//   Shell_NotifyIcon callbacks; winit's message pump dispatches them, the
//   same pattern tauri uses.

use std::sync::mpsc::{channel, Receiver};

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, TrayIconBuilder, TrayIconEvent};

/// Commands sent from the tray icon's menu / click handler to the GUI.
pub enum TrayCommand {
    /// Restore and focus the main window.
    Show,
    /// Quit the whole application (mesh included).
    Quit,
}

pub struct Tray {
    pub commands: Receiver<TrayCommand>,
    _tray: tray_icon::TrayIcon,
}

/// Builds the tray icon. Returns `None` when the current system can't
/// provide one (no tray host on Linux, icon creation failure, ...) -- the
/// GUI then simply runs without a tray and the close-to-tray behavior
/// degrades to a normal close.
pub fn build() -> Option<Tray> {
    let icon = Icon::from_rgba(include_bytes!("../assets/tray_icon.rgba").to_vec(), 32, 32).ok()?;

    let menu = Menu::new();
    let show_item = MenuItem::new("Open lan_mesh", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    if menu.append(&show_item).is_err() {
        return None;
    }
    if menu.append(&quit_item).is_err() {
        return None;
    }
    let show_id = show_item.id().clone();
    let quit_id = quit_item.id().clone();

    let (tx, rx) = channel::<TrayCommand>();

    let tx_menu = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == show_id {
            let _ = tx_menu.send(TrayCommand::Show);
        } else if event.id == quit_id {
            let _ = tx_menu.send(TrayCommand::Quit);
        }
    }));

    let tx_click = tx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            ..
        } = event
        {
            let _ = tx_click.send(TrayCommand::Show);
        }
    }));

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("lan_mesh")
        .with_icon(icon)
        .build()
        .ok()?;

    Some(Tray {
        commands: rx,
        _tray: tray,
    })
}
