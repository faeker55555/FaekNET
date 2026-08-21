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

/// Substance-free description of one candidate network interface, used by
/// `pick_real_interface` below -- kept independent of `netconfig_rs`'s own
/// types so the selection *logic* can be unit-tested on any platform,
/// with the real Windows-only enumeration code (`get_real_interface`)
/// just building this from `netconfig_rs::list_interfaces()` and handing
/// it off. Deliberately NOT `#[cfg(target_os = "windows")]`-gated (unlike
/// the enumeration code that builds it) so `pick_real_interface`'s
/// selection logic -- including the fix for the "self-STUN never
/// resolves on Windows" bug -- has real unit test coverage that runs on
/// every platform's CI, not just a Windows runner this project doesn't
/// have.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone)]
struct InterfaceCandidate {
    index: u32,
    name: String,
    description: String,
    ipv4_addrs: Vec<Ipv4Addr>,
}

/// Markers that identify a VPN/tunnel-style virtual adapter by its driver
/// description (e.g. Cloudflare WARP's is literally named "Cloudflare
/// WARP Interface Tunnel" in GetIfEntry2/description()). Windows
/// Defender/other VPN clients that don't expose a scriptable exclusion
/// mechanism (unlike WARP's own `warp-cli` on Linux/macOS, which has no
/// equivalent in WARP's Windows GUI client) are exactly the case this
/// exists to work around: rather than relying on the VPN cooperating,
/// the mesh routes around it at the socket level.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const VPN_DESCRIPTION_MARKERS: &[&str] = &[
    "cloudflare warp",
    "wireguard",
    "openvpn",
    "tap-windows",
    "wintun",
    "nordlynx",
    "tunnelbear",
];

/// Pure selection logic, factored out of `get_real_interface` so it can
/// be exercised by unit tests without needing real Windows adapters.
/// Picks the best candidate physical adapter out of `candidates`,
/// skipping known VPN/tunnel-style virtual adapters and (critically) our
/// own mesh TUN adapter.
///
/// `my_virtual_ip` is the mesh's own configured virtual address, if
/// known -- this is the authoritative way to exclude our *own* TUN
/// adapter, which the description-based VPN markers and any name-prefix
/// heuristic can both fail to catch. This is the fix for a real bug:
/// tun-rs's Windows/wintun backend defaults an adapter's *driver
/// description* to its dev name (e.g. "meowmesh0") when no separate
/// description is given, which does NOT contain "wintun" -- so the
/// description-based VPN_DESCRIPTION_MARKERS check silently fails to
/// catch it, and Windows doesn't reliably preserve the requested adapter
/// *name* either (it can surface as "Ethernet 3", "Local Area Connection
/// 2", etc., depending on driver/Windows version -- a well-documented
/// wintun/OpenVPN community annoyance, not something under this
/// project's control). When that happened, the old name-prefix-only
/// check (`starts_with("lanmesh")`) failed to exclude our own adapter,
/// so the mesh ended up pinning its own UDP socket (and self-STUN
/// probes) to its own virtual adapter, which has no real route to the
/// internet -- every self-STUN attempt then timed out and retried
/// forever, i.e. exactly the "public address never resolves" symptom
/// this fixes. Matching on the address itself can never miss our own
/// adapter or accidentally exclude a real NIC the way string-matching on
/// name/description can, since our adapter is guaranteed to carry
/// exactly `my_virtual_ip` (that's the address we asked tun-rs/wintun to
/// assign it).
///
/// Returns the winning interface's index (what IP_UNICAST_IF needs) and
/// a human-readable label for logging.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pick_real_interface(candidates: &[InterfaceCandidate], my_virtual_ip: Option<Ipv4Addr>) -> Option<(u32, String)> {
    let mut best: Option<(u32, String)> = None;
    for iface in candidates {
        // Only interested in adapters that actually have an IPv4 address
        // (rules out disabled/unconfigured adapters without needing a
        // separate "is this adapter up" check).
        if iface.ipv4_addrs.is_empty() {
            continue;
        }
        if let Some(my_ip) = my_virtual_ip {
            if iface.ipv4_addrs.contains(&my_ip) {
                continue; // this is our own mesh TUN adapter
            }
        }
        let description = iface.description.to_lowercase();
        if VPN_DESCRIPTION_MARKERS.iter().any(|marker| description.contains(marker)) {
            continue; // explicitly excluded, e.g. Cloudflare WARP
        }
        // Secondary, best-effort heuristics kept as defense-in-depth for
        // cases the address check above can't cover (e.g. querying
        // before the mesh's own adapter has been assigned its address
        // yet, or a stale adapter left over from a previous run) --
        // deliberately NOT relied on as the sole signal anymore.
        let name_lower = iface.name.to_lowercase();
        if name_lower == "loopback" || name_lower.starts_with("lanmesh") {
            continue;
        }
        // First non-excluded candidate wins; if multiple physical NICs
        // exist (e.g. Wi-Fi + Ethernet both up), this doesn't attempt to
        // rank them by route metric the way Linux's default-route lookup
        // implicitly does -- good enough for the common case, and still
        // strictly better than picking whatever the VPN's route
        // rewriting would otherwise cause the OS to choose.
        if best.is_none() {
            best = Some((iface.index, iface.name.clone()));
        }
    }
    best
}

/// Windows equivalent of the Linux `get_real_interface` above: enumerates
/// real system network interfaces via `netconfig_rs` and delegates the
/// actual selection to `pick_real_interface` (see its doc comment for the
/// full rationale, especially around why `my_virtual_ip` matters).
#[cfg(target_os = "windows")]
fn get_real_interface(my_virtual_ip: Option<Ipv4Addr>) -> Option<(u32, String)> {
    use netconfig_rs::sys::InterfaceExt;

    let interfaces = netconfig_rs::list_interfaces().ok()?;
    let candidates: Vec<InterfaceCandidate> = interfaces
        .into_iter()
        .filter_map(|iface| {
            let index = iface.index().ok()?;
            let addresses = iface.addresses().ok()?;
            let ipv4_addrs: Vec<Ipv4Addr> = addresses
                .iter()
                .filter_map(|a| match a.addr() {
                    std::net::IpAddr::V4(v4) => Some(v4),
                    _ => None,
                })
                .collect();
            Some(InterfaceCandidate {
                index,
                name: iface.name().unwrap_or_else(|_| format!("if{index}")),
                description: iface.description().unwrap_or_default(),
                ipv4_addrs,
            })
        })
        .collect();
    pick_real_interface(&candidates, my_virtual_ip)
}

