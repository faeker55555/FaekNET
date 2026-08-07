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
    let mut out = Vec::with_capacity(1 + entries.len() * (15 + MAX_NAME_LEN));
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
        entries.push(GossipEntry {
            virtual_ip,
            name,
            addr: SocketAddr::from((ip, port)),
            epoch_secs,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_entry() {
        let entries = vec![GossipEntry {
            virtual_ip: Ipv4Addr::new(10, 66, 0, 3),
            name: "carol".to_string(),
            addr: "203.0.113.9:54321".parse().unwrap(),
            epoch_secs: 1_700_000_000,
        }];
        let payloads = build_payloads(&entries);
        assert_eq!(payloads.len(), 1);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed, entries);
    }

    #[test]
    fn roundtrip_many_entries_splits_packets() {
        let entries: Vec<GossipEntry> = (0..40)
            .map(|i| GossipEntry {
                virtual_ip: Ipv4Addr::new(10, 66, 0, i as u8),
                name: format!("peer{i}"),
                addr: SocketAddr::from((Ipv4Addr::new(203, 0, 113, i as u8), 40000 + i as u16)),
                epoch_secs: 1_700_000_000 + i as u32,
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
        let entries = vec![GossipEntry {
            virtual_ip: Ipv4Addr::new(10, 66, 0, 9),
            name: "a".repeat(100),
            addr: "203.0.113.9:1234".parse().unwrap(),
            epoch_secs: 42,
        }];
        let payloads = build_payloads(&entries);
        let parsed = parse_payload(&payloads[0]);
        assert_eq!(parsed[0].name.len(), MAX_NAME_LEN);
    }
}
