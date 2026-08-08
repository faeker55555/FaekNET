use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use socket2::{Domain, Socket, Type};
use tun_rs::DeviceBuilder;

use crate::config::{Config, PeerConfig};
use crate::crypto::Cipher;
use crate::dns::{self, DnsHandle};
use crate::gossip::{self, GossipEntry};
use crate::hosts;
use crate::peer::{now_secs, Peer};
use crate::proto;
use crate::stun;

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const GOSSIP_INTERVAL: Duration = Duration::from_secs(20);
const SELF_STUN_INTERVAL: Duration = Duration::from_secs(25);
const SELF_STUN_TIMEOUT: Duration = Duration::from_secs(2);
const RECV_LOOP_TIMEOUT: Duration = Duration::from_secs(30);
/// Sanity cap on total mesh size. Gossip only ever gets processed from
/// packets that already passed AEAD authentication against the shared
/// key, so this isn't a security boundary -- it just prevents a buggy or
/// runaway peer from growing the in-memory table without bound.
const MAX_PEERS: usize = 512;

pub fn log(msg: &str) {
    crate::logsink::emit(msg);
}


/// Finds the real physical/internet-facing network interface name, so the
/// mesh's UDP socket can be pinned to it. This matters on machines that
/// also have the mesh's own virtual TUN adapter and/or VPN adapters
/// present: without pinning, the OS could pick an unexpected route for
/// mesh traffic, especially once the TUN device brings up a route for the
/// mesh subnet -- or, notably, once an always-on VPN/proxy client like
/// Cloudflare WARP rewrites the default route to point at its own virtual
/// adapter, which would otherwise silently drag the mesh's own traffic
/// (including self-STUN discovery) through the VPN and report the VPN's
/// address as "yours" instead of your real one.
#[cfg(target_os = "linux")]
fn get_real_interface() -> Option<String> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg("ip route show default | grep -v CloudflareWARP | head -1 | awk '{print $5}'")
        .output()
        .ok()?;
    let iface = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if iface.is_empty() {
        None
    } else {
        Some(iface)
    }
}

/// Windows equivalent of the Linux function above: picks the best
/// candidate physical adapter, explicitly skipping known VPN/tunnel-style
/// virtual adapters by their driver description (e.g. Cloudflare WARP's
/// is literally named "Cloudflare WARP Interface Tunnel" in
/// GetIfEntry2/description() -- the same signal the Linux path filters on
/// by interface name). Windows Defender/other VPN clients that don't
/// expose a scriptable exclusion mechanism (unlike WARP's own `warp-cli`
/// on Linux/macOS, which has no equivalent in WARP's Windows GUI client)
/// are exactly the case this exists to work around: rather than relying
/// on the VPN cooperating, the mesh routes around it at the socket level.
///
/// Returns the winning interface's index (what IP_UNICAST_IF needs) and a
/// human-readable label for logging.
#[cfg(target_os = "windows")]
fn get_real_interface() -> Option<(u32, String)> {
    use netconfig_rs::sys::InterfaceExt;

    const VPN_DESCRIPTION_MARKERS: &[&str] = &[
        "cloudflare warp",
        "wireguard",
        "openvpn",
        "tap-windows",
        "wintun",
        "nordlynx",
        "tunnelbear",
    ];

    let interfaces = netconfig_rs::list_interfaces().ok()?;
    let mut best: Option<(u32, String)> = None;
    for iface in interfaces {
        let Ok(index) = iface.index() else { continue };
        let Ok(addresses) = iface.addresses() else { continue };
        // Only interested in adapters that actually have an IPv4 address
        // (rules out disabled/unconfigured adapters without needing a
        // separate "is this adapter up" check).
        if !addresses.iter().any(|a| a.addr().is_ipv4()) {
            continue;
        }
        let description = iface.description().unwrap_or_default().to_lowercase();
        let name = iface.name().unwrap_or_else(|_| format!("if{index}"));
        if VPN_DESCRIPTION_MARKERS.iter().any(|marker| description.contains(marker)) {
            continue; // explicitly excluded, e.g. Cloudflare WARP
        }
        // Also skip loopback and our own virtual mesh adapter by name, in
        // case a future run reuses the same process before the old
        // adapter is fully torn down.
        if name.eq_ignore_ascii_case("loopback") || name.to_lowercase().starts_with("lanmesh") {
            continue;
        }
        // First non-excluded candidate wins; if multiple physical NICs
        // exist (e.g. Wi-Fi + Ethernet both up), this doesn't attempt to
        // rank them by route metric the way Linux's default-route lookup
        // implicitly does -- good enough for the common case, and still
        // strictly better than picking whatever the VPN's route
        // rewriting would otherwise cause the OS to choose.
        if best.is_none() {
            best = Some((index, name));
        }
    }
    best
}

