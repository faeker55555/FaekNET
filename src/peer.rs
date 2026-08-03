use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::PeerConfig;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Runtime state for one mesh peer.
///
/// `confirmed_addr` starts out as whatever the config says (best guess from
/// the peer telling you their own public ip:port), but is updated whenever
/// we receive an authenticated packet claiming to be from this peer's
/// virtual IP -- similar to WireGuard's "roaming". This is what makes the
/// mesh resilient to the CGNAT port-remapping problem discussed earlier:
/// even if the configured port is stale/wrong, one successful inbound
/// packet (e.g. during the initial hole-punch burst) corrects it.
pub struct Peer {
    pub name: String,
    pub virtual_ip: std::net::Ipv4Addr,
    pub configured_addr_str: String,
    confirmed_addr: RwLock<Option<SocketAddr>>,
    pub last_seen_secs: AtomicU64,
}

impl Peer {
    pub fn new(cfg: &PeerConfig) -> Peer {
        Peer {
            name: cfg.name.clone(),
            virtual_ip: cfg.virtual_ip,
            configured_addr_str: format!("{}:{}", cfg.public_ip, cfg.public_port),
            confirmed_addr: RwLock::new(None),
            last_seen_secs: AtomicU64::new(0),
        }
    }

    /// Resolve the address we should currently be sending to: prefer a
    /// confirmed (roamed-to) address learned from real traffic, else fall
    /// back to resolving the configured hostname/IP:port from the config
    /// file (re-resolved each time in case it's a dynamic-DNS hostname).
    pub fn current_send_addr(&self) -> Option<SocketAddr> {
        if let Some(addr) = *self.confirmed_addr.read().unwrap() {
            return Some(addr);
        }
        self.configured_addr_str
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
    }

    /// Called whenever we receive and successfully authenticate a packet
    /// that claims to be from this peer. Updates the learned address and
    /// last-seen timestamp.
    pub fn observe(&self, addr: SocketAddr) {
        let mut guard = self.confirmed_addr.write().unwrap();
        if *guard != Some(addr) {
            *guard = Some(addr);
        }
        drop(guard);
        self.last_seen_secs.store(now_secs(), Ordering::Relaxed);
    }

    pub fn seconds_since_seen(&self) -> Option<u64> {
        let last = self.last_seen_secs.load(Ordering::Relaxed);
        if last == 0 {
            None
        } else {
            Some(now_secs().saturating_sub(last))
        }
    }
}
