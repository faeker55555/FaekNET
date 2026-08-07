use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use lan_mesh_core::config::Config;
use lan_mesh_core::mesh::{self, MeshHandle, MeshSnapshot};

pub const MAX_LOG_LINES: usize = 500;

/// Shared log buffer that the mesh engine's log sink writes into, and the
/// GUI reads from every frame. `Arc<Mutex<..>>` because the sink closure
/// runs on the mesh's own background threads, not the GUI thread.
pub type LogBuffer = Arc<Mutex<VecDeque<String>>>;

pub fn new_log_buffer() -> LogBuffer {
    Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES)))
}

pub fn install_log_sink(buffer: LogBuffer) {
    lan_mesh_core::logsink::set_sink(Box::new(move |line: &str| {
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

pub struct SetupForm {
    pub name: String,
    pub virtual_ip: String,
    pub prefix: String,
    pub listen_port: String,
    pub psk_input: String,
    pub generated_psk: Option<String>,
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
    pub last_refresh: std::time::Instant,
    pub toast: Option<(String, std::time::Instant)>,
}

impl App {
    pub fn new() -> Self {
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
            last_refresh: std::time::Instant::now(),
            toast: None,
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