fn create_udp_socket(listen_port: u16) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "linux")]
    {
        if let Some(iface) = get_real_interface() {
            match socket.bind_device(Some(iface.as_bytes())) {
                Ok(_) => log(&format!("UDP socket bound to device: {}", iface)),
                Err(e) => log(&format!(
                    "Could not bind UDP socket to device ({e}); continuing without it."
                )),
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some((index, name)) = get_real_interface() {
            match bind_socket_to_interface_windows(&socket, index) {
                Ok(()) => log(&format!("UDP socket bound to interface: {name} (index {index})")),
                Err(e) => log(&format!(
                    "Could not bind UDP socket to interface '{name}' ({e}); continuing without it."
                )),
            }
        } else {
            log("Warning: could not identify a non-VPN network interface to bind to; \
if you have Cloudflare WARP or another always-on VPN active, the mesh's \
self-detected public address may be wrong.");
        }
    }
    let addr: SocketAddr = format!("0.0.0.0:{}", listen_port).parse().unwrap();
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

/// Applies IP_UNICAST_IF, the Windows equivalent of Linux's
/// SO_BINDTODEVICE: restricts which interface this socket's *outbound*
/// unicast IPv4 traffic egresses through, regardless of what the (WARP-
/// rewritten) routing table would otherwise pick. Must be called before
/// the socket is used for the binding to take effect for all subsequent
/// sends.
#[cfg(target_os = "windows")]
fn bind_socket_to_interface_windows(socket: &Socket, interface_index: u32) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{setsockopt, IPPROTO_IP, IP_UNICAST_IF, SOCKET};

    // IP_UNICAST_IF expects the interface index in network byte order,
    // packed into the same 4 bytes a socket option value normally holds
    // (this is a well-documented Winsock quirk, distinct from IPv6's
    // equivalent option which wants host byte order -- see Microsoft's
    // own docs and the socket2/shadowsocks-rust prior art referenced in
    // this function's design).
    let index_network_order: u32 = interface_index.to_be();
    let raw_socket = socket.as_raw_socket();
    let win_socket = SOCKET(raw_socket as usize);
    let bytes = index_network_order.to_ne_bytes();

    let result = unsafe { setsockopt(win_socket, IPPROTO_IP.0, IP_UNICAST_IF, Some(&bytes)) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Slot used to hand a STUN binding response, received on the mesh's own
/// shared UDP socket, back to whichever thread is waiting for it. Only one
/// self-STUN probe is ever in flight at a time, so a single slot (rather
/// than a table keyed by transaction ID) is sufficient; the transaction ID
/// is still checked so a stray/duplicate/late response can't be confused
/// for the current probe's answer.
struct SelfStunWaiter {
    slot: Mutex<Option<([u8; 12], mpsc::Sender<SocketAddr>)>>,
}

impl SelfStunWaiter {
    fn new() -> Self {
        SelfStunWaiter {
            slot: Mutex::new(None),
        }
    }

    /// Called from the mesh's single UDP receive-loop thread for every
    /// datagram that looks like a STUN message (checked before attempting
    /// AEAD decryption, since it's obviously not going to be one of our
    /// own encrypted packets). Delivers the resolved address to whichever
    /// thread is currently waiting, if the transaction ID matches.
    fn try_deliver(&self, data: &[u8]) -> bool {
        let mut guard = self.slot.lock().unwrap();
        let Some((tx_id, _)) = guard.as_ref() else {
            return false;
        };
        let Some(addr) = stun::try_parse_response_for(data, tx_id) else {
            return false;
        };
        if let Some((_, sender)) = guard.take() {
            let _ = sender.send(addr);
        }
        true
    }
}

pub struct MeshState {
    pub cipher: Cipher,
    pub my_virtual_ip: Ipv4Addr,
    pub my_name: String,
    pub broadcast_addr: Ipv4Addr,
    /// The live, dynamically-growing peer table. Starts out populated from
    /// mesh.toml, and grows automatically as gossip introduces peers we
    /// were never manually told about -- this is what makes the mesh
    /// self-propagating instead of a fixed list.
    peers: RwLock<HashMap<Ipv4Addr, Arc<Peer>>>,
    /// Our own best-known public (ip, port) and the epoch it was learned
    /// at, from periodic self-STUN checks. None until the first successful
    /// check completes. This is what lets brand-new, gossip-only peers
    /// (who never directly exported/imported with us) learn how to reach
    /// us at all.
    my_public_addr: Mutex<Option<(SocketAddr, u32)>>,
    self_stun: SelfStunWaiter,
    /// Kept around so that when a peer's address is learned/roamed to a
    /// new value, or a brand new peer is discovered via gossip, we can
    /// persist it back to mesh.toml -- otherwise a restart would lose
    /// everything gossip discovered and go back to only the manually
    /// configured peers until gossip rediscovers them again.
    pub config: Mutex<Config>,
    /// Suffix appended to sanitized peer names to build local domain
    /// names (e.g. "mesh" -> "alice.mesh"). Copied out of config at
    /// startup for cheap access from the hot paths that refresh
    /// hosts-file/DNS entries.
    domain_suffix: String,
    sync_hosts_file: bool,
    /// Present only if the built-in DNS resolver is enabled; shared with
    /// its background thread so peer-table changes can be reflected
    /// immediately rather than on a polling delay.
    dns_table: Option<dns::DnsTable>,
    /// Handle to the DNS resolver thread, if running, so it can be
    /// stopped when the mesh stops.
    dns_handle: Mutex<Option<DnsHandle>>,
}

impl MeshState {
    fn peers_snapshot(&self) -> Vec<Arc<Peer>> {
        self.peers.read().unwrap().values().cloned().collect()
    }

    fn get_peer(&self, ip: &Ipv4Addr) -> Option<Arc<Peer>> {
        self.peers.read().unwrap().get(ip).cloned()
    }

    fn peer_count(&self) -> usize {
        self.peers.read().unwrap().len()
    }

    /// Provisionally learns a brand-new peer purely from an unsolicited
    /// but authenticated PING -- the fix for the "my friend added me and
    /// can reach me, but I never added them so I can't reach back" case.
    ///
    /// Without this, a one-sided add (A configures B's address, but B
    /// never configures A's) is a dead end: A's keepalive/PING packets
    /// physically arrive at B's machine, but B's receive loop used to
    /// silently drop anything from a virtual IP it didn't already
    /// recognize -- even though the packet already passed ChaCha20-
    /// Poly1305 authentication against the shared mesh key, which is the
    /// same trust bar gossip-discovered peers are held to. This closes
    /// that gap: hearing a PING from someone we don't know is treated the
    /// same way hearing about them via gossip would be. We don't yet know
    /// their display name (PING packets don't carry one), so the entry is
    /// created with a placeholder name that gets overwritten within one
    /// gossip cycle once the new peer (or a mutual friend) announces it
    /// properly.
    ///
    /// Returns Some(NewPeer) if this actually added a new entry (worth
    /// logging/persisting), or None if the peer was already known or the
    /// mesh is at its sanity cap.
    fn learn_peer_from_ping(&self, virtual_ip: Ipv4Addr, addr: SocketAddr) -> Option<GossipOutcome> {
        if virtual_ip == self.my_virtual_ip {
            return None;
        }
        if self.get_peer(&virtual_ip).is_some() {
            return None; // already known; observe() on the normal path handles address updates
        }
        if self.peer_count() >= MAX_PEERS {
            return None;
        }
        let placeholder_name = format!("peer-{virtual_ip}");
        let epoch = now_secs() as u32;
        let peer = Arc::new(Peer::from_gossip(virtual_ip, &placeholder_name, addr, epoch));
        self.peers.write().unwrap().insert(virtual_ip, peer);
        Some(GossipOutcome::NewPeer {
            virtual_ip,
            name: placeholder_name,
            addr,
        })
    }

    /// Applies one gossip entry to our local peer table. Returns Some
    /// describing what happened if it's worth logging/persisting, or None
    /// if the entry was ignored (about ourselves, no-op, or the mesh is
    /// already at its sanity cap).
    fn apply_gossip_entry(&self, entry: &GossipEntry) -> Option<GossipOutcome> {
        if entry.virtual_ip == self.my_virtual_ip {
            return None; // gossip about ourselves, bouncing back around -- ignore
        }
        if let Some(existing) = self.get_peer(&entry.virtual_ip) {
            existing.set_name(&entry.name);
            if existing.observe_epoch(entry.addr, entry.epoch_secs) {
                return Some(GossipOutcome::AddressUpdated {
                    virtual_ip: entry.virtual_ip,
                    name: existing.name(),
                    addr: entry.addr,
                });
            }
            return None;
        }

        if self.peer_count() >= MAX_PEERS {
            return None;
        }
        let peer = Arc::new(Peer::from_gossip(entry.virtual_ip, &entry.name, entry.addr, entry.epoch_secs));
        self.peers.write().unwrap().insert(entry.virtual_ip, peer);
        Some(GossipOutcome::NewPeer {
            virtual_ip: entry.virtual_ip,
            name: entry.name.clone(),
            addr: entry.addr,
        })
    }
}

/// Rebuilds the full local-domain-name mapping (ourselves + every known
/// peer) and pushes it to whichever mechanisms are enabled: the hosts
/// file, and/or the built-in DNS resolver's live table. Called after
/// startup and every time the peer table changes (new peer via gossip or
/// PING, address/name update via gossip, manual add-peer) so names never
/// lag behind reality by more than one such event.
fn refresh_domain_names(state: &MeshState) {
    if !state.sync_hosts_file && state.dns_table.is_none() {
        return; // neither mechanism enabled -- nothing to do
    }
    let mut raw: Vec<(String, Ipv4Addr)> = vec![(state.my_name.clone(), state.my_virtual_ip)];
    for peer in state.peers_snapshot() {
        raw.push((peer.name(), peer.virtual_ip));
    }
    let entries = hosts::build_entries(&state.domain_suffix, &raw);

    if state.sync_hosts_file {
        match hosts::sync(&entries) {
            Ok(()) => {}
            Err(e) => log(&format!(
                "Warning: could not update hosts file for local domain names ({e}). \
This does not affect mesh connectivity, only the convenience of using \
names like 'alice.{}' instead of raw virtual IPs.",
                state.domain_suffix
            )),
        }
    }
    if let Some(table) = &state.dns_table {
        let flat: Vec<(String, Ipv4Addr)> = entries.into_iter().map(|e| (e.hostname, e.virtual_ip)).collect();
        dns::update_table(table, flat);
    }
}

enum GossipOutcome {
    NewPeer {
        virtual_ip: Ipv4Addr,
        name: String,
        addr: SocketAddr,
    },
    AddressUpdated {
        virtual_ip: Ipv4Addr,
        name: String,
        addr: SocketAddr,
    },
}

/// Persists a peer's address into the on-disk config, best effort --
/// creating a new entry if this peer was never manually configured (the
/// gossip-discovery case), or updating an existing one (the roaming /
/// gossip-refresh case). Failures are logged but not fatal -- the
/// in-memory mesh keeps working regardless, this is purely so the next
/// restart starts "warm" with everything already discovered.
fn persist_peer_addr(state: &MeshState, virtual_ip: Ipv4Addr, name: &str, addr: SocketAddr) {
    let mut cfg = state.config.lock().unwrap();
    if let Some(p) = cfg.peers.iter_mut().find(|p| p.virtual_ip == virtual_ip) {
        p.public_ip = addr.ip().to_string();
        p.public_port = addr.port();
        if !name.is_empty() {
            p.name = name.to_string();
        }
    } else {
        cfg.peers.push(PeerConfig {
            name: name.to_string(),
            virtual_ip,
            public_ip: addr.ip().to_string(),
            public_port: addr.port(),
        });
    }
    if let Err(e) = cfg.save() {
        log(&format!("Warning: could not persist peer address: {e}"));
    }
}

/// Builds the gossip entries describing everything we currently know:
/// every peer in our table, plus an entry for ourselves if we've
/// successfully self-STUN'd at least once (so 2+-hop peers can learn how
/// to reach us, not just the peers we've directly exchanged addresses
/// with).
fn build_local_gossip_entries(state: &MeshState) -> Vec<GossipEntry> {
    let mut entries: Vec<GossipEntry> = state
        .peers_snapshot()
        .iter()
        .map(|p| GossipEntry {
            virtual_ip: p.virtual_ip,
            name: p.name(),
            addr: p.current_send_addr().unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0))),
            epoch_secs: p.confirmed_epoch(),
        })
        .filter(|e| e.addr.port() != 0)
        .collect();

    if let Some((addr, epoch)) = *state.my_public_addr.lock().unwrap() {
        entries.push(GossipEntry {
            virtual_ip: state.my_virtual_ip,
            name: state.my_name.clone(),
            addr,
            epoch_secs: epoch,
        });
    }
    entries
}

