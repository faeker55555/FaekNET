use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::config::PeerConfig;

pub fn now_secs() -> u64 {
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
/// virtual IP -- similar to WireGuard's "roaming". This is also updated by
/// gossip from other peers when their information is fresher than ours
/// (see `observe_epoch`), which is what lets the mesh self-heal when a
/// peer's NAT reassigns a new port and only some peers have noticed yet.
pub struct Peer {
    name: RwLock<String>,
    pub virtual_ip: std::net::Ipv4Addr,
    pub configured_addr_str: String,
    confirmed_addr: RwLock<Option<SocketAddr>>,
    /// Unix epoch seconds of the freshest information we have about this
    /// peer's address, whichever source it came from (direct observation
    /// always stamps "now", gossip carries the origin's own timestamp).
    /// Freshest-wins is the sole conflict-resolution rule -- this is what
    /// lets a still-valid link to a third peer repair a broken link
    /// between two others, without any central authority.
    confirmed_epoch: AtomicU32,
    pub last_seen_secs: AtomicU64,
    /// True once this peer was learned via gossip rather than configured
    /// directly (export/import or add-peer) -- purely informational, shown
    /// in status output so it's clear the auto-discovery is working.
    pub discovered_via_gossip: std::sync::atomic::AtomicBool,
    /// Most recently measured round-trip latency, in whole milliseconds.
    /// -1 means "no measurement yet".
    pub last_rtt_ms: AtomicI64,
    /// Pending outstanding pings: sequence number -> time sent, so `pong`
    /// handling can compute an RTT and detect stale/duplicate replies.
    pub pending_pings: Mutex<std::collections::HashMap<u64, Instant>>,
    /// Named services this peer has advertised via gossip (service name,
    /// port), e.g. `[("game", 25565), ("voice", 7777)]`. Learned purely
    /// from `TYPE_GOSSIP` service-announcement entries -- there's no
    /// direct-configuration equivalent since services are always
    /// self-advertised by the machine that hosts them, not something a
    /// friend types in about someone else.
    services: RwLock<Vec<(String, u16)>>,
    /// This peer's self-reported LAN-facing address (e.g.
    /// 192.168.1.20:54321), learned via gossip (see
    /// `gossip::GossipEntry::lan_addr`). Kept entirely separate from
    /// `confirmed_addr`: it's a *candidate* to additionally probe, never
    /// something `current_send_addr()` returns directly, since it's only
    /// useful when we're actually on the same LAN as the peer -- which we
    /// can't know in advance, only by trying it and seeing if a PONG ever
    /// comes back from it. See `mesh.rs`'s keepalive/hole-punch ticker,
    /// which sends a PING to both this and the normal send address
    /// whenever both exist; ordinary `observe()` on whichever one
    /// actually answers is what promotes it to `confirmed_addr` -- no
    /// separate "prefer LAN" logic is needed, direct observation already
    /// always wins.
    lan_candidate: RwLock<Option<SocketAddr>>,
}

impl Peer {
    pub fn new(cfg: &PeerConfig) -> Peer {
        Peer {
            name: RwLock::new(cfg.name.clone()),
            virtual_ip: cfg.virtual_ip,
            configured_addr_str: format!("{}:{}", cfg.public_ip, cfg.public_port),
            confirmed_addr: RwLock::new(None),
            confirmed_epoch: AtomicU32::new(0),
            last_seen_secs: AtomicU64::new(0),
            discovered_via_gossip: std::sync::atomic::AtomicBool::new(false),
            last_rtt_ms: AtomicI64::new(-1),
            pending_pings: Mutex::new(std::collections::HashMap::new()),
            services: RwLock::new(Vec::new()),
            lan_candidate: RwLock::new(None),
        }
    }

    /// Builds a Peer purely from gossip, with no prior config entry at
    /// all -- this is the "auto connection of peers" case: we've never
    /// heard of this virtual IP before, but a peer we trust (already
    /// authenticated with our shared key) just told us about it.
    pub fn from_gossip(virtual_ip: std::net::Ipv4Addr, name: &str, addr: SocketAddr, epoch_secs: u32) -> Peer {
        Peer {
            name: RwLock::new(name.to_string()),
            virtual_ip,
            configured_addr_str: addr.to_string(),
            confirmed_addr: RwLock::new(Some(addr)),
            confirmed_epoch: AtomicU32::new(epoch_secs),
            last_seen_secs: AtomicU64::new(0),
            discovered_via_gossip: std::sync::atomic::AtomicBool::new(true),
            last_rtt_ms: AtomicI64::new(-1),
            pending_pings: Mutex::new(std::collections::HashMap::new()),
            services: RwLock::new(Vec::new()),
            lan_candidate: RwLock::new(None),
        }
    }

    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }

    pub fn set_name(&self, name: &str) {
        if !name.is_empty() {
            *self.name.write().unwrap() = name.to_string();
        }
    }

    /// Resolve the address we should currently be sending to: prefer a
    /// confirmed (roamed-to or gossip-learned) address, else fall back to
    /// resolving the configured hostname/IP:port from the config file
    /// (re-resolved each time in case it's a dynamic-DNS hostname).
    pub fn current_send_addr(&self) -> Option<SocketAddr> {
        if let Some(addr) = *self.confirmed_addr.read().unwrap() {
            return Some(addr);
        }
        self.configured_addr_str
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
    }

    pub fn confirmed_epoch(&self) -> u32 {
        self.confirmed_epoch.load(Ordering::Relaxed)
    }

    /// The most recent LAN-facing address this peer has gossiped about
    /// itself, if any -- see the `lan_candidate` field's doc comment for
    /// why this is separate from `current_send_addr()`.
    pub fn lan_candidate(&self) -> Option<SocketAddr> {
        *self.lan_candidate.read().unwrap()
    }

    /// Records a freshly gossiped LAN candidate for this peer. Always
    /// overwrites (unlike `confirmed_addr`, there's no freshness-epoch
    /// tracking here) -- a LAN candidate that's gone stale is harmless to
    /// keep probing since it's never used unless something actually
    /// answers on it, so there's no correctness reason to gate updates.
    pub fn set_lan_candidate(&self, addr: Option<SocketAddr>) {
        *self.lan_candidate.write().unwrap() = addr;
    }

    /// Called whenever we receive and successfully authenticate a packet
    /// that claims to be from this peer. Direct observation always wins
    /// (stamped with the current time), since nothing is fresher evidence
    /// of a peer's real address than a packet that just arrived from it.
    /// Returns true if the confirmed address actually changed (including
    /// the first time it's ever learned), so callers can decide whether
    /// it's worth persisting to disk / re-gossiping immediately.
    pub fn observe(&self, addr: SocketAddr) -> bool {
        let now = now_secs() as u32;
        let mut guard = self.confirmed_addr.write().unwrap();
        let changed = *guard != Some(addr);
        if changed {
            *guard = Some(addr);
        }
        drop(guard);
        self.confirmed_epoch.store(now, Ordering::Relaxed);
        self.last_seen_secs.store(now_secs(), Ordering::Relaxed);
        changed
    }

    /// Called when gossip from another peer claims an address for this
    /// peer with a given freshness epoch. Only applied if strictly fresher
    /// than what we already have, so direct observation and more-recent
    /// gossip both correctly take precedence over stale information.
    /// Returns true if this updated our stored address.
    pub fn observe_epoch(&self, addr: SocketAddr, epoch_secs: u32) -> bool {
        if epoch_secs <= self.confirmed_epoch.load(Ordering::Relaxed) {
            return false;
        }
        let mut guard = self.confirmed_addr.write().unwrap();
        let changed = *guard != Some(addr);
        *guard = Some(addr);
        drop(guard);
        self.confirmed_epoch.store(epoch_secs, Ordering::Relaxed);
        changed
    }

    /// Records that a PING with the given sequence number was just sent,
    /// so a later matching PONG can compute round-trip time.
    pub fn record_ping_sent(&self, seq: u64) {
        let mut pending = self.pending_pings.lock().unwrap();
        pending.insert(seq, Instant::now());
        // Avoid unbounded growth if pongs never arrive (peer offline etc.)
        if pending.len() > 64 {
            if let Some(&oldest_seq) = pending.keys().min() {
                pending.remove(&oldest_seq);
            }
        }
    }

    /// Records a PONG reply for sequence number `seq`. Returns the
    /// measured round-trip time in milliseconds, if `seq` was a ping we
    /// actually sent (and haven't already matched).
    pub fn record_pong_received(&self, seq: u64) -> Option<u64> {
        let sent_at = self.pending_pings.lock().unwrap().remove(&seq)?;
        let rtt_ms = sent_at.elapsed().as_millis() as u64;
        self.last_rtt_ms.store(rtt_ms as i64, Ordering::Relaxed);
        Some(rtt_ms)
    }

    pub fn last_rtt_ms(&self) -> Option<u64> {
        let v = self.last_rtt_ms.load(Ordering::Relaxed);
        if v < 0 {
            None
        } else {
            Some(v as u64)
        }
    }

    pub fn seconds_since_seen(&self) -> Option<u64> {
        let last = self.last_seen_secs.load(Ordering::Relaxed);
        if last == 0 {
            None
        } else {
            Some(now_secs().saturating_sub(last))
        }
    }

    /// Current snapshot of this peer's advertised services.
    pub fn services(&self) -> Vec<(String, u16)> {
        self.services.read().unwrap().clone()
    }

    /// Merges in a service announcement learned via gossip: adds it if
    /// new, updates the port if it changed, no-ops if already known and
    /// unchanged. Returns true if the table actually changed (worth
    /// logging/re-syncing domain names over).
    pub fn observe_service(&self, service_name: &str, port: u16) -> bool {
        if service_name.is_empty() {
            return false;
        }
        let mut guard = self.services.write().unwrap();
        if let Some(existing) = guard.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(service_name)) {
            if existing.1 == port {
                return false;
            }
            existing.1 = port;
            return true;
        }
        guard.push((service_name.to_string(), port));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PeerConfig;

    fn dummy_peer() -> Peer {
        Peer::new(&PeerConfig {
            name: "alice".to_string(),
            virtual_ip: "10.66.0.2".parse().unwrap(),
            public_ip: "203.0.113.1".to_string(),
            public_port: 12345,
        })
    }

    #[test]
    fn observe_service_adds_new() {
        let p = dummy_peer();
        assert!(p.observe_service("game", 25565));
        assert_eq!(p.services(), vec![("game".to_string(), 25565)]);
    }

    #[test]
    fn observe_service_noop_when_unchanged() {
        let p = dummy_peer();
        assert!(p.observe_service("game", 25565));
        assert!(!p.observe_service("game", 25565)); // no change second time
        assert_eq!(p.services().len(), 1);
    }

    #[test]
    fn observe_service_updates_changed_port() {
        let p = dummy_peer();
        p.observe_service("game", 25565);
        assert!(p.observe_service("game", 25566));
        assert_eq!(p.services(), vec![("game".to_string(), 25566)]);
    }

    #[test]
    fn observe_service_ignores_empty_name() {
        let p = dummy_peer();
        assert!(!p.observe_service("", 80));
        assert!(p.services().is_empty());
    }

    #[test]
    fn observe_service_is_case_insensitive_for_dedup() {
        let p = dummy_peer();
        p.observe_service("Game", 1);
        assert!(!p.observe_service("game", 1)); // same service, different case -- no dup
        assert_eq!(p.services().len(), 1);
    }
}
