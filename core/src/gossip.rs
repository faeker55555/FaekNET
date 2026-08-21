// Peer-table gossip: lets the mesh behave like a self-propagating "subnet"
// instead of a static, manually-maintained peer list. Every connected node
// periodically shares everything it knows about the mesh (name, virtual
// IP, best-known public address, and how fresh that address is) with
// every peer it's currently talking to. When a node hears about a peer it
// didn't already know, it starts hole-punching to it directly -- no
// manual export/import needed for that pair. This is what turns "A knows
// C, C knows B" into "A eventually knows B too" automatically.
//
// Only ONE initial address exchange (export/import) is still needed to
// bootstrap a brand-new node into an existing mesh -- there is no way
// around that without a rendezvous/relay server, which is explicitly out
// of scope for this project. Gossip is what eliminates every exchange
// *after* that first one.
use std::net::{Ipv4Addr, SocketAddr};

/// Cap on how many entries go in a single gossip packet, to keep packets
/// comfortably under typical MTUs even with longer names. If the mesh ever
/// grows past this, multiple gossip packets are sent back-to-back on the
/// same tick rather than truncating the peer table.
pub const MAX_ENTRIES_PER_PACKET: usize = 16;
const MAX_NAME_LEN: usize = 32;
/// Service names are shorter than display names -- these are meant to be
/// short labels like "game"/"voice"/"web", not free-form text.
const MAX_SERVICE_NAME_LEN: usize = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct GossipEntry {
    pub virtual_ip: Ipv4Addr,
    pub name: String,
    pub addr: SocketAddr,
    /// Unix epoch seconds (truncated to u32) of when the sender last
    /// confirmed this address was current. Used for freshest-wins conflict
    /// resolution when merging gossip from multiple sources. This assumes
    /// clocks across peers are roughly in sync (same assumption TLS
    /// certificate validation makes); wildly-off clocks would only affect
    /// which address is preferred, never correctness of the mesh itself.
    pub epoch_secs: u32,
    /// Empty for an ordinary "here's a peer and their address" entry.
    /// Non-empty means this entry instead announces one named *service*
    /// hosted by the peer at `virtual_ip` (e.g. "game", "voice", "web"),
    /// reachable on `service_port` -- this is what powers named mesh
    /// subdomains like `game.alice.mesh`, gossiped the same way peer
    /// addresses are so everyone in the mesh eventually learns about
    /// every declared service without needing to ask each peer directly.
    pub service_name: String,
    /// Only meaningful when `service_name` is non-empty.
    pub service_port: u16,
    /// The sender's own best-guess LAN-facing address (e.g.
    /// 192.168.1.20:54321), gossiped alongside their public address so a
    /// peer that turns out to share the same router/public IP has a
    /// same-LAN fallback to try -- see `peer.rs`'s `lan_candidate` for how
    /// it's used. `None` when the sender couldn't determine a local
    /// address, or (for service entries) simply not applicable.
    pub lan_addr: Option<SocketAddr>,
}

impl GossipEntry {
    /// Convenience constructor for an ordinary peer-address entry (no
    /// service attached) -- the overwhelmingly common case both in
    /// production call sites and tests.
    pub fn peer(virtual_ip: Ipv4Addr, name: impl Into<String>, addr: SocketAddr, epoch_secs: u32) -> GossipEntry {
        GossipEntry {
            virtual_ip,
            name: name.into(),
            addr,
            epoch_secs,
            service_name: String::new(),
            service_port: 0,
            lan_addr: None,
        }
    }

    /// Same as `peer`, but also carries the sender's own LAN-facing
    /// candidate address (see `lan_addr`'s doc comment).
    pub fn peer_with_lan(
        virtual_ip: Ipv4Addr,
        name: impl Into<String>,
        addr: SocketAddr,
        epoch_secs: u32,
        lan_addr: Option<SocketAddr>,
    ) -> GossipEntry {
        GossipEntry {
            lan_addr,
            ..GossipEntry::peer(virtual_ip, name, addr, epoch_secs)
        }
    }

    /// Convenience constructor for a service-announcement entry: `name`
    /// here is still the peer's own display name (services are always
    /// gossiped alongside who hosts them), with `service`/`port`
    /// describing the service itself.
    pub fn service(
        virtual_ip: Ipv4Addr,
        name: impl Into<String>,
        addr: SocketAddr,
        epoch_secs: u32,
        service: impl Into<String>,
        port: u16,
    ) -> GossipEntry {
        GossipEntry {
            virtual_ip,
            name: name.into(),
            addr,
            epoch_secs,
            service_name: service.into(),
            service_port: port,
            lan_addr: None,
        }
    }

    pub fn is_service(&self) -> bool {
        !self.service_name.is_empty()
    }
}

/// Splits `entries` into one or more wire-ready payloads, each containing
/// at most MAX_ENTRIES_PER_PACKET entries and only IPv4 addresses (this
/// mesh is IPv4-only end to end, consistent with proto::ipv4_dst).
pub fn build_payloads(entries: &[GossipEntry]) -> Vec<Vec<u8>> {
    entries
        .chunks(MAX_ENTRIES_PER_PACKET)
        .map(build_single_payload)
        .collect()
}

