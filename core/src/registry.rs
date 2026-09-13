//! Public, read-only bootstrap directory. Network keys are never serialized here.
use crate::config::{MeConfig, PeerConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::process::Command;

pub const URL: &str = "https://raw.githubusercontent.com/faeker55555/FaekNET/main/network/peers.toml";
pub const ENDPOINT_PATH: &str = ".faeknet-endpoint.toml";
const MAX_BYTES: usize = 131072;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Directory {
    schema_version: u32,
    peers: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    virtual_ip: Ipv4Addr,
    public_ip: Ipv4Addr,
    public_port: u16,
    owner: String,
    updated_at: u64,
    expires_at: u64,
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_multicast()
        || a == 0 || a >= 240 || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c <= 2)
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

pub fn parse(raw: &str, now: u64) -> Result<Vec<PeerConfig>, String> {
    if raw.len() > MAX_BYTES { return Err("Peer directory exceeds size limit".into()); }
    let directory: Directory = toml::from_str(raw).map_err(|e| format!("Invalid peer directory: {e}"))?;
    if directory.schema_version != 1 || directory.peers.len() > 254 {
        return Err("Unsupported or oversized peer directory".into());
    }
    let mut seen = HashSet::new();
    let mut peers = Vec::new();
    for entry in directory.peers {
        let octets = entry.virtual_ip.octets();
        if octets[..3] != [10, 66, 0] || octets[3] == 0 || octets[3] == 255
            || !seen.insert(entry.virtual_ip) || !public_ipv4(entry.public_ip)
            || entry.public_port == 0 || entry.owner.is_empty()
            || entry.name.is_empty() || entry.name.len() > 32
            || !entry.name.as_bytes()[0].is_ascii_alphanumeric()
            || !entry.name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || entry.updated_at > now.saturating_add(60)
            || entry.expires_at <= entry.updated_at
            || entry.expires_at - entry.updated_at > 7 * 86400
        {
            return Err("Invalid or duplicate peer directory entry".into());
        }
        if entry.expires_at <= now { continue; }
        peers.push(PeerConfig {
            name: entry.name,
            virtual_ip: entry.virtual_ip,
            public_ip: entry.public_ip.to_string(),
            public_port: entry.public_port,
        });
    }
    Ok(peers)
}

/// Called only from a worker thread. curl is also used by the GUI updater.
/// Fixed HTTPS origin, no redirects, no user shell, no credentials or curlrc.
pub fn fetch(now: u64) -> Result<Vec<PeerConfig>, String> {
    let output = Command::new("curl")
        .args(["-q", "--fail", "--silent", "--show-error", "--proto", "=https",
            "--connect-timeout", "5", "--max-time", "15", "--max-filesize", "131072", URL])
        .output().map_err(|e| format!("Could not run curl for peer discovery: {e}"))?;
    if !output.status.success() { return Err("Could not download the GitHub peer directory".into()); }
    let raw = std::str::from_utf8(&output.stdout).map_err(|_| "Directory is not UTF-8")?;
    parse(raw, now)
}

#[derive(Serialize)]
struct PublicEndpoint<'a> {
    schema_version: u32,
    name: &'a str,
    virtual_ip: Ipv4Addr,
    public_ip: Ipv4Addr,
    public_port: u16,
    observed_at: u64,
}

/// Local handoff to the non-elevated GitHub publisher. Explicit field allowlist:
/// never serialize MeConfig here, since it contains the private network key.
pub fn write_endpoint(me: &MeConfig, addr: SocketAddr, now: u64) -> Result<(), String> {
    let std::net::IpAddr::V4(ip) = addr.ip() else { return Err("IPv4 required".into()); };
    let endpoint = PublicEndpoint {
        schema_version: 1, name: &me.name, virtual_ip: me.virtual_ip,
        public_ip: ip, public_port: addr.port(), observed_at: now,
    };
    let text = toml::to_string(&endpoint).map_err(|e| e.to_string())?;
    // A partial read during replacement is harmless: the publisher rejects it
    // and waits for its next poll. This also works on Windows without rename-overwrite.
    std::fs::write(ENDPOINT_PATH, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry() -> String {
        "schema_version = 1\n[[peers]]\nname = 'alice'\nvirtual_ip = '10.66.0.2'\npublic_ip = '8.8.8.8'\npublic_port = 54321\nowner = 'alice'\nupdated_at = 100\nexpires_at = 200\n".into()
    }
    #[test]
    fn validates_and_expires_entries() {
        assert_eq!(parse(&entry(), 150).unwrap().len(), 1);
        assert!(parse(&entry(), 200).unwrap().is_empty());
        assert!(parse("schema_version = 1\npeers = []", 150).unwrap().is_empty());
    }
    #[test]
    fn rejects_private_endpoints_unknown_fields_and_duplicates() {
        for ip in ["127.0.0.1", "192.168.1.2", "100.64.0.1", "0.0.0.0", "224.0.0.1", "203.0.113.1"] {
            assert!(parse(&entry().replace("8.8.8.8", ip), 150).is_err());
        }
        assert!(parse(&(entry() + "psk = 'must-not-be-here'\n"), 150).is_err());
        assert!(parse(&(entry() + "[[peers]]" + entry().split_once("[[peers]]").unwrap().1), 150).is_err());
        assert!(parse(&"x".repeat(MAX_BYTES + 1), 150).is_err());
        assert!(parse(&entry().replace("public_port = 54321", "public_port = 0"), 150).is_err());
    }
}
