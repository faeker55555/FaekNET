use std::net::Ipv4Addr;

pub const TYPE_DATA: u8 = 1;
/// No longer sent by this version (TYPE_PING now doubles as the keepalive,
/// since it additionally yields an RTT measurement) but kept as a reserved
/// value, and still handled harmlessly on receipt for compatibility with
/// older builds.
#[allow(dead_code)]
pub const TYPE_KEEPALIVE: u8 = 2;
/// Latency probe: payload carries an 8-byte big-endian nonce/sequence id
/// that the receiver echoes back unchanged in a TYPE_PONG. Used by the
/// `ping` CLI command; unrelated to keepalives (which carry no payload and
/// aren't echoed).
pub const TYPE_PING: u8 = 3;
pub const TYPE_PONG: u8 = 4;
/// Carries a gossip::build_payloads() chunk describing peers the sender
/// knows about (name, virtual IP, best-known address, freshness). See
/// gossip.rs for the wire format of the payload itself.
pub const TYPE_GOSSIP: u8 = 5;

/// Plaintext wire format (this is what gets AEAD-encrypted as a whole):
///   byte 0       : packet type (TYPE_DATA / TYPE_KEEPALIVE)
///   bytes 1..5   : sender's virtual IPv4 address (network byte order)
///   bytes 5..    : payload (raw IPv4 packet bytes for TYPE_DATA, empty for
///                  TYPE_KEEPALIVE)
///
/// Embedding the sender's virtual IP lets the receiver identify *which*
/// mesh peer a packet came from independent of the UDP source address it
/// physically arrived from -- this is what allows learning/roaming to a
/// peer's real (possibly NAT-remapped) address the first time we hear from
/// them, exactly like the STUN-discovered-port situation we ran into with
/// the chat program.
pub struct Header {
    pub packet_type: u8,
    pub sender_virtual_ip: Ipv4Addr,
}

pub const HEADER_LEN: usize = 5;

pub fn build(packet_type: u8, sender_virtual_ip: Ipv4Addr, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.push(packet_type);
    out.extend_from_slice(&sender_virtual_ip.octets());
    out.extend_from_slice(payload);
    out
}

pub fn parse(data: &[u8]) -> Option<(Header, &[u8])> {
    if data.len() < HEADER_LEN {
        return None;
    }
    let packet_type = data[0];
    let sender_virtual_ip = Ipv4Addr::new(data[1], data[2], data[3], data[4]);
    Some((
        Header {
            packet_type,
            sender_virtual_ip,
        },
        &data[HEADER_LEN..],
    ))
}

/// Extracts the destination IPv4 address from a raw IP packet as read from
/// the TUN device. Returns None if the packet isn't a plausible IPv4
/// packet (e.g. too short, or IPv6/other -- this mesh only routes IPv4
/// traffic on the virtual LAN).
pub fn ipv4_dst(packet: &[u8]) -> Option<Ipv4Addr> {
    if packet.len() < 20 {
        return None;
    }
    let version = packet[0] >> 4;
    if version != 4 {
        return None;
    }
    Some(Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]))
}

#[allow(dead_code)]
pub fn ipv4_src(packet: &[u8]) -> Option<Ipv4Addr> {
    if packet.len() < 20 {
        return None;
    }
    let version = packet[0] >> 4;
    if version != 4 {
        return None;
    }
    Some(Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]))
}

/// True if `addr` is a destination that should be flooded to every peer
/// rather than routed to one: the subnet broadcast address, the universal
/// limited-broadcast 255.255.255.255, or any IPv4 multicast address
/// (224.0.0.0/4). LAN game discovery traffic (and DHCP-like probes) relies
/// heavily on these.
pub fn is_flood_target(addr: Ipv4Addr, subnet_broadcast: Ipv4Addr) -> bool {
    if addr == subnet_broadcast || addr == Ipv4Addr::BROADCAST {
        return true;
    }
    let octets = addr.octets();
    (224..=239).contains(&octets[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let ip = Ipv4Addr::new(10, 66, 0, 5);
        let payload = b"some ip packet bytes";
        let wire = build(TYPE_DATA, ip, payload);
        let (hdr, rest) = parse(&wire).unwrap();
        assert_eq!(hdr.packet_type, TYPE_DATA);
        assert_eq!(hdr.sender_virtual_ip, ip);
        assert_eq!(rest, payload);
    }

    #[test]
    fn detects_flood_targets() {
        let bc = Ipv4Addr::new(10, 66, 0, 255);
        assert!(is_flood_target(bc, bc));
        assert!(is_flood_target(Ipv4Addr::new(255, 255, 255, 255), bc));
        assert!(is_flood_target(Ipv4Addr::new(239, 255, 255, 250), bc)); // SSDP multicast
        assert!(!is_flood_target(Ipv4Addr::new(10, 66, 0, 5), bc));
    }
}
