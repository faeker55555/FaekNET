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
    /// DNS-style suffix used for local mesh domain names, e.g. with the
    /// default "mesh" a peer named "alice" becomes reachable as
    /// "alice.mesh" once hosts-file sync (and/or the built-in DNS server)
    /// is enabled. Purely cosmetic/organizational -- has no effect on
    /// routing, which is always done by virtual IP under the hood.
    #[serde(default = "default_domain_suffix")]
    pub domain_suffix: String,
    /// Whether to automatically keep a managed block in the OS hosts file
    /// (/etc/hosts or Windows' System32\drivers\etc\hosts) up to date with
    /// "<peer>.<suffix> -> virtual ip" entries for every known peer plus
    /// ourselves. On by default: it needs no new privileges beyond what
    /// the virtual adapter already requires (root/Administrator), and
    /// works with literally every application unconditionally, unlike DNS
    /// integration which some OS network stacks fight to override.
    #[serde(default = "default_true")]
    pub sync_hosts_file: bool,
    /// Whether to run the built-in local DNS resolver (binds a UDP socket,
    /// answers "*.<suffix>" queries with the right virtual IP, and
    /// forwards everything else upstream). Off by default -- it's a
    /// heavier, more failure-prone mechanism than hosts-file sync (see
    /// dns.rs), so it's opt-in for people who specifically want subdomain
    /// support or don't want to rely on the hosts file.
    #[serde(default)]
    pub dns_server: bool,
    /// UDP port the built-in DNS server listens on, on 127.0.0.1. Defaults
    /// to 53 (the standard DNS port) so the OS/network adapter can be
    /// pointed at 127.0.0.1 as a normal resolver; changeable in case
    /// something else on the machine already owns port 53.
    #[serde(default = "default_dns_port")]
    pub dns_port: u16,
    /// Whether to also attempt to point this machine's own DNS resolution
    /// at the built-in server automatically (best-effort; see dns.rs).
    /// Only has any effect if `dns_server` is also enabled.
    #[serde(default)]
    pub dns_auto_configure: bool,
}

/// One named service this machine advertises to the mesh: a friendly
/// subdomain label plus the port it's reachable on (always at this
/// machine's own virtual IP -- services don't have their own address,
/// they're just a memorable name for "something on port N here").
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ServiceConfig {
    pub name: String,
    pub port: u16,
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

fn default_domain_suffix() -> String {
    "mesh".to_string()
}

fn default_true() -> bool {
    true
}

fn default_dns_port() -> u16 {
    53
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
    /// Named services this machine hosts on the mesh, e.g. a game server
    /// or a web dashboard -- each becomes its own subdomain
    /// (`<service>.<my name>.<suffix>`, e.g. `game.alice.mesh`) that's
    /// gossiped to the whole mesh the same way peer addresses are, so
    /// every member eventually learns about it without needing to ask.
    /// Lives at the top level of the config (a `[[services]]` table, same
    /// shape as `[[peers]]`) rather than nested under `[me]`, since TOML
    /// requires the fully-qualified `[[me.services]]` for a nested array
    /// of tables -- putting it at the top level avoids that easy-to-miss
    /// footgun when hand-editing mesh.toml. See `hosts.rs`/`dns.rs` for
    /// how these become actual resolvable names, and `gossip.rs` for the
    /// wire format.
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
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
