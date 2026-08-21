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

    /// Manual override for our own public ip:port, bypassing self-STUN
    /// discovery entirely when set. Useful when STUN is blocked/
    /// unreliable on a given network (corporate firewall, some
    /// antivirus/security suites, or self-STUN simply never resolving
    /// for reasons that resist diagnosis) -- get the IP from any "what's
    /// my IP" site and the port from your router's port-forwarding page
    /// (forward the UDP `listen_port` you configured to this machine) if
    /// you're behind a NAT you control, or leave both unset to keep using
    /// automatic STUN discovery. Both fields must be set together to take
    /// effect; if only one is present it's ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_public_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_public_port: Option<u16>,

    /// Whether to pin the mesh's UDP socket to the OS's real internet-
    /// facing network interface, explicitly skipping VPN/tunnel-style
    /// virtual adapters (Cloudflare WARP, WireGuard, etc.) and the mesh's
    /// own virtual TUN adapter. On by default -- this is what makes
    /// self-STUN discovery work correctly when an always-on VPN/WARP
    /// client is active. Turn this OFF as a diagnostic/workaround if
    /// self-STUN still won't resolve even without any VPN active -- on
    /// some Windows machines the interface enumeration/pinning itself can
    /// interact badly with a particular network setup (security
    /// software, unusual adapter configurations, etc.) for reasons this
    /// project has no way to reproduce or diagnose remotely; disabling it
    /// reverts to letting the OS pick the outbound route normally, the
    /// same as every other UDP application on the machine.
    #[serde(default = "default_true")]
    pub warp_compat: bool,

    /// Whether to persist our own discovered (or manually entered) public
    /// ip:port to mesh.toml (`cached_public_ip`/`cached_public_port`
    /// below), so future launches have an immediately usable value
    /// without waiting on self-STUN to succeed first. Off by default.
    /// When enabled, the cached value is adopted immediately at startup
    /// if present, while self-STUN continues running in the background as
    /// normal to keep it current -- and if self-STUN can never succeed at
    /// all on this network, the cached value keeps being used
    /// indefinitely instead of the mesh being stuck advertising no
    /// address at all (and re-logging a failure warning forever).
    #[serde(default)]
    pub cache_public_addr: bool,
    /// Last successfully discovered (or manually entered) public ip:port,
    /// persisted here when `cache_public_addr` is enabled. Not meant to
    /// be hand-edited -- use `manual_public_ip`/`manual_public_port`
    /// instead if you want to force a specific value; use the "reset
    /// public address" action (CLI: `meow-meow reset-public-addr`, GUI:
    /// Settings screen) to clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_public_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_public_port: Option<u16>,
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

    /// Resolves the manual public-address override, if both its fields
    /// are present and the IP half actually parses. Returns None if
    /// either field is unset (self-STUN discovery should be used
    /// instead) or the stored IP string is invalid (defensively -- the
    /// GUI/CLI both validate on entry, but a hand-edited mesh.toml could
    /// still contain garbage).
    pub fn manual_public_addr(&self) -> Option<std::net::SocketAddr> {
        let ip = self.me.manual_public_ip.as_ref()?;
        let port = self.me.manual_public_port?;
        let ip: Ipv4Addr = ip.trim().parse().ok()?;
        Some(std::net::SocketAddr::from((ip, port)))
    }

    /// Resolves the cached public-address fallback, if present and
    /// `cache_public_addr` is enabled. See `MeConfig::cache_public_addr`
    /// doc comment for the full rationale.
    pub fn cached_public_addr(&self) -> Option<std::net::SocketAddr> {
        if !self.me.cache_public_addr {
            return None;
        }
        let ip = self.me.cached_public_ip.as_ref()?;
        let port = self.me.cached_public_port?;
        let ip: Ipv4Addr = ip.trim().parse().ok()?;
        Some(std::net::SocketAddr::from((ip, port)))
    }

    /// Persists the given address as the cached public address, best
    /// effort (failures are for the caller to log, not fatal). No-op if
    /// `cache_public_addr` is disabled, so a stale cached value left over
    /// from when caching was previously enabled doesn't linger
    /// meaninglessly once it's turned back off... except it deliberately
    /// does NOT clear the old cached value either, so re-enabling caching
    /// later still has something to fall back on immediately rather than
    /// starting from nothing. Use `clear_cached_public_addr` to actually
    /// wipe it.
    pub fn set_cached_public_addr(&mut self, addr: std::net::SocketAddr) {
        if !self.me.cache_public_addr {
            return;
        }
        self.me.cached_public_ip = Some(addr.ip().to_string());
        self.me.cached_public_port = Some(addr.port());
    }

    /// Clears any cached public address (used by the "reset public
    /// address" action). Does not touch `manual_public_ip`/
    /// `manual_public_port` -- those are a separate, explicit override
    /// the user has to clear themselves if they want to go back to
    /// automatic discovery.
    pub fn clear_cached_public_addr(&mut self) {
        self.me.cached_public_ip = None;
        self.me.cached_public_port = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            me: MeConfig {
                name: "me".to_string(),
                virtual_ip: "10.66.0.1".parse().unwrap(),
                prefix: 24,
                listen_port: 12345,
                psk: String::new(),
                mtu: 1400,
                domain_suffix: "mesh".to_string(),
                sync_hosts_file: true,
                dns_server: false,
                dns_port: 53,
                dns_auto_configure: false,
                manual_public_ip: None,
                manual_public_port: None,
                warp_compat: true,
                cache_public_addr: false,
                cached_public_ip: None,
                cached_public_port: None,
            },
            peers: Vec::new(),
            services: Vec::new(),
        }
    }

    #[test]
    fn manual_public_addr_is_none_when_unset() {
        let cfg = base_config();
        assert_eq!(cfg.manual_public_addr(), None);
    }

    #[test]
    fn manual_public_addr_requires_both_fields() {
        let mut cfg = base_config();
        cfg.me.manual_public_ip = Some("203.0.113.5".to_string());
        // port still missing -- should not resolve to anything.
        assert_eq!(cfg.manual_public_addr(), None);

        cfg.me.manual_public_port = Some(4000);
        assert_eq!(cfg.manual_public_addr(), Some("203.0.113.5:4000".parse().unwrap()));
    }

    #[test]
    fn manual_public_addr_rejects_garbage_ip() {
        let mut cfg = base_config();
        cfg.me.manual_public_ip = Some("not-an-ip".to_string());
        cfg.me.manual_public_port = Some(4000);
        assert_eq!(cfg.manual_public_addr(), None);
    }

    #[test]
    fn cached_public_addr_disabled_by_default_even_with_values_present() {
        let mut cfg = base_config();
        cfg.me.cached_public_ip = Some("203.0.113.9".to_string());
        cfg.me.cached_public_port = Some(9000);
        // cache_public_addr is false -- cached values should be ignored.
        assert_eq!(cfg.cached_public_addr(), None);
    }

    #[test]
    fn cached_public_addr_resolves_when_enabled() {
        let mut cfg = base_config();
        cfg.me.cache_public_addr = true;
        cfg.me.cached_public_ip = Some("203.0.113.9".to_string());
        cfg.me.cached_public_port = Some(9000);
        assert_eq!(cfg.cached_public_addr(), Some("203.0.113.9:9000".parse().unwrap()));
    }

    #[test]
    fn set_cached_public_addr_is_a_noop_when_caching_disabled() {
        let mut cfg = base_config();
        cfg.set_cached_public_addr("203.0.113.9:9000".parse().unwrap());
        assert_eq!(cfg.me.cached_public_ip, None);
        assert_eq!(cfg.me.cached_public_port, None);
    }

    #[test]
    fn set_and_clear_cached_public_addr_roundtrip() {
        let mut cfg = base_config();
        cfg.me.cache_public_addr = true;
        cfg.set_cached_public_addr("203.0.113.9:9000".parse().unwrap());
        assert_eq!(cfg.cached_public_addr(), Some("203.0.113.9:9000".parse().unwrap()));

        cfg.clear_cached_public_addr();
        assert_eq!(cfg.cached_public_addr(), None);
        assert_eq!(cfg.me.cached_public_ip, None);
        assert_eq!(cfg.me.cached_public_port, None);
    }

    #[test]
    fn manual_override_and_cache_are_independent() {
        // Setting a manual override should not disturb an existing cache
        // entry, and vice versa -- they're deliberately separate knobs.
        let mut cfg = base_config();
        cfg.me.cache_public_addr = true;
        cfg.set_cached_public_addr("203.0.113.9:9000".parse().unwrap());
        cfg.me.manual_public_ip = Some("198.51.100.1".to_string());
        cfg.me.manual_public_port = Some(1234);

        assert_eq!(cfg.manual_public_addr(), Some("198.51.100.1:1234".parse().unwrap()));
        assert_eq!(cfg.cached_public_addr(), Some("203.0.113.9:9000".parse().unwrap()));
    }
}
