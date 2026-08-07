use serde::{Deserialize, Serialize};
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

pub const CONFIG_PATH: &str = "mesh.toml";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MeConfig {
    /// Display name shown to peers (used when generating an `export` peer
    /// card). Purely cosmetic.
    #[serde(default = "default_name")]
    pub name: String,
    /// This machine's address on the virtual LAN, e.g. "10.66.0.2"
    pub virtual_ip: Ipv4Addr,
    /// Prefix length of the virtual LAN subnet (all peers must share this), e.g. 24
    #[serde(default = "default_prefix")]
    pub prefix: u8,
    /// UDP port this instance listens on
    pub listen_port: u16,
    /// Base64-encoded 32-byte pre-shared key shared by the whole mesh.
    /// Anyone with this key can join the virtual LAN, so treat it like a
    /// Wi-Fi password: share it only with people you trust, over a
    /// separate channel (chat app, voice call, etc.), never publicly.
    pub psk: String,
    /// MTU for the virtual adapter. Kept below 1500 to leave room for the
    /// UDP/IP + encryption overhead this tool adds around every packet.
    #[serde(default = "default_mtu")]
    pub mtu: u16,
}

fn default_name() -> String {
    "peer".to_string()
}

fn default_prefix() -> u8 {
    24
}

fn default_mtu() -> u16 {
    1400
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PeerConfig {
    pub name: String,
    pub virtual_ip: Ipv4Addr,
    pub public_ip: String,
    pub public_port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    pub me: MeConfig,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
}

impl Config {
    pub fn load() -> std::io::Result<Config> {
        let content = fs::read_to_string(CONFIG_PATH)?;
        let cfg: Config = toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cfg)
    }

    pub fn save(&self) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(CONFIG_PATH, s)
    }

    pub fn exists() -> bool {
        Path::new(CONFIG_PATH).exists()
    }

    /// Broadcast address for the virtual LAN, derived from this node's
    /// virtual IP and subnet prefix (e.g. 10.66.0.2/24 -> 10.66.0.255).
    pub fn broadcast_addr(&self) -> Ipv4Addr {
        let ip_u32 = u32::from(self.me.virtual_ip);
        let mask: u32 = if self.me.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.me.prefix as u32)
        };
        let broadcast_u32 = (ip_u32 & mask) | !mask;
        Ipv4Addr::from(broadcast_u32)
    }
}