fn send_gossip_burst(state: &MeshState, sock: &UdpSocket) {
    let entries = build_local_gossip_entries(state);
    if entries.is_empty() {
        return;
    }
    let payloads = gossip::build_payloads(&entries);
    for peer in state.peers_snapshot() {
        let Some(addr) = peer.current_send_addr() else {
            continue;
        };
        for payload in &payloads {
            let wire_plain = proto::build(proto::TYPE_GOSSIP, state.my_virtual_ip, payload);
            let wire = state.cipher.seal(&wire_plain);
            let _ = sock.send_to(&wire, addr);
        }
    }
}

/// Sends one STUN binding request over the mesh's own shared socket (so
/// the discovered external mapping is the one that actually matters --
/// the mapping for the exact port peers need to send to) and waits for
/// the mesh's receive loop to hand back a matching response. Tries each
/// well-known STUN server in turn until one answers or all fail.
fn self_stun_probe(state: &MeshState, sock: &UdpSocket) -> Option<SocketAddr> {
    for (host, port) in stun::DEFAULT_SERVERS {
        let Ok(mut addrs) = (*host, *port).to_socket_addrs() else {
            continue;
        };
        let Some(server_addr) = addrs.find(|a| a.is_ipv4()) else {
            continue;
        };
        let (tx_id, req) = stun::new_transaction();
        let (tx, rx) = mpsc::channel();
        {
            let mut guard = state.self_stun.slot.lock().unwrap();
            *guard = Some((tx_id, tx));
        }
        if sock.send_to(&req, server_addr).is_err() {
            continue;
        }
        if let Ok(addr) = rx.recv_timeout(SELF_STUN_TIMEOUT) {
            return Some(addr);
        }
        // Clear the slot before trying the next server so a very late
        // response from this attempt can't be misdelivered later.
        *state.self_stun.slot.lock().unwrap() = None;
    }
    None
}

