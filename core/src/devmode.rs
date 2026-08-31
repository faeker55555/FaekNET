//! Dev mode: a remote-control channel that runs over the mesh's own virtual
//! adapter (full-IP routing).
//!
//! The daemon exposes a plain TCP RPC listener on `my_virtual_ip:49375`. A
//! connection can only arrive through the tunnel, so its source address is
//! already an authenticated peer's virtual IP (every packet passed the AEAD
//! PSK check at the UDP layer before reaching this socket). On top of that,
//! every command additionally requires *two-sided approval*:
//!
//! 1. The controlled side runs `meow-meow dev enable --operator NAME ...`
//!    locally, which writes a TTL-bound state file (`devmode.json`, next to
//!    `mesh.toml`) listing the allowed operator names (fail-closed: missing
//!    file, invalid JSON, expired timestamp or empty operator list all mean
//!    "refuse everything").
//! 2. The controlling side must explicitly type a typed command
//!    (`dev fetch <vip>`, `dev exec <vip> --cmd ...`) from the CLI — typing
//!    the target and the action is itself the approval gesture; there is no
//!    interactive confirm prompt, so nothing runs without an explicit human
//!    decision on each side.
//!
//! Both sides append audit lines to `devmode.log` (cwd-relative, like
//! `mesh.toml`; daemons are launched from their own directory).

use std::io::{self, BufRead, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Child, Command, Stdout, Stderr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Fixed RPC port, bound to our own virtual IP (only reachable through the tunnel).
pub const DEV_RPC_PORT: u16 = 49375;
/// Rolling dev tag that peers auto-fetch by default.
pub const DEV_DEFAULT_TAG: &str = "v0-dev";

const DEV_REPO: &str = "faeker55555/FaekNET";
const STATE_FILE: &str = "devmode.json";
const AUDIT_FILE: &str = "devmode.log";
const MAX_AUDIT_LINES: usize = 10_000;

const EXEC_CMD_MAX_BYTES: usize = 4096;
const EXEC_TIMEOUT_MIN_MS: u64 = 5_000;
const EXEC_TIMEOUT_MAX_MS: u64 = 600_000;
const EXEC_TIMEOUT_DEFAULT_MS: u64 = 120_000;
/// Total cap on combined exec stdout/stderr forwarded over the wire.
const OUTPUT_CAP_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Approval state (the file `dev enable` writes)
// ---------------------------------------------------------------------------

/// Two-sided approval record: written by the local human on the controlled
/// side via `meow-meow dev enable`, read by the RPC listener per connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    /// Unix seconds when approval was granted locally.
    pub granted_at: i64,
    /// Unix seconds after which this approval stops matching (None = no expiry).
    pub expires_at: Option<i64>,
    /// Operator names allowed to control this machine; `dev enable` requires
    /// at least one, and the listener enforces membership per connection.
    pub allowed_operators: Vec<String>,
}

impl Approval {
    pub fn is_expired(&self) -> bool {
        matches!(self.expires_at, Some(t) if now_unix() >= t)
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64
}

fn rfc3339_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Append one line to the audit log (cwd-relative), trimming the tail so the
/// file never grows past `MAX_AUDIT_LINES`. Best-effort: failures only print.
fn append_audit_raw(bytes: &[u8]) -> std::io::Result<()> {
    use std::fs::{File, OpenOptions};
    let mut f = OpenOptions::new().create(true).append(true).open(AUDIT_FILE)?;
    f.write_all(bytes)?;
    // Trim to the tail once we grow past a size where it is cheaply known.
    if let Ok(meta) = std::fs::metadata(AUDIT_FILE) {
        if meta.len() > (512 * MAX_AUDIT_LINES as u64) {
            let ok_trim = std::fs::read_to_string(AUDIT_FILE).map(|all| {
                let lines: Vec<&str> = all.lines().collect();
                if lines.len() > MAX_AUDIT_LINES + 500 {
                    let keep = lines.len() - MAX_AUDIT_LINES;
                    let trimmed = &lines[keep..];
                    std::fs::write(AUDIT_FILE, (trimmed.join("\n") + "\n").into_bytes())?;
                }
                Ok(())
            });
            if let Err(e) = ok_trim {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, e));
            }
        }
    }
    drop(File::open(AUDIT_FILE).map_err(|_| io::Error::new(io::ErrorKind::NotFound, "audit log unreadable"))?);
    Ok(())
}

fn audit(event: &str) {
    if let Err(e) = append_audit_raw(format!("{rfc3339_now} {event}\n").into_bytes()).as_ref() {
        eprintln!("warning: could not write dev-mode audit log: {e}");
        return;
    }
}

fn load_state() -> Option<Approval> {
    match std::fs::read_to_string(STATE_FILE) {
        Ok(s) => serde_json::from_str(&s).ok(),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Server half: RPC listener on our virtual IP
// ---------------------------------------------------------------------------

/// Spawns the dev-mode RPC listener (bound to `my_virtual_ip:{DEV_RPC_PORT}`).
/// Best-effort: if binding fails we log and retry a few times.
pub fn spawn(state: Arc<mesh::MeshState>, my_virtual_ip: Ipv4Addr, running: Arc<AtomicBool>) {
    thread::spawn(move || loop {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        match TcpListener_bind(my_virtual_ip) {
            Ok(listener) => {
                dlog(&format!("dev-mode RPC listener on {my_virtual_ip}:{DEV_RPC_PORT}"));
                server_loop(listener, state.clone());
                return; // listener died -> stop retrying (mesh is shutting down anyway)
            }
            Err(e) => {
                dlog(&format!(
                    "warning: dev-mode RPC could not bind {my_virtual_ip}:{DEV_RPC_PORT}: {e} (retry in 5s)"
                ));
                thread::sleep(Duration::from_secs(5));
            }
        }
    });
}

#[allow(unused)]
fn TcpListener_bind(ipv4: Ipv4Addr) -> Result<TcpStream, std::io::Error> { ... }
