use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use meow-meow_core::config::Config;
use meow-meow_core::mesh::{self, MeshHandle, MeshSnapshot};

use crate::gui_settings::GuiSettings;
use crate::{tray, updater};

pub const MAX_LOG_LINES: usize = 500;

/// Shared log buffer that the mesh engine's log sink writes into, and the
/// GUI reads from every frame. `Arc<Mutex<..>>` because the sink closure
/// runs on the mesh's own background threads, not the GUI thread.
pub type LogBuffer = Arc<Mutex<VecDeque<String>>>;

pub fn new_log_buffer() -> LogBuffer {
    Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES)))
}

pub fn install_log_sink(buffer: LogBuffer) {
    meow-meow_core::logsink::set_sink(Box::new(move |line: &str| {
        let mut guard = buffer.lock().unwrap();
        guard.push_back(line.to_string());
        if guard.len() > MAX_LOG_LINES {
            guard.pop_front();
        }
    }));
}

#[derive(PartialEq, Clone, Copy)]
pub enum Screen {
    Network,
    Peers,
    Domains,
    Log,
    Settings,
}

#[derive(PartialEq, Clone, Copy)]
pub enum SetupStage {
    /// No mesh.toml exists yet / user chose to (re)configure identity.
    Identity,
    /// Identity saved; offer to add a bootstrap peer or start solo.
    Bootstrap,
}

pub struct AddPeerModal {
    pub open: bool,
    pub paste_buffer: String,
    pub error: Option<String>,
    pub my_card: Option<String>,
    pub card_error: Option<String>,
}

impl Default for AddPeerModal {
    fn default() -> Self {
        AddPeerModal {
            open: false,
            paste_buffer: String::new(),
            error: None,
            my_card: None,
            card_error: None,
        }
    }
}

/// Scratch state for the Domains screen's inline "advertise a service"
/// form (e.g. "game" on port 25565) -- kept separate from `AddPeerModal`
/// since it's a different concept (a service isn't a peer), but follows
/// the same "small owned scratch buffer + optional error" shape.
pub struct AddServiceForm {
    pub name: String,
    pub port: String,
    pub error: Option<String>,
}

impl Default for AddServiceForm {
    fn default() -> Self {
        AddServiceForm {
            name: String::new(),
            port: String::new(),
            error: None,
        }
    }
}

/// Scratch state for the Settings screen's "manually enter public
/// IP/port" form -- same shape as `AddServiceForm`, kept separate since
/// it's a conceptually different action.
pub struct ManualAddrForm {
    pub ip: String,
    pub port: String,
    pub error: Option<String>,
}

impl Default for ManualAddrForm {
    fn default() -> Self {
        ManualAddrForm {
            ip: String::new(),
            port: String::new(),
            error: None,
        }
    }
}

pub struct SetupForm {
    pub name: String,
    pub virtual_ip: String,
    pub prefix: String,
    pub listen_port: String,
    pub psk_input: String,
    pub generated_psk: Option<String>,
    pub domain_suffix: String,
    pub error: Option<String>,
}

impl Default for SetupForm {
    fn default() -> Self {
        SetupForm {
            name: "player".to_string(),
            virtual_ip: "10.66.0.1".to_string(),
            prefix: "24".to_string(),
            listen_port: "54321".to_string(),
            psk_input: String::new(),
            generated_psk: None,
            domain_suffix: "mesh".to_string(),
            error: None,
        }
    }
}

pub enum AppMode {
    /// No config on disk yet, or user is (re)running setup.
    Setup {
        stage: SetupStage,
        form: SetupForm,
        saved_config: Option<Config>,
    },
    /// Mesh is configured and (usually) running.
    Running {
        handle: MeshHandle,
        snapshot: MeshSnapshot,
        selected_peer: Option<std::net::Ipv4Addr>,
    },
    /// Configured but explicitly stopped by the user.
    Stopped { config: Config },
}