/// Point-in-time, UI-friendly view of one peer, for GUIs/status displays
/// that can't (and shouldn't) reach into the live `Peer`/`Arc` internals
/// directly.
#[derive(Clone, Debug)]
pub struct PeerSnapshot {
    pub name: String,
    pub virtual_ip: Ipv4Addr,
    pub addr: Option<SocketAddr>,
    pub rtt_ms: Option<u64>,
    pub seconds_since_seen: Option<u64>,
    pub discovered_via_gossip: bool,
}

/// Point-in-time, UI-friendly view of the whole mesh's state.
#[derive(Clone, Debug)]
pub struct MeshSnapshot {
    pub my_virtual_ip: Ipv4Addr,
    pub my_name: String,
    pub my_public_addr: Option<SocketAddr>,
    pub listen_port: u16,
    pub peers: Vec<PeerSnapshot>,
}

/// A running mesh instance. Dropping this does NOT stop the mesh (the
/// background threads keep an `Arc` of their own) -- call `stop()`
/// explicitly, which is what actually signals every worker thread to
/// exit. This split exists so a GUI can hold a handle across many event
/// loop frames, polling `snapshot()` each frame, without blocking.
pub struct MeshHandle {
    state: Arc<MeshState>,
    running: Arc<AtomicBool>,
}

