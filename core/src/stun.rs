// Minimal RFC 5389 STUN Binding Request client, used only to help the user
// discover their own external ip:port for a given local port -- exactly
// the same technique used to diagnose the CGNAT port-remapping issue
// earlier. This is purely a discovery aid for the user to fill in the
// peer's config; the mesh's actual data path never depends on a STUN
// server at runtime.
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

const MAGIC_COOKIE: u32 = 0x2112A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE_SUCCESS: u16 = 0x0101;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const MAPPED_ADDRESS: u16 = 0x0001;

/// Well-known public STUN servers, tried in order until one answers.
/// Deliberately spread across several unrelated operators/networks
/// (Google, Cloudflare, Yandex, plus a handful of independent/community
/// servers) rather than relying on any single provider, since a STUN
/// probe only needs ONE of these to succeed -- more independent servers
/// means more resilience against any one of them being blocked,
/// rate-limited, or temporarily down on a given network. This is also a
/// direct mitigation for "self-STUN never resolves": if the reason is
/// that a specific server (or a specific provider's IP ranges) is
/// blocked on that network/firewall rather than STUN in general, trying
/// several unrelated ones gives a real chance of finding one that isn't.
pub const DEFAULT_SERVERS: &[(&str, u16)] = &[
    ("stun.l.google.com", 19302),
    ("stun1.l.google.com", 19302),
    ("stun2.l.google.com", 19302),
    ("stun3.l.google.com", 19302),
    ("stun4.l.google.com", 19302),
    ("stun.cloudflare.com", 3478),
    ("stun.rtc.yandex.net", 3478),
    ("stun.nextcloud.com", 3478),
    ("stunserver2025.stunprotocol.org", 3478),
    ("stun.sipnet.ru", 3478),
    ("stun.miwifi.com", 3478),
];

fn build_request(tx_id: &[u8; 12]) -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    buf[2..4].copy_from_slice(&0u16.to_be_bytes());
    buf[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    buf[8..20].copy_from_slice(tx_id);
    buf
}

fn parse_response(data: &[u8], tx_id: &[u8; 12]) -> Option<SocketAddr> {
    if data.len() < 20 {
        return None;
    }
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if msg_type != BINDING_RESPONSE_SUCCESS {
        return None;
    }
    if &data[8..20] != tx_id {
        return None;
    }
    let body = data.get(20..20 + msg_len)?;
    let mut offset = 0usize;
    let mut result = None;
    while offset + 4 <= body.len() {
        let attr_type = u16::from_be_bytes([body[offset], body[offset + 1]]);
        let attr_len = u16::from_be_bytes([body[offset + 2], body[offset + 3]]) as usize;
        let val_start = offset + 4;
        let val_end = val_start + attr_len;
        if val_end > body.len() {
            break;
        }
        let val = &body[val_start..val_end];
        let padded = (attr_len + 3) & !3;
        offset = val_start + padded;

        if attr_type == XOR_MAPPED_ADDRESS && val.len() >= 8 && val[1] == 0x01 {
            let xport = u16::from_be_bytes([val[2], val[3]]);
            let port = xport ^ ((MAGIC_COOKIE >> 16) as u16);
            let xaddr = u32::from_be_bytes([val[4], val[5], val[6], val[7]]);
            let addr = xaddr ^ MAGIC_COOKIE;
            let ip = std::net::Ipv4Addr::from(addr);
            result = Some(SocketAddr::from((ip, port)));
        } else if attr_type == MAPPED_ADDRESS && val.len() >= 8 && val[1] == 0x01 && result.is_none() {
            let port = u16::from_be_bytes([val[2], val[3]]);
            let ip = std::net::Ipv4Addr::new(val[4], val[5], val[6], val[7]);
            result = Some(SocketAddr::from((ip, port)));
        }
    }
    result
}

