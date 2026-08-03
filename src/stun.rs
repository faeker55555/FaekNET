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

pub const DEFAULT_SERVERS: &[(&str, u16)] = &[
    ("stun.l.google.com", 19302),
    ("stun1.l.google.com", 19302),
    ("stun.cloudflare.com", 3478),
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