impl MeshHandle {
    /// Signals all of the mesh's background threads to stop at their next
    /// opportunity (they check this flag between iterations). Threads
    /// blocked in a socket read will exit within one `RECV_LOOP_TIMEOUT`.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.state.dns_handle.lock().unwrap().take() {
            handle.stop();
        }
        #[cfg(target_os = "windows")]
        {
            if self.state.config.lock().unwrap().me.dns_auto_configure {
                if let Some((_, iface_name)) = get_real_interface() {
                    dns::try_undo_auto_configure(&iface_name);
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            if self.state.config.lock().unwrap().me.dns_auto_configure {
                dns::try_undo_auto_configure();
            }
        }
        if self.state.sync_hosts_file {
            let _ = hosts::remove_block();
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Builds a cheap, consistent point-in-time snapshot of the mesh for
    /// display purposes (GUI peer list, status bar, etc.). Safe to call
    /// every frame from a GUI's update loop.
    pub fn snapshot(&self) -> MeshSnapshot {
        let mut peers: Vec<PeerSnapshot> = self
            .state
            .peers_snapshot()
            .iter()
            .map(|p| PeerSnapshot {
                name: p.name(),
                virtual_ip: p.virtual_ip,
                addr: p.current_send_addr(),
                rtt_ms: p.last_rtt_ms(),
                seconds_since_seen: p.seconds_since_seen(),
                discovered_via_gossip: p.discovered_via_gossip.load(Ordering::Relaxed),
            })
            .collect();
        peers.sort_by(|a, b| a.virtual_ip.cmp(&b.virtual_ip));

        MeshSnapshot {
            my_virtual_ip: self.state.my_virtual_ip,
            my_name: self.state.my_name.clone(),
            my_public_addr: self.state.my_public_addr.lock().unwrap().map(|(a, _)| a),
            listen_port: self.state.config.lock().unwrap().me.listen_port,
            peers,
        }
    }

    /// Returns a clone of the live config, e.g. so a GUI can display or
    /// re-save it (settings screen, "copy my card" flow, etc.).
    pub fn config_snapshot(&self) -> Config {
        self.state.config.lock().unwrap().clone()
    }

    /// Adds (or updates) a peer in the live, already-running mesh -- used
    /// by the GUI's "add peer" flow so a manually-imported card takes
    /// effect immediately, exactly like a gossip-discovered peer does,
    /// instead of requiring a restart. Also persists to mesh.toml.
    pub fn add_peer_live(&self, peer: PeerConfig) {
        let addr_str = format!("{}:{}", peer.public_ip, peer.public_port);
        let Ok(addr) = addr_str.parse::<SocketAddr>() else {
            return;
        };
        if let Some(existing) = self.state.get_peer(&peer.virtual_ip) {
            existing.set_name(&peer.name);
            existing.observe_epoch(addr, now_secs() as u32);
        } else {
            let new_peer = Arc::new(Peer::new(&peer));
            self.state.peers.write().unwrap().insert(peer.virtual_ip, new_peer);
        }
        persist_peer_addr(&self.state, peer.virtual_ip, &peer.name, addr);
        refresh_domain_names(&self.state);
    }

    /// Point-in-time view of this machine's local mesh domain names
    /// (itself plus every currently-known peer), for the GUI's domains
    /// screen and the "open in browser" shortcut.
    pub fn domain_snapshot(&self) -> Vec<(String, Ipv4Addr)> {
        let cfg = self.state.config.lock().unwrap();
        let suffix = cfg.me.domain_suffix.clone();
        drop(cfg);
        let mut raw: Vec<(String, Ipv4Addr)> =
            vec![(self.state.my_name.clone(), self.state.my_virtual_ip)];
        for peer in self.state.peers_snapshot() {
            raw.push((peer.name(), peer.virtual_ip));
        }
        hosts::build_entries(&suffix, &raw)
            .into_iter()
            .map(|e| (e.hostname, e.virtual_ip))
            .collect()
    }
}

/// Starts the mesh (virtual adapter, UDP transport, and all background
/// threads: hole-punch/keepalive, gossip, self-STUN) and returns
/// immediately with a handle for polling status and requesting shutdown.
/// This is the entry point GUIs should use instead of `run()`, which
/// blocks the calling thread forever (fine for a CLI's main thread, fatal
/// for a GUI's event loop thread).
pub fn start(config: Config) -> std::io::Result<MeshHandle> {
    let cipher = Cipher::from_psk_b64(&config.me.psk)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if config.peers.is_empty() {
        log("No peers configured yet -- waiting for a bootstrap peer (import a card with `lan_mesh import`).");
    } else {
        log(&format!(
            "Starting with {} known peer(s); more may be auto-discovered via gossip.",
            config.peers.len()
        ));
    }

    let mut peers_map = HashMap::new();
    for p in &config.peers {
        peers_map.insert(p.virtual_ip, Arc::new(Peer::new(p)));
    }

    let broadcast_addr = config.broadcast_addr();
    let my_virtual_ip = config.me.virtual_ip;
    let my_name = config.me.name.clone();
    let mtu = config.me.mtu;
    let prefix = config.me.prefix;
    let listen_port = config.me.listen_port;
    let domain_suffix = config.me.domain_suffix.clone();
    let sync_hosts_file = config.me.sync_hosts_file;
    let dns_server_enabled = config.me.dns_server;
    let dns_port = config.me.dns_port;
    let dns_auto_configure = config.me.dns_auto_configure;

    let dns_table = if dns_server_enabled { Some(dns::new_table()) } else { None };

    let state = Arc::new(MeshState {
        cipher,
        my_virtual_ip,
        my_name,
        broadcast_addr,
        peers: RwLock::new(peers_map),
        my_public_addr: Mutex::new(None),
        self_stun: SelfStunWaiter::new(),
        config: Mutex::new(config),
        domain_suffix,
        sync_hosts_file,
        dns_table: dns_table.clone(),
        dns_handle: Mutex::new(None),
    });

    // ---- Local domain names: hosts-file sync and/or built-in DNS ----
    if let Some(table) = &dns_table {
        match dns::start(dns_port, table.clone()) {
            Ok(handle) => {
                *state.dns_handle.lock().unwrap() = Some(handle);
                if dns_auto_configure {
                    #[cfg(target_os = "linux")]
                    {
                        if let Err(e) = dns::try_auto_configure_system(dns_port) {
                            log(&format!("Warning: DNS auto-configure failed: {e}"));
                        }
                    }
                    #[cfg(target_os = "windows")]
                    {
                        if let Some((_, iface_name)) = get_real_interface() {
                            if let Err(e) = dns::try_auto_configure_system(&iface_name, dns_port) {
                                log(&format!("Warning: DNS auto-configure failed: {e}"));
                            }
                        } else {
                            log("Warning: DNS auto-configure skipped (could not identify a network interface).");
                        }
                    }
                }
            }
            Err(e) => log(&format!(
                "Warning: could not start built-in DNS resolver on 127.0.0.1:{dns_port} ({e}). \
Local mesh domain names will still work via the hosts file if enabled."
            )),
        }
    }
    refresh_domain_names(&state);

    // ---- Set up the virtual network adapter ----
    let dev_name = std::env::var("LAN_MESH_DEV_NAME").unwrap_or_else(|_| "lanmesh0".to_string());
    let dev = DeviceBuilder::new()
        .name(dev_name)
        .ipv4(my_virtual_ip, prefix, None)
        .mtu(mtu)
        .build_sync()
        .map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to create virtual network adapter ({e}). \
On Linux this needs root/CAP_NET_ADMIN (try running with sudo). \
On Windows this needs Administrator and wintun.dll next to the executable."
                ),
            )
        })?;
    log(&format!(
        "Virtual adapter up: {} ({}/{})",
        dev.name().unwrap_or_else(|_| "lanmesh0".to_string()),
        my_virtual_ip,
        prefix
    ));
    let dev = Arc::new(dev);

    // ---- Set up the UDP transport socket ----
    let sock = create_udp_socket(listen_port)?;
    log(&format!("Listening on UDP 0.0.0.0:{}", listen_port));
    let sock = Arc::new(sock);
    sock.set_read_timeout(Some(RECV_LOOP_TIMEOUT))?;

    let running = Arc::new(AtomicBool::new(true));

    // ---- TUN -> UDP: packets from local apps/games headed onto the virtual LAN ----
    {
        let dev = dev.clone();
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                let n = match dev.recv(&mut buf) {
                    Ok(n) => n,
                    Err(e) => {
                        log(&format!("TUN read error: {e}"));
                        continue;
                    }
                };
                let packet = &buf[..n];
                let Some(dst) = proto::ipv4_dst(packet) else {
                    continue; // not IPv4 (e.g. IPv6), mesh only routes v4
                };

                let wire_plain = proto::build(proto::TYPE_DATA, state.my_virtual_ip, packet);
                let wire = state.cipher.seal(&wire_plain);

                if proto::is_flood_target(dst, state.broadcast_addr) {
                    for peer in state.peers_snapshot() {
                        if let Some(addr) = peer.current_send_addr() {
                            let _ = sock.send_to(&wire, addr);
                        }
                    }
                } else if let Some(peer) = state.get_peer(&dst) {
                    if let Some(addr) = peer.current_send_addr() {
                        let _ = sock.send_to(&wire, addr);
                    }
                }
                // else: destination isn't a known mesh peer or a flood
                // target -- silently drop, nothing we can do with it.
            }
        });
    }

    // ---- UDP -> TUN: packets arriving from mesh peers, injected as if they came off a real LAN NIC ----
    {
        let dev = dev.clone();
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                let (n, from_addr) = match sock.recv_from(&mut buf) {
                    Ok(v) => v,
                    Err(e) => match e.kind() {
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => continue,
                        _ => {
                            log(&format!("UDP recv error: {e}"));
                            continue;
                        }
                    },
                };

                // Check for an in-flight self-STUN probe response before
                // attempting mesh decryption -- STUN responses are plain,
                // unencrypted UDP from a public STUN server and would
                // never successfully decrypt as mesh traffic anyway, but
                // checking first (and matching cheaply on the STUN magic
                // cookie) avoids wasted AEAD work and keeps this codepath
                // obviously correct rather than relying on decryption
                // failure as an implicit signal.
                if stun::looks_like_stun_message(&buf[..n]) && state.self_stun.try_deliver(&buf[..n]) {
                    continue;
                }

                let Some(plain) = state.cipher.open(&buf[..n]) else {
                    // Wrong PSK, corrupted, or random internet noise -- drop.
                    continue;
                };
                let Some((hdr, payload)) = proto::parse(&plain) else {
                    continue;
                };
                // Normally, only traffic from an already-known peer is
                // accepted -- gossip is the usual way *new* peers get
                // introduced (an already-known peer describing them in a
                // payload). The one deliberate exception: an unsolicited
                // TYPE_PING from a virtual IP we've never heard of is
                // itself a valid introduction, since it means someone who
                // has our mesh's shared key (the packet already passed
                // AEAD authentication) has configured us as a peer and is
                // actively trying to reach us. Without this, a one-sided
                // add (they add us, we never add them) would leave them
                // permanently unreachable from our side -- see the
                // learn_peer_from_ping doc comment for the full story.
                // TYPE_DATA is deliberately NOT treated this way: we only
                // ever want to auto-learn a peer from a packet whose whole
                // purpose is self-announcement, never from what could be
                // real application traffic.
                let peer = match state.get_peer(&hdr.sender_virtual_ip) {
                    Some(peer) => peer,
                    None if hdr.packet_type == proto::TYPE_PING => {
                        match state.learn_peer_from_ping(hdr.sender_virtual_ip, from_addr) {
                            Some(GossipOutcome::NewPeer { virtual_ip, name, addr }) => {
                                log(&format!(
                                    "Peer '{name}' ({virtual_ip}) reached us first at {addr} -- \
adding them so we can reach back (their real name will arrive shortly via gossip)."
                                ));
                                persist_peer_addr(&state, virtual_ip, &name, addr);
                                refresh_domain_names(&state);
                                state.get_peer(&virtual_ip).expect("just inserted")
                            }
                            _ => continue,
                        }
                    }
                    None => continue,
                };
                let was_new = peer.seconds_since_seen().is_none();
                let addr_changed = peer.observe(from_addr);
                if was_new {
                    log(&format!(
                        "Peer '{}' ({}) is now reachable at {}",
                        peer.name(), peer.virtual_ip, from_addr
                    ));
                }
                if addr_changed {
                    persist_peer_addr(&state, peer.virtual_ip, &peer.name(), from_addr);
                }

                match hdr.packet_type {
                    proto::TYPE_DATA if !payload.is_empty() => {
                        if let Err(e) = dev.send(payload) {
                            log(&format!("TUN write error: {e}"));
                        }
                    }
                    proto::TYPE_PING if payload.len() == 8 => {
                        // Echo the nonce straight back as a PONG, over UDP
                        // directly -- this deliberately bypasses the TUN
                        // device so `lan_mesh ping` works without root and
                        // measures the mesh transport's own latency, not
                        // anything game/OS-routing related.
                        let pong_plain =
                            proto::build(proto::TYPE_PONG, state.my_virtual_ip, payload);
                        let pong_wire = state.cipher.seal(&pong_plain);
                        let _ = sock.send_to(&pong_wire, from_addr);
                    }
                    proto::TYPE_PONG if payload.len() == 8 => {
                        let seq = u64::from_be_bytes(payload.try_into().unwrap());
                        peer.record_pong_received(seq);
                    }
                    proto::TYPE_GOSSIP => {
                        for entry in gossip::parse_payload(payload) {
                            if let Some(outcome) = state.apply_gossip_entry(&entry) {
                                match outcome {
                                    GossipOutcome::NewPeer { virtual_ip, name, addr } => {
                                        log(&format!(
                                            "Discovered new peer via gossip: '{name}' ({virtual_ip}) @ {addr}"
                                        ));
                                        persist_peer_addr(&state, virtual_ip, &name, addr);
                                        refresh_domain_names(&state);
                                    }
                                    GossipOutcome::AddressUpdated { virtual_ip, name, addr } => {
                                        log(&format!(
                                            "Updated peer '{name}' ({virtual_ip}) address via gossip -> {addr}"
                                        ));
                                        persist_peer_addr(&state, virtual_ip, &name, addr);
                                        refresh_domain_names(&state);
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // TYPE_KEEPALIVE, or anything else: observe() above
                        // already did the useful work.
                    }
                }
            }
        });
    }

    // ---- Keepalive / hole-punch ticker (also doubles as a latency probe) ----
    {
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || {
            // Immediate burst on startup, similar in spirit to the chat
            // program's diagnostic pings: this is what actually opens the
            // NAT/CGNAT mapping on each side before any real traffic needs
            // to flow.
            let mut seq: u64 = 0;
            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                for peer in state.peers_snapshot() {
                    if let Some(addr) = peer.current_send_addr() {
                        // A TYPE_PING doubles as the keepalive itself (it
                        // still refreshes the NAT mapping and updates
                        // last-seen via the PONG's observe() call), while
                        // additionally giving us a live RTT reading for
                        // the periodic status log.
                        peer.record_ping_sent(seq);
                        let wire_plain =
                            proto::build(proto::TYPE_PING, state.my_virtual_ip, &seq.to_be_bytes());
                        let wire = state.cipher.seal(&wire_plain);
                        let _ = sock.send_to(&wire, addr);
                    }
                }
                seq = seq.wrapping_add(1);
                thread::sleep(KEEPALIVE_INTERVAL);
            }
        });
    }

    // ---- Gossip ticker: periodically share our full peer table with everyone we know ----
    {
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || loop {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            send_gossip_burst(&state, &sock);
            thread::sleep(GOSSIP_INTERVAL);
        });
    }

    // ---- Self-STUN ticker: keep our own external address current, so the
    // ---- mesh self-heals immediately after a NAT/CGNAT port reassignment
    // ---- instead of waiting for a peer to notice on their own.
    {
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || loop {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            match self_stun_probe(&state, &sock) {
                Some(addr) => {
                    let epoch = now_secs() as u32;
                    let previous = {
                        let mut guard = state.my_public_addr.lock().unwrap();
                        let previous = *guard;
                        *guard = Some((addr, epoch));
                        previous
                    };
                    let changed = previous.map(|(a, _)| a) != Some(addr);
                    if changed {
                        log(&format!(
                            "Our external address is {}{} -- notifying peers immediately.",
                            addr,
                            if previous.is_some() { " (changed)" } else { "" }
                        ));
                        // Don't wait for the next scheduled gossip tick --
                        // push the update out right away so the mesh
                        // converges on our new address as fast as
                        // possible after a NAT/CGNAT reassignment.
                        send_gossip_burst(&state, &sock);
                    }
                }
                None => {
                    log("Warning: could not determine our own external address via STUN this round (will retry).");
                }
            }
            thread::sleep(SELF_STUN_INTERVAL);
        });
    }

    log("Mesh is running.");
    log_peer_summary(&state);

    // Periodic status log, independent of anything a GUI might also be
    // polling via MeshHandle::snapshot() -- useful for the CLI/journald,
    // harmless overhead for the GUI case.
    {
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(60));
            if !running.load(Ordering::Relaxed) {
                break;
            }
            log_peer_summary(&state);
        });
    }

    Ok(MeshHandle { state, running })
}