pub struct App {
    pub mode: AppMode,
    pub screen: Screen,
    pub logs: LogBuffer,
    pub add_peer_modal: AddPeerModal,
    pub add_service_form: AddServiceForm,
    pub manual_addr_form: ManualAddrForm,
    pub last_refresh: std::time::Instant,
    pub toast: Option<(String, std::time::Instant)>,
    /// GUI-only preferences (tray behavior, update checks), persisted to
    /// gui.toml next to mesh.toml.
    pub gui_settings: GuiSettings,
    /// System tray icon, when the platform/desktop provides one. Keeping
    /// it alive for the whole run is what keeps the icon visible.
    pub tray: Option<tray::Tray>,
    /// Shared self-updater state, written by updater worker threads and
    /// rendered by the Settings screen.
    pub updater: Arc<Mutex<updater::UpdaterState>>,
    /// Set by the updater once the swap script is running -- the main
    /// loop reacts by closing the window for real so the script can
    /// replace the binaries and relaunch.
    pub updater_quit: Arc<AtomicBool>,
    /// True when the app is about to exit for real (tray "Quit" or a
    /// finished update) -- suppresses the close-to-tray interception.
    pub pending_quit: bool,
    /// Launched with `--minimized` (autostart): start hidden in the
    /// tray instead of popping a window.
    pub start_hidden: bool,
    pub hide_done: bool,
    pub update_check_started: bool,
    pub updater_toast_shown: bool,
}

impl App {
    pub fn new(start_hidden: bool) -> Self {
        let logs = new_log_buffer();
        install_log_sink(logs.clone());

        let mode = if Config::exists() {
            match Config::load() {
                Ok(cfg) => start_mesh_or_stopped(cfg),
                Err(_) => AppMode::Setup {
                    stage: SetupStage::Identity,
                    form: SetupForm::default(),
                    saved_config: None,
                },
            }
        } else {
            AppMode::Setup {
                stage: SetupStage::Identity,
                form: SetupForm::default(),
                saved_config: None,
            }
        };

        App {
            mode,
            screen: Screen::Network,
            logs,
            add_peer_modal: AddPeerModal::default(),
            add_service_form: AddServiceForm::default(),
            manual_addr_form: ManualAddrForm::default(),
            last_refresh: std::time::Instant::now(),
            toast: None,
            gui_settings: GuiSettings::load(),
            tray: tray::build(),
            updater: updater::shared(),
            updater_quit: Arc::new(AtomicBool::new(false)),
            pending_quit: false,
            start_hidden,
            hide_done: false,
            update_check_started: false,
            updater_toast_shown: false,
        }
    }

    pub fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
    }

    pub fn refresh_snapshot(&mut self) {
        if let AppMode::Running { handle, snapshot, .. } = &mut self.mode {
            *snapshot = handle.snapshot();
        }
    }

    pub fn start_mesh(&mut self, config: Config) {
        match mesh::start(config.clone()) {
            Ok(handle) => {
                let snapshot = handle.snapshot();
                self.mode = AppMode::Running {
                    handle,
                    snapshot,
                    selected_peer: None,
                };
            }
            Err(e) => {
                self.show_toast(format!("Failed to start mesh: {e}"));
                self.mode = AppMode::Stopped { config };
            }
        }
    }

    pub fn stop_mesh(&mut self) {
        if let AppMode::Running { handle, .. } = &self.mode {
            handle.stop();
        }
        if let AppMode::Running { handle, .. } = &self.mode {
            let config = handle.config_snapshot();
            self.mode = AppMode::Stopped { config };
        }
    }

    pub fn current_config(&self) -> Option<Config> {
        match &self.mode {
            AppMode::Running { handle, .. } => Some(handle.config_snapshot()),
            AppMode::Stopped { config } => Some(config.clone()),
            AppMode::Setup { saved_config, .. } => saved_config.clone(),
        }
    }

    /// Saves `cfg` to disk AND updates whichever in-memory copy the
    /// current `AppMode` is holding, so a screen that edits config while
    /// the mesh is stopped/in setup (rather than live via a
    /// `MeshHandle::*_live` method) sees its own change reflected
    /// immediately instead of only after a restart/reload -- the same
    /// class of bug the one-sided-peer-visibility and Windows-self-STUN
    /// fixes were about, just in the GUI's own state instead of the mesh
    /// engine's.
    pub fn save_and_sync_config(&mut self, cfg: Config) {
        let _ = cfg.save();
        match &mut self.mode {
            AppMode::Stopped { config } => *config = cfg,
            AppMode::Setup { saved_config, .. } => *saved_config = Some(cfg),
            AppMode::Running { .. } => {
                // Live mode edits its own config through MeshHandle
                // methods instead (add_service_live, etc.) -- if this is
                // ever called while running, saving directly to disk
                // like this would just get overwritten the next time the
                // live mesh persists something of its own, so there's
                // nothing more to do here.
            }
        }
    }
}

fn start_mesh_or_stopped(cfg: Config) -> AppMode {
    match mesh::start(cfg.clone()) {
        Ok(handle) => {
            let snapshot = handle.snapshot();
            AppMode::Running {
                handle,
                snapshot,
                selected_peer: None,
            }
        }
        Err(_) => AppMode::Stopped { config: cfg },
    }
}
