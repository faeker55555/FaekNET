// Encodes/decodes a single-line, copy-pasteable "peer card" so adding a
// peer is: they run `export`, paste you the one line, you run
// `import <that line>`. The pre-shared key is intentionally NOT included
// here -- it's shared separately (via `init`/`genkey`), same as before,
// so this token is safe to paste into a group chat even though it reveals
// your public IP:port (which a peer would need anyway to connect to you).
use base64::Engine;

use crate::config::PeerConfig;

const PREFIX: &str = "LMESH1:";

pub fn encode(name: &str, virtual_ip: std::net::Ipv4Addr, public_ip: &str, public_port: u16) -> String {
    // '|' is used as the field separator; strip it out of free-text fields
    // defensively so decoding can't get confused.
    let safe_name = name.replace('|', "_");
    let raw = format!("1|{safe_name}|{virtual_ip}|{public_ip}|{public_port}");
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    format!("{PREFIX}{encoded}")
}

pub fn decode(token: &str) -> Result<PeerConfig, String> {
    let token = token.trim();
    let payload = token
        .strip_prefix(PREFIX)
        .ok_or_else(|| format!("Not a valid meow-meow peer card (expected it to start with '{PREFIX}')"))?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| format!("Could not decode peer card: {e}"))?;
    let raw = String::from_utf8(raw).map_err(|e| format!("Peer card is not valid text: {e}"))?;
    let fields: Vec<&str> = raw.split('|').collect();
    if fields.len() != 5 || fields[0] != "1" {
        return Err("Peer card is malformed or from an incompatible version".to_string());
    }
    let name = fields[1].to_string();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(|c| c.is_control()) {
        return Err("Peer card has an invalid name".to_string());
    }
    let virtual_ip: std::net::Ipv4Addr = fields[2]
        .parse()
        .map_err(|_| format!("Peer card has an invalid virtual IP: {}", fields[2]))?;
    // Only literal IPv4 addresses: a hostname here would make the client
    // resolve an attacker-chosen name and send mesh traffic to it.
    let public_ip: std::net::Ipv4Addr = fields[3]
        .parse()
        .map_err(|_| format!("Peer card has an invalid public IP: {}", fields[3]))?;
    let public_ip = public_ip.to_string();
    let public_port: u16 = fields[4]
        .parse()
        .map_err(|_| format!("Peer card has an invalid port: {}", fields[4]))?;
    if public_port == 0 {
        return Err("Peer card has an invalid port: 0".to_string());
    }

    Ok(PeerConfig {
        name,
        virtual_ip,
        public_ip,
        public_port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn roundtrip() {
        let token = encode("Ivan|weird", Ipv4Addr::new(10, 66, 0, 3), "203.0.113.5", 54321);
        let peer = decode(&token).unwrap();
        assert_eq!(peer.name, "Ivan_weird");
        assert_eq!(peer.virtual_ip, Ipv4Addr::new(10, 66, 0, 3));
        assert_eq!(peer.public_ip, "203.0.113.5");
        assert_eq!(peer.public_port, 54321);
    }

    #[test]
    fn rejects_hostnames_and_bad_fields() {
        let good = encode("bob", Ipv4Addr::new(10, 66, 0, 3), "203.0.113.5", 4000);
        assert!(decode(&good).is_ok());
        for raw in [
            "1|bob|10.66.0.3|evil.example|4000",
            "1|bob|10.66.0.3|203.0.113.5|0",
            "1|bob|10.66.0.3|999.1.1.1|4000",
            "1|bad\nname|10.66.0.3|203.0.113.5|4000",
            "1||10.66.0.3|203.0.113.5|4000",
        ] {
            let token = format!("{PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw));
            assert!(decode(&token).is_err(), "should reject {raw:?}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("not a token").is_err());
        assert!(decode("LMESH1:not-valid-base64!!!").is_err());
    }
}