/// Blocking convenience wrapper for the CLI: starts the mesh and then
/// parks the calling thread forever (process exit / Ctrl+C is how the CLI
/// stops -- see `MeshHandle::stop` for the GUI's non-blocking equivalent).
pub fn run(config: Config) -> std::io::Result<()> {
    let _handle = start(config)?;
    log("Press Ctrl+C to stop.");
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// Standalone latency test: sends a handful of encrypted PING packets to
/// every configured peer and reports round-trip time / packet loss.
/// Deliberately does NOT create the virtual TUN adapter, so this works
/// without root/Administrator and can be run even while `lan_mesh run` is
/// not active elsewhere (though it obviously can't measure anything for a
/// peer whose mesh isn't currently running to answer).
pub fn ping(config: Config, count: u32, timeout: Duration) -> std::io::Result<()> {
    let cipher = Cipher::from_psk_b64(&config.me.psk)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if config.peers.is_empty() {
        println!("No peers configured. Use `lan_mesh add-peer` or `lan_mesh import`.");
        return Ok(());
    }

    let sock = create_udp_socket(config.me.listen_port)?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    let my_virtual_ip = config.me.virtual_ip;

    struct Stats {
        name: String,
        sent: u32,
        received: u32,
        rtts_ms: Vec<u64>,
    }
    let mut stats: HashMap<Ipv4Addr, Stats> = HashMap::new();
    for p in &config.peers {
        stats.insert(
            p.virtual_ip,
            Stats {
                name: p.name.clone(),
                sent: 0,
                received: 0,
                rtts_ms: Vec::new(),
            },
        );
    }

    println!(
        "Pinging {} peer(s), {} probe(s) each...\n",
        config.peers.len(),
        count
    );

    for seq in 0..count as u64 {
        let mut sent_at: HashMap<Ipv4Addr, std::time::Instant> = HashMap::new();
        for p in &config.peers {
            let addr_str = format!("{}:{}", p.public_ip, p.public_port);
            let Ok(mut addrs) = addr_str
                .to_socket_addrs()
            else {
                continue;
            };
            let Some(addr) = addrs.next() else { continue };
            let plain = proto::build(proto::TYPE_PING, my_virtual_ip, &seq.to_be_bytes());
            let wire = cipher.seal(&plain);
            if sock.send_to(&wire, addr).is_ok() {
                stats.get_mut(&p.virtual_ip).unwrap().sent += 1;
                sent_at.insert(p.virtual_ip, std::time::Instant::now());
            }
        }

        let deadline = std::time::Instant::now() + timeout;
        let mut buf = [0u8; 512];
        while std::time::Instant::now() < deadline {
            let (n, _from) = match sock.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(plain) = cipher.open(&buf[..n]) else {
                continue;
            };
            let Some((hdr, payload)) = proto::parse(&plain) else {
                continue;
            };
            if hdr.packet_type != proto::TYPE_PONG || payload.len() != 8 {
                continue;
            }
            let got_seq = u64::from_be_bytes(payload.try_into().unwrap());
            if got_seq != seq {
                continue;
            }
            if let Some(sent_time) = sent_at.get(&hdr.sender_virtual_ip) {
                let rtt = sent_time.elapsed().as_millis() as u64;
                if let Some(s) = stats.get_mut(&hdr.sender_virtual_ip) {
                    s.received += 1;
                    s.rtts_ms.push(rtt);
                    println!("  {} ({}): seq={} time={}ms", s.name, hdr.sender_virtual_ip, seq, rtt);
                }
            }
        }
    }

    println!("\n--- results ---");
    for p in &config.peers {
        let s = &stats[&p.virtual_ip];
        let loss_pct = if s.sent == 0 {
            100.0
        } else {
            100.0 * (1.0 - s.received as f64 / s.sent as f64)
        };
        if s.rtts_ms.is_empty() {
            println!(
                "{} ({}): {}/{} received, 100% loss -- unreachable",
                s.name, p.virtual_ip, s.received, s.sent
            );
        } else {
            let min = *s.rtts_ms.iter().min().unwrap();
            let max = *s.rtts_ms.iter().max().unwrap();
            let avg = s.rtts_ms.iter().sum::<u64>() as f64 / s.rtts_ms.len() as f64;
            println!(
                "{} ({}): {}/{} received, {:.0}% loss, rtt min/avg/max = {}/{:.1}/{} ms",
                s.name, p.virtual_ip, s.received, s.sent, loss_pct, min, avg, max
            );
        }
    }

    Ok(())
}

fn log_peer_summary(state: &MeshState) {
    let peers = state.peers_snapshot();
    if peers.is_empty() {
        return;
    }
    for peer in peers {
        let status = match peer.seconds_since_seen() {
            Some(s) => match peer.last_rtt_ms() {
                Some(rtt) => format!("reachable, last heard {s}s ago, ~{rtt}ms"),
                None => format!("reachable, last heard {s}s ago"),
            },
            None => "not yet reachable".to_string(),
        };
        let via = if peer.discovered_via_gossip.load(Ordering::Relaxed) {
            " [auto-discovered]"
        } else {
            ""
        };
        log(&format!(
            "  peer '{}' ({}) @ {} -- {}{}",
            peer.name(),
            peer.virtual_ip,
            peer.current_send_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "unresolved".to_string()),
            status,
            via
        ));
    }
}
