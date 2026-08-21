// GUI-only preferences, persisted to `gui.toml` next to `mesh.toml`
// (i.e. in the directory the GUI is launched from -- the same portable
// layout the rest of meow-meow uses).
//
// Deliberately a *separate* file rather than a section inside mesh.toml:
// the mesh engine rewrites mesh.toml wholesale whenever it persists a
// change (peer learned, service added, cached address, ...), which would
// silently drop a `[gui]` section it doesn't know about.

use serde::{Deserialize, Serialize};

pub const GUI_SETTINGS_PATH: &str = "gui.toml";

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct GuiSettings {
    /// Closing (or minimizing) the window hides it to the system tray
    /// instead of quitting; the mesh keeps running. The tray's own
    /// "Quit" menu item always exits for real.
    pub close_to_tray: bool,
    /// Ask GitHub for a newer release once per launch, shortly after
    /// startup. Results land on the Settings screen (and as a toast when
    /// an update is available).
    pub check_updates_on_start: bool,
}

impl Default for GuiSettings {
    fn default() -> Self {
        GuiSettings {
            close_to_tray: true,
            check_updates_on_start: true,
        }
    }
}

impl GuiSettings {
    pub fn load() -> GuiSettings {
        std::fs::read_to_string(GUI_SETTINGS_PATH)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(GUI_SETTINGS_PATH, s);
        }
    }
}
