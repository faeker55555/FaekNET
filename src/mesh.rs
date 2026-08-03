use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use socket2::{Domain, Socket, Type};
use tun_rs::DeviceBuilder;

use crate::config::Config;
use crate::crypto::Cipher;
use crate::peer::Peer;
use crate::proto;

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const RECV_LOOP_TIMEOUT: Duration = Duration::from_secs(30);

pub fn log(msg: &str) {
    let now = chrono::Local::now().format("%H:%M:%S");
    println!("[{}] {}", now, msg);
}

/// Finds the real physical/internet-facing network interface name, so the
/// mesh's UDP socket can be pinned to it. This matters on machines that
/// also have the mesh's own virtual TUN adapter and/or VPN adapters
/// present: without pinning, the OS could pick an unexpected route for
/// mesh traffic, especially once the TUN device brings up a route for the
/// mesh subnet.
fn get_real_interface() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let output = Command::new("sh")
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
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
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
    let addr: SocketAddr = format!("0.0.0.0:{}", listen_port).parse().unwrap();
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

pub struct MeshState {
    pub cipher: Cipher,
    pub my_virtual_ip: Ipv4Addr,
    pub broadcast_addr: Ipv4Addr,
    pub peers_by_ip: HashMap<Ipv4Addr, Arc<Peer>>,
    pub peer_list: Vec<Arc<Peer>>,
}

pub fn run(config: Config) -> std::io::Result<()> {
    let cipher = Cipher::from_psk_b64(&config.me.psk)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if config.peers.is_empty() {
        log("Warning: no peers configured yet. Add some with `lan_mesh add-peer`.");
    }

    let mut peers_by_ip = HashMap::new();
    let mut peer_list = Vec::new();
    for p in &config.peers {
        let peer = Arc::new(Peer::new(p));
        peers_by_ip.insert(p.virtual_ip, peer.clone());
        peer_list.push(peer);
    }

    let state = Arc::new(MeshState {
        cipher,
        my_virtual_ip: config.me.virtual_ip,
        broadcast_addr: config.broadcast_addr(),
        peers_by_ip,
        peer_list,
    });

    // ---- Set up the virtual network adapter ----
    let dev_name = std::env::var("LAN_MESH_DEV_NAME").unwrap_or_else(|_| "lanmesh0".to_string());
    let dev = DeviceBuilder::new()
        .name(dev_name)
        .ipv4(config.me.virtual_ip, config.me.prefix, None)
        .mtu(config.me.mtu)
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
        config.me.virtual_ip,
        config.me.prefix
    ));
    let dev = Arc::new(dev);

    // ---- Set up the UDP transport socket ----
    let sock = create_udp_socket(config.me.listen_port)?;
    log(&format!("Listening on UDP 0.0.0.0:{}", config.me.listen_port));
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
                    for peer in &state.peer_list {
                        if let Some(addr) = peer.current_send_addr() {
                            let _ = sock.send_to(&wire, addr);
                        }
                    }
                } else if let Some(peer) = state.peers_by_ip.get(&dst) {
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
                let Some(plain) = state.cipher.open(&buf[..n]) else {
                    // Wrong PSK, corrupted, or random internet noise -- drop.
                    continue;
                };
                let Some((hdr, payload)) = proto::parse(&plain) else {
                    continue;
                };
                // Only accept traffic claiming to be from a configured peer.
                let Some(peer) = state.peers_by_ip.get(&hdr.sender_virtual_ip) else {
                    continue;
                };
                let was_new = peer.seconds_since_seen().is_none();
                peer.observe(from_addr);
                if was_new {
                    log(&format!(
                        "Peer '{}' ({}) is now reachable at {}",
                        peer.name, peer.virtual_ip, from_addr
                    ));
                }

                if hdr.packet_type == proto::TYPE_DATA && !payload.is_empty() {
                    if let Err(e) = dev.send(payload) {
                        log(&format!("TUN write error: {e}"));
                    }
                }
                // TYPE_KEEPALIVE: observe() above already did the useful work.
            }
        });
    }

    // ---- Keepalive / hole-punch ticker ----
    {
        let sock = sock.clone();
        let state = state.clone();
        let running = running.clone();
        thread::spawn(move || {
            // Immediate burst on startup, similar in spirit to the chat
            // program's diagnostic pings: this is what actually opens the
            // NAT/CGNAT mapping on each side before any real traffic needs
            // to flow.
            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                for peer in &state.peer_list {
                    if let Some(addr) = peer.current_send_addr() {
                        let wire_plain =
                            proto::build(proto::TYPE_KEEPALIVE, state.my_virtual_ip, &[]);
                        let wire = state.cipher.seal(&wire_plain);
                        let _ = sock.send_to(&wire, addr);
                    }
                }
                thread::sleep(KEEPALIVE_INTERVAL);
            }
        });
    }

    log("Mesh is running. Press Ctrl+C to stop.");
    log_peer_summary(&state);

    // Main thread just waits; a real Ctrl+C handler could set `running` to
    // false for clean shutdown, but process exit tears everything down
    // anyway.
    loop {
        thread::sleep(Duration::from_secs(60));
        log_peer_summary(&state);
    }
}

fn log_peer_summary(state: &MeshState) {
    if state.peer_list.is_empty() {
        return;
    }
    for peer in &state.peer_list {
        let status = match peer.seconds_since_seen() {
            Some(s) => format!("reachable, last heard {s}s ago"),
            None => "not yet reachable".to_string(),
        };
        log(&format!(
            "  peer '{}' ({}) @ {} -- {}",
            peer.name,
            peer.virtual_ip,
            peer.current_send_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "unresolved".to_string()),
            status
        ));
    }
}