/// Best-effort discovery of this machine's own LAN-facing IPv4 address,
/// e.g. 192.168.1.20. Used to build a same-LAN fallback candidate address
/// for a peer (see `peer.rs`'s `lan_candidate`) -- when two peers share
/// the same public IP (behind the same router), the public path often
/// can't be used at all: it depends on the router supporting NAT
/// hairpin/loopback (sending a packet to your own WAN IP and having it
/// routed back inward), which many consumer routers simply don't do.
/// Reaching each other via LAN IPs instead sidesteps the router (and its
/// hairpin support or lack thereof) entirely.
///
/// This never actually sends any traffic: connecting a UDP socket only
/// asks the OS routing table which local address *would* be used to
/// reach `target`, without transmitting a single packet (UDP `connect()`
/// is purely a local kernel-side operation until you `send`). The address
/// 8.8.8.8:80 is used only as a plausible "public internet host" target
/// to force the OS to pick our real outbound-facing interface, the same
/// well-known technique `get_real_interface`'s callers rely on elsewhere
/// in this codebase -- no packet to 8.8.8.8 is ever sent, and no
/// connectivity to it is required for this to work.
pub fn discover_local_addr() -> Option<std::net::Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) => Some(v4),
        std::net::IpAddr::V6(_) => None,
    }
}

/// Binds a UDP socket to `local_port` and asks a STUN server what external
/// ip:port that mapping became. Returns None on any failure (DNS, timeout,
/// unparseable response).
pub fn discover_external_addr(local_port: u16, server_host: &str, server_port: u16) -> Option<SocketAddr> {
    let sock = UdpSocket::bind(("0.0.0.0", local_port)).ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok();

    let server_ip = std::net::ToSocketAddrs::to_socket_addrs(&(server_host, server_port))
        .ok()?
        .find(|a| a.is_ipv4())?;

    let mut tx_id = [0u8; 12];
    rand::Rng::fill(&mut rand::rng(), &mut tx_id);
    let req = build_request(&tx_id);
    sock.send_to(&req, server_ip).ok()?;

    let mut buf = [0u8; 512];
    let (n, _) = sock.recv_from(&mut buf).ok()?;
    parse_response(&buf[..n], &tx_id)
}

/// Generates a fresh random transaction ID and the raw request bytes to
/// send for it. Callers that need to reuse an already-bound/in-use socket
/// (e.g. the mesh's own listen-port socket, which must be the one that
/// actually gets STUN-probed so the discovered mapping is the one peers
/// need to use) send this themselves via `sock.send_to`, and separately
/// hand any inbound response bytes to `try_parse_response_for` -- see
/// mesh.rs's periodic re-discovery logic and its STUN-response hijack in
/// the main receive loop for why this can't just call `recv_from` itself
/// (that socket already has its own dedicated receive thread).
pub fn new_transaction() -> ([u8; 12], [u8; 20]) {
    let mut tx_id = [0u8; 12];
    rand::Rng::fill(&mut rand::rng(), &mut tx_id);
    let req = build_request(&tx_id);
    (tx_id, req)
}

/// Quick, cheap check for whether `data` even has the shape of a STUN
/// message (magic cookie in the right place) before bothering to try a
/// full parse. Used by the mesh's receive loop to decide, for each inbound
/// UDP datagram, whether it might be a STUN response it's waiting on
/// rather than encrypted mesh traffic -- real mesh packets (ciphertext)
/// will essentially never collide with this by chance.
pub fn looks_like_stun_message(data: &[u8]) -> bool {
    data.len() >= 8 && u32::from_be_bytes([data[4], data[5], data[6], data[7]]) == MAGIC_COOKIE
}

/// Parses `data` as a STUN binding response, only accepting it if its
/// transaction ID matches `tx_id`. Thin public wrapper around the internal
/// parser, for use by mesh.rs when it intercepts a candidate STUN response
/// from the shared listen-port socket.
pub fn try_parse_response_for(data: &[u8], tx_id: &[u8; 12]) -> Option<SocketAddr> {
    parse_response(data, tx_id)
}

/// Tries several well-known STUN servers and returns the first successful
/// result.
#[allow(dead_code)]
pub fn discover_external_addr_any(local_port: u16) -> Option<SocketAddr> {
    for (host, port) in DEFAULT_SERVERS {
        if let Some(addr) = discover_external_addr(local_port, host, *port) {
            return Some(addr);
        }
    }
    None
}