/// `warp_compat` gates the interface-pinning logic below entirely: when
/// `false`, the socket is bound to `0.0.0.0` and left for the OS to route
/// normally, exactly like every other UDP application on the machine --
/// no interface enumeration, no `SO_BINDTODEVICE`/`IP_UNICAST_IF` calls
/// at all. This exists as an explicit escape hatch/diagnostic toggle: the
/// pinning logic exists specifically to work around always-on VPN/WARP
/// clients silently rerouting the mesh's own traffic, but on some
/// machines -- for reasons this project has no way to reproduce or
/// diagnose remotely (unusual adapter configurations, security software
/// intercepting `netconfig_rs`'s enumeration calls, etc.) -- the pinning
/// logic itself can misbehave even with no VPN present at all. Turning
/// this off trades away the WARP workaround for "definitely behaves like
/// a normal UDP socket," which is the right trade for anyone hitting that
/// failure mode and not running WARP/a VPN anyway.
fn create_udp_socket(
    listen_port: u16,
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))] my_virtual_ip: Option<Ipv4Addr>,
    warp_compat: bool,
) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    socket.set_reuse_address(true)?;
    if !warp_compat {
        log("WARP-compatibility interface pinning is disabled (warp_compat = false) -- \
the socket will use whatever route the OS picks normally.");
    } else {
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
            if let Some((index, name)) = get_real_interface(my_virtual_ip) {
                match bind_socket_to_interface_windows(&socket, index) {
                    Ok(()) => log(&format!("UDP socket bound to interface: {name} (index {index})")),
                    Err(e) => log(&format!(
                        "Could not bind UDP socket to interface '{name}' ({e}); continuing without it."
                    )),
                }
            } else {
                log("Warning: could not identify a non-VPN network interface to bind to; \
if you have Cloudflare WARP or another always-on VPN active, the mesh's \
self-detected public address may be wrong. If you are NOT using a VPN and self-STUN \
still isn't resolving, try disabling \"WARP compatibility\" in Settings.");
            }
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
    /// Our own best-guess LAN-facing address (e.g. 192.168.1.20:54321),
    /// discovered once at startup via `stun::discover_local_addr()` and
    /// gossiped alongside `my_public_addr` -- see `gossip::GossipEntry`'s
    /// `lan_addr` field and `peer.rs`'s `lan_candidate` for why this
    /// exists: it's what lets two peers behind the same router/public IP
    /// (where NAT hairpin/loopback often isn't supported) find each other
    /// over the LAN instead. `None` if local-address discovery failed, or
    /// on a machine with no real network interface at all.
    my_lan_addr: Option<SocketAddr>,
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
    /// Lets `MeshHandle` methods (manual address override, reset public
    /// address, etc.) wake the self-STUN ticker thread immediately
    /// instead of waiting up to `SELF_STUN_INTERVAL` for the change to
    /// take effect -- see `start()`'s self-STUN ticker for the receiving
    /// end. Wrapped in a `Mutex` purely because `mpsc::Sender` isn't
    /// `Sync`, not because concurrent sends need any coordination.
    stun_wake_tx: Mutex<mpsc::Sender<()>>,
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
            // Track whether the name is *actually* changing before
            // overwriting it -- this matters for the placeholder-name
            // case (see learn_peer_from_ping): a peer we first heard from
            // via an unsolicited PING gets a "peer-<ip>" placeholder name
            // that only gets corrected once gossip about them arrives.
            // If that gossip doesn't also carry a *fresher* address (very
            // common: we already know their current address just fine,
            // gossip is only telling us their real name), the address
            // branch below never fires and domain names would otherwise
            // silently keep showing the stale placeholder forever.
            let name_changed = !entry.name.is_empty() && existing.name() != entry.name;
            existing.set_name(&entry.name);
            // Unlike confirmed_addr, the LAN candidate has no
            // freshness-epoch gating (see `Peer::set_lan_candidate`'s
            // doc comment for why that's fine) -- but we only ever
            // *overwrite* it when this entry actually carries one, so a
            // re-gossiped entry that doesn't know about a candidate we
            // already learned some other way can't erase it.
            if entry.lan_addr.is_some() {
                existing.set_lan_candidate(entry.lan_addr);
            }
            if existing.observe_epoch(entry.addr, entry.epoch_secs) {
                return Some(GossipOutcome::AddressUpdated {
                    virtual_ip: entry.virtual_ip,
                    name: existing.name(),
                    addr: entry.addr,
                });
            }
            if name_changed {
                return Some(GossipOutcome::NameUpdated {
                    virtual_ip: entry.virtual_ip,
                    name: existing.name(),
                });
            }
            return None;
        }

        if self.peer_count() >= MAX_PEERS {
            return None;
        }
        let peer = Peer::from_gossip(entry.virtual_ip, &entry.name, entry.addr, entry.epoch_secs);
        peer.set_lan_candidate(entry.lan_addr);
        self.peers.write().unwrap().insert(entry.virtual_ip, Arc::new(peer));
        Some(GossipOutcome::NewPeer {
            virtual_ip: entry.virtual_ip,
            name: entry.name.clone(),
            addr: entry.addr,
        })
    }

    /// Applies one *service*-announcement gossip entry (see
    /// `GossipEntry::is_service`) to our local peer table: merges the
    /// service into the named peer's advertised-services list, creating
    /// the peer first (from the entry's own address/epoch) if this is
    /// somehow the first we've heard of them at all. Returns Some
    /// describing what happened if it's worth logging/re-syncing domain
    /// names over, None if it was a no-op (service already known
    /// unchanged, entry about ourselves, or the mesh is at its cap).
    fn apply_gossip_service(&self, entry: &GossipEntry) -> Option<GossipOutcome> {
        if entry.virtual_ip == self.my_virtual_ip {
            return None; // a service of ours, bounced back around via gossip -- ignore
        }
        let peer = match self.get_peer(&entry.virtual_ip) {
            Some(peer) => peer,
            None => {
                if self.peer_count() >= MAX_PEERS {
                    return None;
                }
                let peer = Arc::new(Peer::from_gossip(entry.virtual_ip, &entry.name, entry.addr, entry.epoch_secs));
                self.peers.write().unwrap().insert(entry.virtual_ip, peer.clone());
                peer
            }
        };
        if peer.observe_service(&entry.service_name, entry.service_port) {
            Some(GossipOutcome::ServiceAnnounced {
                virtual_ip: entry.virtual_ip,
                peer_name: peer.name(),
                service_name: entry.service_name.clone(),
                port: entry.service_port,
            })
        } else {
            None
        }
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
    let infos = build_domain_infos(state);
    let entries = hosts::build_entries_with_services(&state.domain_suffix, &infos);

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
        let dns_entries: Vec<dns::DnsEntry> = entries
            .into_iter()
            .map(|e| dns::DnsEntry {
                hostname: e.hostname,
                virtual_ip: e.virtual_ip,
                is_peer_root: e.is_peer_root,
            })
            .collect();
        dns::update_table(table, dns_entries);
    }
}