fn build_single_payload(entries: &[GossipEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + entries.len() * (18 + MAX_NAME_LEN + MAX_SERVICE_NAME_LEN));
    out.push(entries.len().min(255) as u8);
    for entry in entries.iter().take(255) {
        let SocketAddr::V4(v4) = entry.addr else {
            continue; // IPv6 peer addresses aren't supported; skip.
        };
        out.extend_from_slice(&entry.virtual_ip.octets());
        let name_bytes = entry.name.as_bytes();
        let name_len = name_bytes.len().min(MAX_NAME_LEN);
        out.push(name_len as u8);
        out.extend_from_slice(&name_bytes[..name_len]);
        out.extend_from_slice(&v4.ip().octets());
        out.extend_from_slice(&v4.port().to_be_bytes());
        out.extend_from_slice(&entry.epoch_secs.to_be_bytes());
        let service_bytes = entry.service_name.as_bytes();
        let service_len = service_bytes.len().min(MAX_SERVICE_NAME_LEN);
        out.push(service_len as u8);
        out.extend_from_slice(&service_bytes[..service_len]);
        out.extend_from_slice(&entry.service_port.to_be_bytes());
        // Optional LAN candidate: a 1-byte presence flag followed by 6
        // bytes (IPv4 + port) if present, 0 bytes otherwise. Appended
        // after every other field (including the pre-existing service
        // fields) so older builds that don't know about it simply stop
        // parsing at the byte they already understood -- see
        // `parse_payload`'s tolerant trailing-field handling below, the
        // same pattern already used to add the service fields.
        match entry.lan_addr {
            Some(SocketAddr::V4(lan)) => {
                out.push(1);
                out.extend_from_slice(&lan.ip().octets());
                out.extend_from_slice(&lan.port().to_be_bytes());
            }
            _ => out.push(0),
        }
    }
    out
}