/// Builds the (name, ip, services) picture for ourselves plus every known
/// peer, for `hosts::build_entries_with_services` to expand into actual
/// hostnames. Our own services come straight from the live config (we
/// don't gossip to ourselves); peers' services come from whatever
/// service-announcement gossip has arrived so far.
fn build_domain_infos(state: &MeshState) -> Vec<hosts::PeerDomainInfo> {
    let my_services: Vec<(String, u16)> = state
        .config
        .lock()
        .unwrap()
        .services
        .iter()
        .map(|s| (s.name.clone(), s.port))
        .collect();

    let mut infos = vec![hosts::PeerDomainInfo {
        name: state.my_name.clone(),
        virtual_ip: state.my_virtual_ip,
        services: my_services,
    }];
    for peer in state.peers_snapshot() {
        infos.push(hosts::PeerDomainInfo {
            name: peer.name(),
            virtual_ip: peer.virtual_ip,
            services: peer.services(),
        });
    }
    infos
}

#[derive(Debug)]
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
    NameUpdated {
        virtual_ip: Ipv4Addr,
        name: String,
    },
    ServiceAnnounced {
        virtual_ip: Ipv4Addr,
        peer_name: String,
        service_name: String,
        port: u16,
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
/// every peer in our table (plus one service-announcement entry per
/// service we've heard they advertise, so services propagate multi-hop
/// exactly like addresses do), and an entry for ourselves -- plus our own
/// configured services -- if we've successfully self-STUN'd at least once
/// (so 2+-hop peers can learn how to reach us, not just the peers we've
/// directly exchanged addresses with).
fn build_local_gossip_entries(state: &MeshState) -> Vec<GossipEntry> {
    let mut entries: Vec<GossipEntry> = Vec::new();

    for p in state.peers_snapshot() {
        let Some(addr) = p.current_send_addr() else { continue };
        if addr.port() == 0 {
            continue;
        }
        let epoch = p.confirmed_epoch();
        let name = p.name();
        entries.push(GossipEntry::peer_with_lan(p.virtual_ip, &name, addr, epoch, p.lan_candidate()));
        for (service_name, port) in p.services() {
            entries.push(GossipEntry::service(p.virtual_ip, &name, addr, epoch, service_name, port));
        }
    }

    if let Some((addr, epoch)) = *state.my_public_addr.lock().unwrap() {
        entries.push(GossipEntry::peer_with_lan(
            state.my_virtual_ip,
            &state.my_name,
            addr,
            epoch,
            state.my_lan_addr,
        ));
        let my_services = state.config.lock().unwrap().services.clone();
        for service in my_services {
            entries.push(GossipEntry::service(
                state.my_virtual_ip,
                &state.my_name,
                addr,
                epoch,
                service.name,
                service.port,
            ));
        }
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
                if let Some((_, iface_name)) = get_real_interface(Some(self.state.my_virtual_ip)) {
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

    /// Adds (or updates the port of) a service *we* host, taking effect
    /// immediately in the running mesh -- it's included in the very next
    /// gossip burst (or right away, since this also nudges an immediate
    /// re-sync of domain names locally) and persisted to mesh.toml so it
    /// survives a restart. Used by the GUI's "add service" flow.
    pub fn add_service_live(&self, name: &str, port: u16) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let mut cfg = self.state.config.lock().unwrap();
        if let Some(existing) = cfg.services.iter_mut().find(|s| s.name.eq_ignore_ascii_case(name)) {
            existing.port = port;
        } else {
            cfg.services.push(crate::config::ServiceConfig {
                name: name.to_string(),
                port,
            });
        }
        let _ = cfg.save();
        drop(cfg);
        refresh_domain_names(&self.state);
    }

    /// Removes a service we host by name (case-insensitive). No-op if it
    /// wasn't configured. Takes effect immediately and persists.
    pub fn remove_service_live(&self, name: &str) {
        let mut cfg = self.state.config.lock().unwrap();
        let before = cfg.services.len();
        cfg.services.retain(|s| !s.name.eq_ignore_ascii_case(name));
        if cfg.services.len() != before {
            let _ = cfg.save();
        }
        drop(cfg);
        refresh_domain_names(&self.state);
    }

    /// Point-in-time view of this machine's local mesh domain names
    /// (itself plus every currently-known peer and their advertised
    /// services), for the GUI's Domains screen and the "open in browser"
    /// shortcut.
    pub fn domain_snapshot(&self) -> Vec<DomainNameEntry> {
        let suffix = self.state.config.lock().unwrap().me.domain_suffix.clone();
        let infos = build_domain_infos(&self.state);
        hosts::build_entries_with_services(&suffix, &infos)
            .into_iter()
            .map(|e| DomainNameEntry {
                hostname: e.hostname,
                virtual_ip: e.virtual_ip,
                is_peer_root: e.is_peer_root,
                port: e.port,
            })
            .collect()
    }

    /// Sets a manual public ip:port override, taking effect immediately
    /// (wakes the self-STUN thread rather than waiting for its next
    /// scheduled tick) and persisting to mesh.toml. While set, self-STUN
    /// discovery is skipped entirely and this exact address is
    /// advertised to peers -- see the self-STUN ticker's doc comment in
    /// `start()` for the full rationale. Used by the GUI/CLI's "manually
    /// enter public IP/port" action.
    pub fn set_manual_public_addr(&self, ip: Ipv4Addr, port: u16) {
        let mut cfg = self.state.config.lock().unwrap();
        cfg.me.manual_public_ip = Some(ip.to_string());
        cfg.me.manual_public_port = Some(port);
        if let Err(e) = cfg.save() {
            log(&format!("Warning: could not persist manual public address: {e}"));
        }
        drop(cfg);
        // Applied to the in-memory value immediately (not left for the
        // self-STUN thread to notice on its next wake) so a caller
        // reading `my_public_addr()`/`snapshot()` right after this call
        // returns -- e.g. a GUI updating its display -- sees the new
        // value straight away, the same way `clear_manual_public_addr`
        // and `reset_public_addr` already do for their own cases.
        *self.state.my_public_addr.lock().unwrap() = Some((SocketAddr::from((ip, port)), now_secs() as u32));
        self.wake_self_stun();
    }

    /// Clears a previously-set manual public address override, reverting
    /// to automatic self-STUN discovery immediately.
    pub fn clear_manual_public_addr(&self) {
        let mut cfg = self.state.config.lock().unwrap();
        cfg.me.manual_public_ip = None;
        cfg.me.manual_public_port = None;
        if let Err(e) = cfg.save() {
            log(&format!("Warning: could not persist manual public address change: {e}"));
        }
        drop(cfg);
        // Also clear the in-memory value right away rather than waiting
        // for the self-STUN thread to wake up and notice -- otherwise a
        // GUI showing "public address" would keep displaying the just-
        // cleared manual value for up to SELF_STUN_INTERVAL.
        *self.state.my_public_addr.lock().unwrap() = None;
        self.wake_self_stun();
    }

    /// "Reset public address" action: clears any cached public address
    /// (see `MeConfig::cache_public_addr`) AND clears the currently-known
    /// in-memory value, forcing a genuinely fresh self-STUN probe on the
    /// next wake instead of continuing to advertise a possibly-stale
    /// cached/previous value. Does NOT touch a manual override, if one is
    /// set -- that's a separate, more explicit choice the user has to
    /// clear themselves via `clear_manual_public_addr`.
    pub fn reset_public_addr(&self) {
        let mut cfg = self.state.config.lock().unwrap();
        cfg.clear_cached_public_addr();
        if let Err(e) = cfg.save() {
            log(&format!("Warning: could not persist public address reset: {e}"));
        }
        drop(cfg);
        *self.state.my_public_addr.lock().unwrap() = None;
        log("Public address reset -- re-running self-STUN discovery now.");
        self.wake_self_stun();
    }

    /// Point-in-time read of whatever public address the mesh is
    /// currently advertising (manual, self-STUN-discovered, or cached),
    /// for display purposes.
    pub fn my_public_addr(&self) -> Option<SocketAddr> {
        self.state.my_public_addr.lock().unwrap().map(|(a, _)| a)
    }

    fn wake_self_stun(&self) {
        let _ = self.state.stun_wake_tx.lock().unwrap().send(());
    }
}

/// UI-friendly view of one resolvable local domain name -- either a
/// peer's own root name, or a named service hung off one.
#[derive(Clone, Debug)]
pub struct DomainNameEntry {
    pub hostname: String,
    pub virtual_ip: Ipv4Addr,
    pub is_peer_root: bool,
    pub port: Option<u16>,
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
        log("No peers configured yet -- waiting for a bootstrap peer (import a card with `meow-meow import`).");
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
    let warp_compat = config.me.warp_compat;

    let dns_table = if dns_server_enabled { Some(dns::new_table()) } else { None };
    let (stun_wake_tx, stun_wake_rx) = mpsc::channel::<()>();

    // Best-effort: discover our own LAN-facing IP once at startup, so we
    // have a same-LAN fallback candidate to gossip -- see MeshState's
    // `my_lan_addr` doc comment. Failure here (e.g. no real network
    // interface) just means we won't offer a LAN candidate; the mesh
    // still works exactly as before over the public path.
    let my_lan_addr = stun::discover_local_addr();
    if let Some(ip) = my_lan_addr {
        log(&format!("Our LAN-facing address is {ip}:{listen_port} -- will be gossiped as a same-network fallback."));
    }

    let state = Arc::new(MeshState {
        cipher,
        my_virtual_ip,
        my_name,
        broadcast_addr,
        peers: RwLock::new(peers_map),
        my_public_addr: Mutex::new(None),
        my_lan_addr: my_lan_addr.map(|ip| SocketAddr::from((ip, listen_port))),
        self_stun: SelfStunWaiter::new(),
        config: Mutex::new(config),
        domain_suffix,
        sync_hosts_file,
        dns_table: dns_table.clone(),
        dns_handle: Mutex::new(None),
        stun_wake_tx: Mutex::new(stun_wake_tx),
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
                        if let Some((_, iface_name)) = get_real_interface(Some(my_virtual_ip)) {
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
    let dev_name = std::env::var("MEOW_MEOW_DEV_NAME").unwrap_or_else(|_| "meowmesh0".to_string());
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
        dev.name().unwrap_or_else(|_| "meowmesh0".to_string()),
        my_virtual_ip,
        prefix
    ));
    let dev = Arc::new(dev);

    // ---- Set up the UDP transport socket ----
    // Passing our own virtual IP lets get_real_interface() (Windows only)
    // authoritatively exclude the TUN adapter we just brought up above,
    // regardless of what name/description Windows happens to report for
    // it -- see get_real_interface()'s doc comment for why that matters.
    let sock = create_udp_socket(listen_port, Some(my_virtual_ip), warp_compat)?;
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
                        // device so `meow-meow ping` works without root and
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
                            let outcome = if entry.is_service() {
                                state.apply_gossip_service(&entry)
                            } else {
                                state.apply_gossip_entry(&entry)
                            };
                            if let Some(outcome) = outcome {
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
                                    GossipOutcome::NameUpdated { virtual_ip, name } => {
                                        // Address was already current (most
                                        // commonly: we learned this peer
                                        // from their own unsolicited PING
                                        // and gave them a placeholder name
                                        // -- see learn_peer_from_ping) --
                                        // only the display name changed.
                                        // Domain names (hosts file/DNS)
                                        // still need refreshing so the
                                        // placeholder doesn't linger
                                        // forever; the on-disk peer
                                        // address entry doesn't need
                                        // rewriting since it's unchanged,
                                        // but its name should match too.
                                        log(&format!(
                                            "Updated peer name via gossip: now known as '{name}' ({virtual_ip})"
                                        ));
                                        if let Some(peer) = state.get_peer(&virtual_ip) {
                                            if let Some(addr) = peer.current_send_addr() {
                                                persist_peer_addr(&state, virtual_ip, &name, addr);
                                            }
                                        }
                                        refresh_domain_names(&state);
                                    }
                                    GossipOutcome::ServiceAnnounced { virtual_ip, peer_name, service_name, port } => {
                                        log(&format!(
                                            "Discovered service '{service_name}' on '{peer_name}' ({virtual_ip}) via gossip -> port {port} \
(reachable as {service_name}.<{peer_name}'s mesh name>.{})",
                                            state.domain_suffix
                                        ));
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
                    let wire_plain = proto::build(proto::TYPE_PING, state.my_virtual_ip, &seq.to_be_bytes());
                    let wire = state.cipher.seal(&wire_plain);
                    let mut pinged = false;
                    if let Some(addr) = peer.current_send_addr() {
                        // A TYPE_PING doubles as the keepalive itself (it
                        // still refreshes the NAT mapping and updates
                        // last-seen via the PONG's observe() call), while
                        // additionally giving us a live RTT reading for
                        // the periodic status log.
                        let _ = sock.send_to(&wire, addr);
                        pinged = true;
                    }
                    // Also probe the peer's self-reported LAN candidate,
                    // if any and if it's a different address than the one
                    // above -- this is what lets two peers behind the
                    // same router/public IP (where the public path often
                    // silently never works at all, since it depends on
                    // NAT hairpin/loopback support most consumer routers
                    // lack) find each other over the LAN instead. Cheap
                    // and harmless to keep trying indefinitely on peers
                    // that turn out to be on a different network: it's
                    // just one extra small UDP packet every
                    // KEEPALIVE_INTERVAL that will simply never get a
                    // reply, exactly like pinging any other unreachable
                    // address. Whichever candidate (public or LAN)
                    // actually answers is what `Peer::observe()` promotes
                    // to `confirmed_addr` on the receiving end -- no
                    // separate "prefer LAN" logic needed here at all.
                    if let Some(lan_addr) = peer.lan_candidate() {
                        if Some(lan_addr) != peer.current_send_addr() {
                            let _ = sock.send_to(&wire, lan_addr);
                            pinged = true;
                        }
                    }
                    if pinged {
                        peer.record_ping_sent(seq);
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
    //
    // Checked in this order on every wake (either the normal
    // SELF_STUN_INTERVAL tick, or an immediate wake-up via
    // `MeshHandle`'s manual-override/reset methods -- see
    // `stun_wake_rx.recv_timeout` below):
    //  1. Manual override (`manual_public_ip`/`manual_public_port` both
    //     set) -- self-STUN is skipped entirely, this exact address is
    //     used until the override is cleared. This is the escape hatch
    //     for networks where STUN itself is blocked/unreliable, or a
    //     user who already knows their address (e.g. a server with a
    //     static IP and a manually port-forwarded router) and would
    //     rather not depend on STUN at all. Re-checked every tick (not
    //     just once at startup) so toggling it live via the GUI/CLI takes
    //     effect on the very next wake, not just after a restart.
    //  2. Normal self-STUN probing (unchanged from before).
    //  3. If self-STUN fails AND a cached address exists
    //     (`cache_public_addr` enabled with a previously-successful
    //     value on file), fall back to the cached value instead of
    //     leaving `my_public_addr` unset -- this is what lets the mesh
    //     start immediately usable on a subsequent launch even before
    //     the first successful probe of *this* run completes, and keeps
    //     working indefinitely if self-STUN can never succeed at all on
    //     a given network.
    {
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || {
            // Seed from a cached address immediately at startup, if one
            // exists, so the mesh has *something* to gossip/advertise
            // right away instead of waiting for the first successful
            // probe. Skipped if a manual override is already set, since
            // that takes priority and is applied by the loop below on
            // its very first iteration anyway.
            let seed = {
                let cfg = state.config.lock().unwrap();
                if cfg.manual_public_addr().is_none() { cfg.cached_public_addr() } else { None }
            };
            if let Some(cached_addr) = seed {
                log(&format!(
                    "Starting with cached public address {cached_addr} while self-STUN discovery runs in the background."
                ));
                *state.my_public_addr.lock().unwrap() = Some((cached_addr, now_secs() as u32));
                send_gossip_burst(&state, &sock);
            }

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                // IMPORTANT: `manual_public_addr()` must be captured into an
                // owned value *before* the `if`/`else`, not called directly
                // inside the `if let` condition -- Rust's temporary lifetime
                // extension keeps a `MutexGuard` produced inside an `if let
                // ... = EXPR { .. } else { .. }` condition alive for the
                // *entire* if/else, including the else branch. Since the
                // `else` branch below also needs to lock `state.config`
                // (for caching a freshly self-STUN-discovered address),
                // that used to self-deadlock forever the very first time
                // self-STUN succeeded with `cache_public_addr` enabled --
                // silently, since the deadlocked thread just stops logging
                // instead of panicking. Binding to a plain `Option` first
                // drops the guard immediately after the statement.
                let manual_addr_now = state.config.lock().unwrap().manual_public_addr();
                if let Some(manual_addr) = manual_addr_now {
                    let changed = state.my_public_addr.lock().unwrap().map(|(a, _)| a) != Some(manual_addr);
                    if changed {
                        log(&format!(
                            "Using manually configured public address {manual_addr} -- self-STUN discovery is disabled while this is set."
                        ));
                        *state.my_public_addr.lock().unwrap() = Some((manual_addr, now_secs() as u32));
                        send_gossip_burst(&state, &sock);
                    }
                } else {
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
                            let mut cfg = state.config.lock().unwrap();
                            if cfg.me.cache_public_addr {
                                let already_cached = cfg.cached_public_addr() == Some(addr);
                                if !already_cached {
                                    cfg.set_cached_public_addr(addr);
                                    if let Err(e) = cfg.save() {
                                        log(&format!("Warning: could not persist cached public address: {e}"));
                                    }
                                }
                            }
                        }
                        None => {
                            let has_cache = state.config.lock().unwrap().cached_public_addr().is_some();
                            if has_cache {
                                log("Warning: could not determine our own external address via STUN this round; \
continuing to use the cached address (will keep retrying STUN in the background).");
                            } else {
                                log("Warning: could not determine our own external address via STUN this round (will retry). \
If this keeps happening, try Settings -> manually enter your public IP/port, or enable \
\"cache public address\" once STUN succeeds at least once so future launches aren't blocked on it.");
                            }
                        }
                    }
                }

                // Sleep by waiting on the wake channel instead of a plain
                // thread::sleep, so MeshHandle::set_manual_public_addr /
                // clear_manual_public_addr / reset_public_addr can force
                // an immediate re-check instead of waiting up to
                // SELF_STUN_INTERVAL for the change to be picked up.
                // Ignoring the Result: a RecvTimeoutError::Disconnected
                // (sender dropped) is impossible here since `state` (and
                // therefore its stun_wake_tx) outlives this thread, and
                // Timeout is the expected/normal case every interval.
                let _ = stun_wake_rx.recv_timeout(SELF_STUN_INTERVAL);
            }
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
/// without root/Administrator and can be run even while `meow-meow run` is
/// not active elsewhere (though it obviously can't measure anything for a
/// peer whose mesh isn't currently running to answer).
pub fn ping(config: Config, count: u32, timeout: Duration) -> std::io::Result<()> {
    let cipher = Cipher::from_psk_b64(&config.me.psk)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if config.peers.is_empty() {
        println!("No peers configured. Use `meow-meow add-peer` or `meow-meow import`.");
        return Ok(());
    }

    let my_virtual_ip = config.me.virtual_ip;
    let sock = create_udp_socket(config.me.listen_port, Some(my_virtual_ip), config.me.warp_compat)?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MeConfig;

    /// Bare `MeshState` for exercising the gossip-application logic in
    /// isolation, without any real sockets/threads/TUN device -- those
    /// are only ever created inside `start()`, which these tests don't
    /// call.
    fn test_state() -> MeshState {
        let cipher = Cipher::from_psk_b64(&Cipher::generate_psk_b64()).unwrap();
        let config = Config {
            me: MeConfig {
                name: "me".to_string(),
                virtual_ip: "10.0.0.1".parse().unwrap(),
                prefix: 24,
                listen_port: 12345,
                psk: String::new(),
                mtu: 1400,
                domain_suffix: "mesh".to_string(),
                sync_hosts_file: false,
                dns_server: false,
                dns_port: 53,
                dns_auto_configure: false,
                manual_public_ip: None,
                manual_public_port: None,
                warp_compat: true,
                cache_public_addr: false,
                cached_public_ip: None,
                cached_public_port: None,
            },
            peers: Vec::new(),
            services: Vec::new(),
        };
        MeshState {
            cipher,
            my_virtual_ip: config.me.virtual_ip,
            my_name: config.me.name.clone(),
            broadcast_addr: config.broadcast_addr(),
            peers: RwLock::new(HashMap::new()),
            my_public_addr: Mutex::new(None),
            my_lan_addr: None,
            self_stun: SelfStunWaiter::new(),
            domain_suffix: config.me.domain_suffix.clone(),
            sync_hosts_file: config.me.sync_hosts_file,
            dns_table: None,
            dns_handle: Mutex::new(None),
            config: Mutex::new(config),
            stun_wake_tx: Mutex::new(mpsc::channel().0),
        }
    }

    #[test]
    fn learn_peer_from_ping_then_gossip_name_only_update_is_reported() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let addr: SocketAddr = "203.0.113.5:4000".parse().unwrap();

        // Step 1: peer reaches us first via an unsolicited PING -- gets a
        // placeholder name (mirrors the receive loop's real behavior).
        let outcome = state.learn_peer_from_ping(peer_ip, addr);
        assert!(matches!(outcome, Some(GossipOutcome::NewPeer { .. })));
        assert_eq!(state.get_peer(&peer_ip).unwrap().name(), "peer-10.0.0.2");

        // Step 2: gossip arrives with their real name, but the SAME
        // address/epoch we already have (a very common case -- we
        // already know how to reach them fine, gossip is only useful
        // here for the name). Before the fix, this returned None and the
        // placeholder name would never get corrected in domain names.
        let entry = GossipEntry::peer(peer_ip, "alice", addr, now_secs() as u32);
        // Bump epoch so it's not silently ignored as stale, but keep the
        // address identical to isolate the name-only-change path.
        let entry = GossipEntry {
            epoch_secs: state.get_peer(&peer_ip).unwrap().confirmed_epoch(),
            ..entry
        };
        let outcome = state.apply_gossip_entry(&entry);
        match outcome {
            Some(GossipOutcome::NameUpdated { virtual_ip, name }) => {
                assert_eq!(virtual_ip, peer_ip);
                assert_eq!(name, "alice");
            }
            other => panic!("expected NameUpdated, got {other:?}"),
        }
        assert_eq!(state.get_peer(&peer_ip).unwrap().name(), "alice");
    }

    #[test]
    fn gossip_with_fresher_address_reports_address_updated_not_name_updated() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.3".parse().unwrap();
        let old_addr: SocketAddr = "203.0.113.5:4000".parse().unwrap();
        state.learn_peer_from_ping(peer_ip, old_addr);

        let new_addr: SocketAddr = "203.0.113.5:5000".parse().unwrap();
        let fresher_epoch = state.get_peer(&peer_ip).unwrap().confirmed_epoch() + 10;
        let entry = GossipEntry::peer(peer_ip, "bob", new_addr, fresher_epoch);
        match state.apply_gossip_entry(&entry) {
            Some(GossipOutcome::AddressUpdated { virtual_ip, name, addr }) => {
                assert_eq!(virtual_ip, peer_ip);
                assert_eq!(name, "bob");
                assert_eq!(addr, new_addr);
            }
            other => panic!("expected AddressUpdated, got {other:?}"),
        }
    }

    #[test]
    fn new_peer_learned_via_gossip_stores_its_lan_candidate() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.20".parse().unwrap();
        let public_addr: SocketAddr = "203.0.113.7:1024".parse().unwrap();
        let lan_addr: SocketAddr = "192.168.1.20:54321".parse().unwrap();
        let entry = GossipEntry::peer_with_lan(peer_ip, "server", public_addr, 1000, Some(lan_addr));
        state.apply_gossip_entry(&entry);
        assert_eq!(state.get_peer(&peer_ip).unwrap().lan_candidate(), Some(lan_addr));
    }

    #[test]
    fn existing_peer_gains_a_lan_candidate_from_a_later_gossip_entry() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.21".parse().unwrap();
        let public_addr: SocketAddr = "203.0.113.7:1024".parse().unwrap();
        state.learn_peer_from_ping(peer_ip, public_addr);
        assert_eq!(state.get_peer(&peer_ip).unwrap().lan_candidate(), None);

        let lan_addr: SocketAddr = "192.168.1.20:54321".parse().unwrap();
        let entry = GossipEntry::peer_with_lan(peer_ip, "server", public_addr, 2000, Some(lan_addr));
        state.apply_gossip_entry(&entry);
        assert_eq!(state.get_peer(&peer_ip).unwrap().lan_candidate(), Some(lan_addr));
    }

    #[test]
    fn gossip_entry_without_a_lan_candidate_does_not_erase_a_previously_learned_one() {
        // A peer might be re-gossiped by a third party who doesn't happen
        // to know its LAN candidate (e.g. an older build, or simply a
        // peer that never learned it) -- that must not blow away a LAN
        // candidate we already learned some other way.
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.22".parse().unwrap();
        let public_addr: SocketAddr = "203.0.113.7:1024".parse().unwrap();
        let lan_addr: SocketAddr = "192.168.1.20:54321".parse().unwrap();
        state.apply_gossip_entry(&GossipEntry::peer_with_lan(peer_ip, "server", public_addr, 1000, Some(lan_addr)));
        assert_eq!(state.get_peer(&peer_ip).unwrap().lan_candidate(), Some(lan_addr));

        // Fresher entry, but with no LAN candidate attached.
        state.apply_gossip_entry(&GossipEntry::peer(peer_ip, "server", public_addr, 2000));
        assert_eq!(state.get_peer(&peer_ip).unwrap().lan_candidate(), Some(lan_addr));
    }

    #[test]
    fn same_router_scenario_lan_candidate_recovers_when_public_path_is_unreachable() {
        // Reproduces the real-world bug report this feature fixes: two
        // peers behind the same router/public IP where the "public" path
        // is a dead end (either because the router doesn't support NAT
        // hairpin/loopback, or -- as modeled here -- because it's simply
        // some address nothing is listening on), but they're really on
        // the same LAN and reachable that way instead.
        //
        // Uses real UDP sockets on loopback rather than the full
        // mesh::start() (which needs a real TUN adapter and root/admin
        // privileges) -- this exercises the actual send-to-both-
        // candidates-then-let-observe()-pick-the-winner mechanism from
        // the keepalive ticker, just without the TUN/thread machinery
        // around it.
        use std::net::UdpSocket;

        // Stands in for the peer's real LAN-facing socket -- this one
        // actually receives our probe.
        let lan_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let lan_addr = lan_sock.local_addr().unwrap();
        lan_sock.set_read_timeout(Some(std::time::Duration::from_millis(500))).unwrap();

        // Stands in for the "public" address: a real bound-but-abandoned
        // socket, immediately dropped so the port becomes unreachable --
        // sends to it are accepted by the OS but nothing is listening,
        // modeling a router that can't hairpin traffic back to itself.
        let dead_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead_sock.local_addr().unwrap();
        drop(dead_sock);

        let peer = Peer::from_gossip("10.0.0.30".parse().unwrap(), "server", dead_addr, 1000);
        peer.set_lan_candidate(Some(lan_addr));
        assert_eq!(peer.current_send_addr(), Some(dead_addr));

        // Exactly what the keepalive ticker does: probe both candidates.
        let prober = UdpSocket::bind("127.0.0.1:0").unwrap();
        prober.send_to(b"ping-public", dead_addr).unwrap();
        prober.send_to(b"ping-lan", lan_addr).unwrap();

        // Only the LAN socket actually receives anything.
        let mut buf = [0u8; 64];
        let (n, from) = lan_sock.recv_from(&mut buf).expect("LAN candidate should have received the probe");
        assert_eq!(&buf[..n], b"ping-lan");
        assert_eq!(from, prober.local_addr().unwrap());

        // The receiving side would now reply, and our observe() on
        // receiving that reply is what promotes the LAN address to
        // confirmed_addr -- modeled directly here since that part is
        // already covered by peer::tests.
        assert!(peer.observe(lan_addr));
        assert_eq!(peer.current_send_addr(), Some(lan_addr));
    }

    #[test]
    fn gossip_with_unchanged_name_and_stale_address_is_a_true_noop() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.4".parse().unwrap();
        let addr: SocketAddr = "203.0.113.5:4000".parse().unwrap();
        state.learn_peer_from_ping(peer_ip, addr);
        // First, give it a real name.
        let epoch = state.get_peer(&peer_ip).unwrap().confirmed_epoch();
        state.apply_gossip_entry(&GossipEntry::peer(peer_ip, "carol", addr, epoch));
        assert_eq!(state.get_peer(&peer_ip).unwrap().name(), "carol");

        // Same name, same (now-stale, lower-or-equal) epoch -- should be
        // a genuine no-op, not reported as any kind of update.
        let outcome = state.apply_gossip_entry(&GossipEntry::peer(peer_ip, "carol", addr, epoch));
        assert!(outcome.is_none());
    }

    #[test]
    fn apply_gossip_service_creates_unknown_peer_and_reports_announcement() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        let addr: SocketAddr = "203.0.113.9:9000".parse().unwrap();
        let entry = GossipEntry::service(peer_ip, "dave", addr, now_secs() as u32, "game", 25565);
        match state.apply_gossip_service(&entry) {
            Some(GossipOutcome::ServiceAnnounced { virtual_ip, peer_name, service_name, port }) => {
                assert_eq!(virtual_ip, peer_ip);
                assert_eq!(peer_name, "dave");
                assert_eq!(service_name, "game");
                assert_eq!(port, 25565);
            }
            other => panic!("expected ServiceAnnounced, got {other:?}"),
        }
        assert_eq!(state.get_peer(&peer_ip).unwrap().services(), vec![("game".to_string(), 25565)]);
    }

    #[test]
    fn apply_gossip_service_is_noop_when_unchanged() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.6".parse().unwrap();
        let addr: SocketAddr = "203.0.113.9:9000".parse().unwrap();
        let entry = GossipEntry::service(peer_ip, "eve", addr, now_secs() as u32, "web", 8080);
        assert!(state.apply_gossip_service(&entry).is_some());
        assert!(state.apply_gossip_service(&entry).is_none());
    }

    #[test]
    fn refresh_domain_names_includes_peer_root_and_service_subdomains() {
        let state = test_state();
        let peer_ip: Ipv4Addr = "10.0.0.7".parse().unwrap();
        let addr: SocketAddr = "203.0.113.9:9000".parse().unwrap();
        state.learn_peer_from_ping(peer_ip, addr);
        let epoch = state.get_peer(&peer_ip).unwrap().confirmed_epoch();
        state.apply_gossip_entry(&GossipEntry::peer(peer_ip, "frank", addr, epoch));
        state.apply_gossip_service(&GossipEntry::service(peer_ip, "frank", addr, epoch, "game", 25565));

        let infos = build_domain_infos(&state);
        let frank = infos.iter().find(|i| i.virtual_ip == peer_ip).unwrap();
        assert_eq!(frank.name, "frank");
        assert_eq!(frank.services, vec![("game".to_string(), 25565)]);
    }

    // ---- pick_real_interface: Windows self-STUN-never-resolves fix ----
    //
    // These exercise the pure interface-selection logic in isolation
    // (see pick_real_interface's doc comment for the full story of the
    // bug being fixed here). They run on every platform's CI, not just
    // Windows, since the function itself has no OS dependency -- only
    // the real enumeration wrapper around it (get_real_interface) is
    // Windows-only.

    fn candidate(index: u32, name: &str, description: &str, ipv4_addrs: &[&str]) -> InterfaceCandidate {
        InterfaceCandidate {
            index,
            name: name.to_string(),
            description: description.to_string(),
            ipv4_addrs: ipv4_addrs.iter().map(|s| s.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn picks_the_only_real_adapter() {
        let candidates = vec![candidate(5, "Ethernet", "Realtek PCIe GbE Family Controller", &["192.168.1.10"])];
        let picked = pick_real_interface(&candidates, Some("10.66.0.1".parse().unwrap()));
        assert_eq!(picked, Some((5, "Ethernet".to_string())));
    }

    #[test]
    fn excludes_own_tun_adapter_by_address_even_with_a_misleading_name_and_description() {
        // Reproduces the exact real-world failure mode: our own mesh TUN
        // adapter surfaces with neither a "lanmesh"-prefixed name NOR a
        // "wintun"-containing description (both entirely plausible on
        // real Windows installs -- see this function's doc comment) --
        // only its known virtual IP identifies it as ours. Before the
        // fix, this candidate would have been wrongly selected as the
        // "real" interface, pinning the mesh's own UDP socket (and every
        // self-STUN probe) to itself -- which has no route to the
        // internet, so self-STUN would time out and retry forever.
        let candidates = vec![
            candidate(9, "Ethernet 3", "meowmesh0", &["10.66.0.1"]), // our own adapter, misleadingly named
            candidate(3, "Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", &["192.168.1.11"]),
        ];
        let picked = pick_real_interface(&candidates, Some("10.66.0.1".parse().unwrap()));
        assert_eq!(picked, Some((3, "Wi-Fi".to_string())));
    }

    #[test]
    fn falls_back_to_name_heuristic_when_virtual_ip_unknown() {
        // If we don't yet know our own virtual IP (shouldn't normally
        // happen at the point this is called in practice, but defence in
        // depth), the legacy name-prefix heuristic still applies.
        let candidates = vec![
            candidate(9, "meowmesh0", "meowmesh0", &["10.66.0.1"]),
            candidate(3, "Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", &["192.168.1.11"]),
        ];
        let picked = pick_real_interface(&candidates, None);
        assert_eq!(picked, Some((3, "Wi-Fi".to_string())));
    }

    #[test]
    fn excludes_cloudflare_warp_by_description() {
        let candidates = vec![
            candidate(9, "Ethernet 5", "Cloudflare WARP Interface Tunnel", &["100.96.0.5"]),
            candidate(3, "Ethernet", "Realtek PCIe GbE Family Controller", &["192.168.1.10"]),
        ];
        let picked = pick_real_interface(&candidates, Some("10.66.0.1".parse().unwrap()));
        assert_eq!(picked, Some((3, "Ethernet".to_string())));
    }

    #[test]
    fn excludes_adapters_with_no_ipv4_address() {
        let candidates = vec![
            candidate(9, "Disabled NIC", "Some Adapter", &[]),
            candidate(3, "Ethernet", "Realtek PCIe GbE Family Controller", &["192.168.1.10"]),
        ];
        let picked = pick_real_interface(&candidates, Some("10.66.0.1".parse().unwrap()));
        assert_eq!(picked, Some((3, "Ethernet".to_string())));
    }

    #[test]
    fn returns_none_when_only_our_own_adapter_and_vpns_are_present() {
        let candidates = vec![
            candidate(9, "meowmesh0", "meowmesh0", &["10.66.0.1"]),
            candidate(4, "Ethernet 5", "Cloudflare WARP Interface Tunnel", &["100.96.0.5"]),
        ];
        let picked = pick_real_interface(&candidates, Some("10.66.0.1".parse().unwrap()));
        assert_eq!(picked, None);
    }

    #[test]
    fn first_non_excluded_candidate_wins_when_multiple_real_nics_present() {
        let candidates = vec![
            candidate(2, "Ethernet", "Realtek PCIe GbE Family Controller", &["192.168.1.10"]),
            candidate(3, "Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", &["192.168.1.11"]),
        ];
        let picked = pick_real_interface(&candidates, Some("10.66.0.1".parse().unwrap()));
        assert_eq!(picked, Some((2, "Ethernet".to_string())));
    }

    // ---- MeshHandle: manual public address / caching / reset ----
    //
    // Constructed directly from `test_state()` (same module, so private
    // fields are accessible) rather than through `start()`, since these
    // methods only ever touch `MeshState`/`Config` and the wake channel
    // -- no real socket or TUN device needed to exercise them.

    fn test_handle() -> MeshHandle {
        MeshHandle {
            state: Arc::new(test_state()),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    #[test]
    fn set_manual_public_addr_updates_config_and_live_state() {
        let handle = test_handle();
        assert_eq!(handle.my_public_addr(), None);

        handle.set_manual_public_addr("203.0.113.5".parse().unwrap(), 4000);

        assert_eq!(handle.my_public_addr(), Some("203.0.113.5:4000".parse().unwrap()));
        let cfg = handle.config_snapshot();
        assert_eq!(cfg.me.manual_public_ip.as_deref(), Some("203.0.113.5"));
        assert_eq!(cfg.me.manual_public_port, Some(4000));
    }

    #[test]
    fn clear_manual_public_addr_clears_config_and_live_state_immediately() {
        let handle = test_handle();
        handle.set_manual_public_addr("203.0.113.5".parse().unwrap(), 4000);
        assert!(handle.my_public_addr().is_some());

        handle.clear_manual_public_addr();

        // Cleared immediately -- not left showing the stale manual value
        // until some background thread notices (there's no background
        // thread wired up in this test harness at all, which is exactly
        // why the immediate in-memory clear inside the method itself
        // matters).
        assert_eq!(handle.my_public_addr(), None);
        let cfg = handle.config_snapshot();
        assert_eq!(cfg.me.manual_public_ip, None);
        assert_eq!(cfg.me.manual_public_port, None);
    }

    #[test]
    fn reset_public_addr_clears_cache_and_live_value_but_not_manual_override() {
        let handle = test_handle();
        {
            let mut cfg = handle.state.config.lock().unwrap();
            cfg.me.cache_public_addr = true;
            cfg.set_cached_public_addr("203.0.113.9:9000".parse().unwrap());
            cfg.me.manual_public_ip = Some("198.51.100.1".to_string());
            cfg.me.manual_public_port = Some(1234);
        }
        *handle.state.my_public_addr.lock().unwrap() = Some(("203.0.113.9:9000".parse().unwrap(), 1));

        handle.reset_public_addr();

        assert_eq!(handle.my_public_addr(), None);
        let cfg = handle.config_snapshot();
        assert_eq!(cfg.me.cached_public_ip, None);
        assert_eq!(cfg.me.cached_public_port, None);
        // Manual override is a separate, more explicit choice -- reset
        // must not silently clear it too.
        assert_eq!(cfg.me.manual_public_ip.as_deref(), Some("198.51.100.1"));
        assert_eq!(cfg.me.manual_public_port, Some(1234));
    }

    #[test]
    fn set_manual_public_addr_wakes_the_stun_channel() {
        let handle = test_handle();
        let (tx, rx) = mpsc::channel();
        *handle.state.stun_wake_tx.lock().unwrap() = tx;

        handle.set_manual_public_addr("203.0.113.5".parse().unwrap(), 4000);

        assert!(rx.try_recv().is_ok(), "expected a wake signal after setting a manual override");
    }
}