/// Parses a gossip payload back into entries. Malformed trailing bytes are
/// ignored rather than rejecting the whole packet, since gossip is
/// best-effort/self-healing by nature -- a partially-useful packet is
/// still useful.
pub fn parse_payload(data: &[u8]) -> Vec<GossipEntry> {
    let mut entries = Vec::new();
    if data.is_empty() {
        return entries;
    }
    let count = data[0] as usize;
    let mut offset = 1usize;
    for _ in 0..count {
        // 4 (vip) + 1 (name_len) at minimum before we know the real length
        if offset + 5 > data.len() {
            break;
        }
        let virtual_ip = Ipv4Addr::new(data[offset], data[offset + 1], data[offset + 2], data[offset + 3]);
        let name_len = data[offset + 4] as usize;
        offset += 5;
        if offset + name_len + 4 + 2 + 4 > data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
        offset += name_len;
        let ip = Ipv4Addr::new(data[offset], data[offset + 1], data[offset + 2], data[offset + 3]);
        offset += 4;
        let port = u16::from_be_bytes([data[offset], data[offset + 1]]);
        offset += 2;
        let epoch_secs = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
        offset += 4;

        // Service fields: 1-byte length + up to MAX_SERVICE_NAME_LEN bytes
        // + 2-byte port. Tolerated as absent (older-format packet) rather
        // than aborting the whole entry, so a mixed-version mesh still
        // gets ordinary peer-address gossip working even if service
        // announcements don't round-trip until everyone's updated.
        let (service_name, service_port) = if offset + 1 <= data.len() {
            let service_len = data[offset] as usize;
            offset += 1;
            if offset + service_len + 2 <= data.len() {
                let sname = String::from_utf8_lossy(&data[offset..offset + service_len]).to_string();
                offset += service_len;
                let sport = u16::from_be_bytes([data[offset], data[offset + 1]]);
                offset += 2;
                (sname, sport)
            } else {
                (String::new(), 0)
            }
        } else {
            (String::new(), 0)
        };

        // Optional LAN candidate, appended after the service fields (see
        // `build_single_payload`). Tolerated as absent for the same
        // reason the service fields are: an older-format packet, or one
        // truncated for any other reason, still yields a fully usable
        // entry minus this one optional field.
        let lan_addr = if offset + 1 <= data.len() {
            let present = data[offset] != 0;
            offset += 1;
            if present && offset + 6 <= data.len() {
                let lan_ip = Ipv4Addr::new(data[offset], data[offset + 1], data[offset + 2], data[offset + 3]);
                let lan_port = u16::from_be_bytes([data[offset + 4], data[offset + 5]]);
                offset += 6;
                Some(SocketAddr::from((lan_ip, lan_port)))
            } else {
                None
            }
        } else {
            None
        };

        entries.push(GossipEntry {
            virtual_ip,
            name,
            addr: SocketAddr::from((ip, port)),
            epoch_secs,
            service_name,
            service_port,
            lan_addr,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_entry() {
        let entries = vec![GossipEntry::peer(
            Ipv4Addr::new(10, 66, 0, 3),
            "carol",
            "203.0.113.9:54321".parse().unwrap(),
            1_700_000_000,
        )];
        let payloads = build_payloads(&entries);
        assert_eq!(payloads.len(), 1);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed, entries);
    }

    #[test]
    fn roundtrip_many_entries_splits_packets() {
        let entries: Vec<GossipEntry> = (0..40)
            .map(|i| {
                GossipEntry::peer(
                    Ipv4Addr::new(10, 66, 0, i as u8),
                    format!("peer{i}"),
                    SocketAddr::from((Ipv4Addr::new(203, 0, 113, i as u8), 40000 + i as u16)),
                    1_700_000_000 + i as u32,
                )
            })
            .collect();
        let payloads = build_payloads(&entries);
        assert_eq!(payloads.len(), 3); // 40 entries / 16 per packet -> 3 packets
        let mut all_parsed = Vec::new();
        for p in &payloads {
            all_parsed.extend(parse_payload(p));
        }
        assert_eq!(all_parsed, entries);
    }

    #[test]
    fn handles_empty_and_garbage_gracefully() {
        assert!(parse_payload(&[]).is_empty());
        assert!(parse_payload(&[5]).is_empty()); // claims 5 entries, has none
        assert!(parse_payload(&[1, 1, 2, 3]).is_empty()); // truncated entry
    }

    #[test]
    fn long_name_is_truncated_not_rejected() {
        let entries = vec![GossipEntry::peer(
            Ipv4Addr::new(10, 66, 0, 9),
            "a".repeat(100),
            "203.0.113.9:1234".parse().unwrap(),
            42,
        )];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed[0].name.len(), MAX_NAME_LEN);
    }

    #[test]
    fn roundtrip_service_entry() {
        let entries = vec![GossipEntry::service(
            Ipv4Addr::new(10, 66, 0, 3),
            "carol",
            "203.0.113.9:54321".parse().unwrap(),
            1_700_000_000,
            "game",
            25565,
        )];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed, entries);
        assert!(parsed[0].is_service());
        assert_eq!(parsed[0].service_name, "game");
        assert_eq!(parsed[0].service_port, 25565);
    }

    #[test]
    fn mixed_peer_and_service_entries_roundtrip() {
        let entries = vec![
            GossipEntry::peer(Ipv4Addr::new(10, 66, 0, 3), "carol", "203.0.113.9:54321".parse().unwrap(), 100),
            GossipEntry::service(
                Ipv4Addr::new(10, 66, 0, 3),
                "carol",
                "203.0.113.9:54321".parse().unwrap(),
                100,
                "voice",
                7777,
            ),
        ];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed, entries);
    }

    #[test]
    fn roundtrip_with_lan_candidate() {
        let entries = vec![GossipEntry::peer_with_lan(
            Ipv4Addr::new(10, 66, 0, 10),
            "server",
            "203.0.113.7:1024".parse().unwrap(),
            1_700_000_000,
            Some("192.168.1.20:54321".parse().unwrap()),
        )];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed, entries);
        assert_eq!(parsed[0].lan_addr, Some("192.168.1.20:54321".parse().unwrap()));
    }

    #[test]
    fn no_lan_candidate_roundtrips_as_none() {
        let entries = vec![GossipEntry::peer(
            Ipv4Addr::new(10, 66, 0, 3),
            "carol",
            "203.0.113.9:54321".parse().unwrap(),
            100,
        )];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed[0].lan_addr, None);
    }

    #[test]
    fn truncated_packet_missing_lan_flag_byte_still_parses_other_fields() {
        // Simulates a packet from an OLDER build that doesn't know about
        // the LAN-candidate field at all (i.e. it's simply absent from
        // the wire, not zero-length) -- everything up through the
        // service fields must still parse correctly, with lan_addr
        // defaulting to None rather than the whole entry being dropped.
        let entries = vec![GossipEntry::peer(
            Ipv4Addr::new(10, 66, 0, 3),
            "carol",
            "203.0.113.9:54321".parse().unwrap(),
            100,
        )];
        let mut payload = build_single_payload(&entries);
        // Strip the trailing "no LAN candidate" flag byte we just wrote,
        // reproducing exactly what an old-format packet looks like.
        payload.pop();
        let parsed = parse_payload(&payload);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].virtual_ip, Ipv4Addr::new(10, 66, 0, 3));
        assert_eq!(parsed[0].lan_addr, None);
    }

    #[test]
    fn long_service_name_is_truncated_not_rejected() {
        let entries = vec![GossipEntry::service(
            Ipv4Addr::new(10, 66, 0, 9),
            "bob",
            "203.0.113.9:1234".parse().unwrap(),
            42,
            "s".repeat(100),
            80,
        )];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed[0].service_name.len(), MAX_SERVICE_NAME_LEN);
    }
}
