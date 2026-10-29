use meow_meow_core::backends::client::{DirectoryClient, Member};
use meow_meow_core::{config, crypto, hosts, mesh, share, stun};

use config::{Config, MeConfig, PeerConfig};
use std::io::{self, BufRead, Write};
use std::net::Ipv4Addr;
use std::time::Duration;

fn prompt(msg: &str) -> String {
    print!("{}", msg);
    io::stdout().flush().unwrap();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).unwrap();
    line.trim().to_string()
}

fn prompt_with_default(msg: &str, default: &str) -> String {
    let input = prompt(&format!("{} [{}]: ", msg, default));
    if input.is_empty() {
        default.to_string()
    } else {
        input
    }
}

fn print_usage() {
    println!(
        "meow-meow -- pure P2P virtual LAN for games, no third-party VPN service

Usage:
  meow-meow directory ...   Shared directory so address changes and new members
                           propagate automatically (`meow-meow directory` for help)
  meow-meow init            Interactive first-time setup (creates mesh.toml)
  meow-meow add-peer        Interactively add a peer to mesh.toml
  meow-meow export          Print a one-line peer card to send to a friend
                           (they run `meow-meow import <the line>`)
  meow-meow import <card>   Add a peer from a card produced by their `export`
  meow-meow list-peers      Show configured peers and their reachability
  meow-meow myaddr          Discover your own external ip:port via STUN
                           (only needed if NOT using export/import)
  meow-meow ping [N]        Measure round-trip latency to every peer over
                           the mesh transport (N probes, default 5). Does
                           NOT need root/Administrator.
  meow-meow run             Start the mesh (creates the virtual adapter,
                           needs root/Administrator)
  meow-meow genkey          Generate a fresh pre-shared key to share with
                           everyone in your mesh
  meow-meow domains         Show the local mesh domain name (<name>.<suffix>)
                           for yourself and every configured peer
  meow-meow add-service <name> <port>
                           Advertise a named service you host (e.g. a game
                           server) as a subdomain of your own mesh name --
                           reachable as <service>.<yourname>.<suffix> once
                           the mesh is running, gossiped automatically to
                           everyone else
  meow-meow remove-service <name>
                           Stop advertising a service
  meow-meow list-services   Show services you've configured
  meow-meow set-public-addr <ip> <port>
                           Manually set your own public ip:port, bypassing
                           self-STUN discovery entirely -- for networks
                           where STUN is blocked/unreliable, or when you
                           already know the address (e.g. a server with a
                           static IP + port-forwarded router)
  meow-meow clear-public-addr
                           Remove a manual override, reverting to
                           automatic self-STUN discovery
  meow-meow reset-public-addr
                           Clear a cached public address (see
                           `cache-public-addr` below), forcing a fresh
                           self-STUN probe next run
  meow-meow warp-compat <on|off>
                           Toggle interface-pinning used to work around
                           always-on VPN/WARP clients rerouting the
                           mesh's own traffic. Leave ON unless self-STUN
                           still won't resolve on Windows even with no
                           VPN active, in which case try turning it OFF
  meow-meow cache-public-addr <on|off>
                           When ON, your discovered public address is
                           saved to mesh.toml so future launches have an
                           immediately usable value without waiting on
                           STUN to succeed again

Quickest way to connect with a friend:
  1. Both run `meow-meow init` (one of you leaves the PSK prompt empty to
     generate a new key, and sends that exact key to the other over chat/
     voice -- do this once per group, never post it publicly).
  2. Both run `meow-meow export`, and send each other the single printed
     line.
  3. Both run `meow-meow import <the line your friend sent you>`.
  4. Both run `meow-meow run`.
"
    );
}

fn cmd_init() {
    if Config::exists() {
        let ans = prompt(&format!(
            "{} already exists. Overwrite? (y/N): ",
            config::CONFIG_PATH
        ));
        if !ans.eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return;
        }
    }

    println!("=== meow-meow setup ===");
    println!(
        "Everyone in your mesh must use the SAME virtual subnet and the SAME\n\
         pre-shared key, but a DIFFERENT virtual IP within that subnet.\n"
    );

    let name = prompt_with_default("Your display name (shown to peers)", "player");

    let virtual_ip_str = prompt_with_default("Your virtual LAN IP", "10.66.0.1");
    let virtual_ip: Ipv4Addr = match virtual_ip_str.parse() {
        Ok(ip) => ip,
        Err(_) => {
            println!("Invalid IP, defaulting to 10.66.0.1");
            Ipv4Addr::new(10, 66, 0, 1)
        }
    };

    // Stable network defaults keep first-run setup to name, virtual IP and key.
    let prefix: u8 = 24;
    let listen_port: u16 = 54321;

    let psk = loop {
        let existing = prompt(
            "Paste the mesh's shared pre-shared key (leave empty to generate a NEW one \
             -- only do this if you are the first person setting up the mesh): ",
        );
        if existing.is_empty() {
            let generated = crypto::Cipher::generate_psk_b64();
            println!(
                "\nGenerated a new pre-shared key. Share this EXACT string with everyone \n\
                 you want in your mesh, over a channel you trust (do not post publicly):\n\n  {}\n",
                generated
            );
            break generated;
        }
        if crypto::Cipher::from_psk_b64(&existing).is_ok() {
            break existing;
        }
        println!("That doesn't look like a valid key (expected 32 bytes, base64-encoded). Try again.");
    };

    let domain_suffix = "mesh".to_string();

    let cfg = Config {
        me: MeConfig {
            repository_discovery: true,
            name,
            virtual_ip,
            prefix,
            listen_port,
            psk,
            mtu: 1400,
            domain_suffix,
            sync_hosts_file: true,
            dns_server: false,
            dns_port: 53,
            dns_auto_configure: false,
            manual_public_ip: None,
            manual_public_port: None,
            warp_compat: true,
            warp_split_tunnel_auto: true,
            cache_public_addr: false,
            cached_public_ip: None,
            cached_public_port: None,
        },
        peers: Vec::new(),
        services: Vec::new(),
    };
    cfg.save().expect("failed to write mesh.toml");
    println!("\nSaved {}.", config::CONFIG_PATH);
    println!("Repository discovery is enabled: the shared peer directory will be fetched automatically.");
}

fn cmd_add_peer() {
    let mut cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };

    let name = prompt("Peer's name (for your reference): ");
    let virtual_ip_str = prompt("Peer's virtual LAN IP (they chose this in their own `init`): ");
    let virtual_ip: Ipv4Addr = match virtual_ip_str.parse() {
        Ok(ip) => ip,
        Err(_) => {
            eprintln!("Invalid IP, aborting.");
            return;
        }
    };
    if virtual_ip == cfg.me.virtual_ip {
        eprintln!("That's your own virtual IP -- peers need distinct addresses. Aborting.");
        return;
    }
    let public_ip = prompt("Peer's public IP (what they got from `meow-meow myaddr`): ");
    let public_port_str = prompt("Peer's public port (from `meow-meow myaddr`): ");
    let public_port: u16 = match public_port_str.parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Invalid port, aborting.");
            return;
        }
    };

    cfg.peers.push(PeerConfig {
        name,
        virtual_ip,
        public_ip,
        public_port,
    });
    cfg.save().expect("failed to write mesh.toml");
    println!("Peer added. Run `meow-meow list-peers` to review, `meow-meow run` to start the mesh.");
}

fn cmd_export() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    println!(
        "Discovering your external ip:port via STUN (local port {})...",
        cfg.me.listen_port
    );
    // With WARP compatibility on (the default) the probe socket is pinned to
    // the real network interface so a VPN's exit IP isn't mistaken for yours.
    let probe = if cfg.me.warp_compat {
        stun::discover_external_addr_any_pinned(cfg.me.listen_port, Some(cfg.me.virtual_ip))
    } else {
        stun::discover_external_addr_any(cfg.me.listen_port)
    };
    let Some(addr) = probe else {
        eprintln!(
            "Could not reach any STUN server -- check your internet connection, \
or run `meow-meow myaddr` for more detail."
        );
        return;
    };
    let card = share::encode(&cfg.me.name, cfg.me.virtual_ip, &addr.ip().to_string(), addr.port());
    println!("\nSend this exact line to your friend (they run `meow-meow import <line>`):\n");
    println!("{card}\n");
    println!(
        "(Reminder: they also need your shared pre-shared key, sent separately, \
if they don't already have it.)"
    );
}

fn cmd_import(card: &str) {
    let mut cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    let peer = match share::decode(card) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Could not import peer card: {e}");
            return;
        }
    };
    if peer.virtual_ip == cfg.me.virtual_ip {
        eprintln!("That card's virtual IP is the same as yours -- refusing to add yourself.");
        return;
    }
    if let Some(existing) = cfg.peers.iter_mut().find(|p| p.virtual_ip == peer.virtual_ip) {
        *existing = peer.clone();
        println!("Updated existing peer '{}' ({}).", peer.name, peer.virtual_ip);
    } else {
        println!("Added peer '{}' ({}).", peer.name, peer.virtual_ip);
        cfg.peers.push(peer);
    }
    if let Err(e) = cfg.save() {
        eprintln!("Could not write {}: {e}", config::CONFIG_PATH);
        return;
    }
    println!("Run `meow-meow list-peers` to review, `meow-meow run` to start the mesh.");
}

fn cmd_ping(count: u32) {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    if let Err(e) = mesh::ping(cfg, count, Duration::from_secs(2)) {
        eprintln!("Error: {e}");
    }
}

fn cmd_list_peers() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}", config::CONFIG_PATH);
            return;
        }
    };
    println!("Your virtual IP: {}/{}", cfg.me.virtual_ip, cfg.me.prefix);
    println!("Broadcast address: {}", cfg.broadcast_addr());
    if cfg.peers.is_empty() {
        println!("No peers configured yet. Use `meow-meow add-peer`.");
        return;
    }
    for p in &cfg.peers {
        println!(
            "  {} -- virtual {} @ public {}:{}",
            p.name, p.virtual_ip, p.public_ip, p.public_port
        );
    }
}

fn cmd_myaddr() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    println!(
        "Probing STUN servers using local port {} (same port the mesh listens on)...",
        cfg.me.listen_port
    );
    let mut results = std::collections::HashSet::new();
    for (host, port) in stun::DEFAULT_SERVERS {
        // With WARP compatibility on (the default), probe through a socket pinned
        // to the real network interface so an active VPN's exit IP isn't mistaken
        // for yours.
        let addr = if cfg.me.warp_compat {
            stun::discover_external_addr_pinned(cfg.me.listen_port, Some(cfg.me.virtual_ip), host, *port)
        } else {
            stun::discover_external_addr(cfg.me.listen_port, host, *port)
        };
        match addr {
            Some(addr) => {
                println!("  via {host}:{port} -> {addr}");
                results.insert(addr);
            }
            None => println!("  via {host}:{port} -> failed"),
        }
    }
    if results.is_empty() {
        eprintln!("Could not reach any STUN server. Check your internet connection.");
        return;
    }
    if results.len() > 1 {
        println!(
            "\nWARNING: got different external mappings from different STUN servers.\n\
             Your NAT/CGNAT may use endpoint-dependent (symmetric-like) mapping,\n\
             meaning this address may not work when peers connect. Direct P2P\n\
             may be unreliable in this case."
        );
    }
    for addr in &results {
        println!(
            "\nShare with your peers -- public IP: {}   public port: {}",
            addr.ip(),
            addr.port()
        );
    }
}

fn cmd_genkey() {
    println!("{}", crypto::Cipher::generate_psk_b64());
}

fn cmd_domains() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    let mut infos: Vec<hosts::PeerDomainInfo> = vec![hosts::PeerDomainInfo {
        name: cfg.me.name.clone(),
        virtual_ip: cfg.me.virtual_ip,
        services: cfg.services.iter().map(|s| (s.name.clone(), s.port)).collect(),
    }];
    for p in &cfg.peers {
        infos.push(hosts::PeerDomainInfo {
            name: p.name.clone(),
            virtual_ip: p.virtual_ip,
            // Peers' *own* advertised services only become known once
            // the mesh is actually running and gossip has delivered
            // them -- mesh.toml never stores other peers' services, only
            // our own. This static/offline listing can't show them; use
            // the GUI's Domains screen (or a live status log) while the
            // mesh is running for the full picture including those.
            services: Vec::new(),
        });
    }
    let entries = hosts::build_entries_with_services(&cfg.me.domain_suffix, &infos);
    println!("Local mesh domain names (suffix: '{}'):\n", cfg.me.domain_suffix);
    for e in &entries {
        let port_note = e.port.map(|p| format!(" (port {p})")).unwrap_or_default();
        println!("  {:<28} -> {}{}", e.hostname, e.virtual_ip, port_note);
    }
    println!();
    if !cfg.services.is_empty() {
        println!(
            "(Peers' own advertised services aren't shown here -- they're only known once \
the mesh is running and gossip has delivered them. Run `meow-meow run` and check the \
GUI's Domains screen, or the activity log, for the live picture.)\n"
        );
    }
    if cfg.me.sync_hosts_file {
        println!(
            "Hosts-file sync is ENABLED -- these names already work in any app on this \
             machine while `meow-meow run` is active (managed block in {}).",
            hosts::hosts_file_path().display()
        );
    } else {
        println!("Hosts-file sync is DISABLED in mesh.toml (sync_hosts_file = false).");
    }
    if cfg.me.dns_server {
        println!(
            "Built-in DNS resolver is ENABLED on 127.0.0.1:{} -- point a device's DNS \
             settings at this machine's LAN/mesh IP to resolve these names from other \
             devices too, not just this one. It also answers wildcard subdomains of any \
             peer's own name (e.g. 'anything.alice.{}') automatically.",
            cfg.me.dns_port, cfg.me.domain_suffix
        );
    } else {
        println!("Built-in DNS resolver is DISABLED in mesh.toml (dns_server = false).");
    }
}

fn cmd_add_service(name: &str, port_str: &str) {
    let mut cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    let name = name.trim();
    if name.is_empty() {
        eprintln!("Service name can't be empty.");
        return;
    }
    let port: u16 = match port_str.parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Invalid port '{port_str}'.");
            return;
        }
    };
    if let Some(existing) = cfg.services.iter_mut().find(|s| s.name.eq_ignore_ascii_case(name)) {
        existing.port = port;
        println!("Updated service '{name}' -> port {port}.");
    } else {
        cfg.services.push(config::ServiceConfig {
            name: name.to_string(),
            port,
        });
        println!(
            "Added service '{name}' -> port {port}. Once the mesh is running, it'll be \
reachable as '{}.{}.{}' (and gossiped to the whole mesh automatically).",
            hosts::sanitize_label(name),
            hosts::sanitize_label(&cfg.me.name),
            cfg.me.domain_suffix
        );
    }
    cfg.save().expect("failed to write mesh.toml");
    println!("Restart the mesh (or use the GUI's live \"add service\") for a running instance to pick this up.");
}

fn cmd_remove_service(name: &str) {
    let mut cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    let before = cfg.services.len();
    cfg.services.retain(|s| !s.name.eq_ignore_ascii_case(name));
    if cfg.services.len() == before {
        println!("No service named '{name}' found.");
        return;
    }
    cfg.save().expect("failed to write mesh.toml");
    println!("Removed service '{name}'.");
}

fn cmd_list_services() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}", config::CONFIG_PATH);
            return;
        }
    };
    if cfg.services.is_empty() {
        println!("No services configured. Use `meow-meow add-service <name> <port>`.");
        return;
    }
    for s in &cfg.services {
        println!(
            "  {} -> port {} (reachable as {}.{}.{})",
            s.name,
            s.port,
            hosts::sanitize_label(&s.name),
            hosts::sanitize_label(&cfg.me.name),
            cfg.me.domain_suffix
        );
    }
}

fn cmd_run() {
    let cfg = match Config::load() {
        Ok(c) => c,

        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            return;
        }
    };
    if let Err(e) = mesh::run(cfg) {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}

fn load_or_die() -> Option<Config> {
    match Config::load() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            None
        }
    }
}

/// Manually sets this machine's public ip:port, bypassing self-STUN
/// discovery entirely -- for networks where STUN is blocked/unreliable,
/// or when you already know the address (e.g. a server with a static IP
/// and a manually port-forwarded router). Takes effect on the next `run`
/// (or immediately, if the mesh is running live in the GUI, via
/// `MeshHandle::set_manual_public_addr`).
fn cmd_set_public_addr(ip_str: &str, port_str: &str) {
    let Some(mut cfg) = load_or_die() else { return };
    let Ok(ip) = ip_str.parse::<Ipv4Addr>() else {
        eprintln!("Invalid IP address '{ip_str}'.");
        return;
    };
    let Ok(port) = port_str.parse::<u16>() else {
        eprintln!("Invalid port '{port_str}'.");
        return;
    };
    cfg.me.manual_public_ip = Some(ip.to_string());
    cfg.me.manual_public_port = Some(port);
    cfg.save().expect("failed to write mesh.toml");
    println!(
        "Manual public address set to {ip}:{port} -- self-STUN discovery is now disabled. \
Run `meow-meow clear-public-addr` to go back to automatic discovery."
    );
}

fn cmd_clear_public_addr() {
    let Some(mut cfg) = load_or_die() else { return };
    if cfg.me.manual_public_ip.is_none() && cfg.me.manual_public_port.is_none() {
        println!("No manual public address is set.");
        return;
    }
    cfg.me.manual_public_ip = None;
    cfg.me.manual_public_port = None;
    cfg.save().expect("failed to write mesh.toml");
    println!("Manual public address cleared -- automatic self-STUN discovery will be used.");
}

/// Clears any cached public address (see `cache-public-addr`), forcing a
/// fresh self-STUN probe on next `run` instead of continuing to
/// advertise a possibly-stale previous value. Does not touch a manual
/// override, if one is set.
fn cmd_reset_public_addr() {
    let Some(mut cfg) = load_or_die() else { return };
    cfg.clear_cached_public_addr();
    cfg.save().expect("failed to write mesh.toml");
    println!("Cached public address cleared.");
}

fn cmd_warp_compat(arg: &str) {
    let Some(mut cfg) = load_or_die() else { return };
    match arg {
        "on" | "true" | "enable" => {
            cfg.me.warp_compat = true;
            cfg.save().expect("failed to write mesh.toml");
            println!(
                "WARP-compatibility interface pinning ENABLED. The mesh will bind its UDP \
socket to your real network interface, explicitly skipping VPN/tunnel adapters \
(Cloudflare WARP, WireGuard, etc.) and its own virtual adapter."
            );
        }
        "off" | "false" | "disable" => {
            cfg.me.warp_compat = false;
            cfg.save().expect("failed to write mesh.toml");
            println!(
                "WARP-compatibility interface pinning DISABLED. The mesh's UDP socket will use \
whatever route the OS picks normally, like any other application -- use this if self-STUN \
still won't resolve on Windows even without a VPN/WARP active."
            );
        }
        other => eprintln!("Usage: meow-meow warp-compat <on|off> (got '{other}')"),
    }
}

fn cmd_cache_public_addr(arg: &str) {
    let Some(mut cfg) = load_or_die() else { return };
    match arg {
        "on" | "true" | "enable" => {
            cfg.me.cache_public_addr = true;
            cfg.save().expect("failed to write mesh.toml");
            println!(
                "Public address caching ENABLED. Once self-STUN succeeds, the result is saved to \
mesh.toml so future launches have an immediately usable address without waiting on STUN."
            );
        }
        "off" | "false" | "disable" => {
            cfg.me.cache_public_addr = false;
            cfg.save().expect("failed to write mesh.toml");
            println!("Public address caching DISABLED. (Any already-cached value is left in place; use `meow-meow reset-public-addr` to clear it.)");
        }
        other => eprintln!("Usage: meow-meow cache-public-addr <on|off> (got '{other}')"),
    }
}

const DIRECTORY_USAGE: &str = "Usage:
  meow-meow directory status
      Show configured directory services, the members they list, and any problems.
  meow-meow directory github <owner/repo> <token> [--branch <name>] [--path <file>]
      Use a GitHub repository as a directory service. The token must be a
      fine-grained token with contents read/write on that one repository.
  meow-meow directory mantle <namespace> [write-key]
      Use a MantleDB entry as a directory service. Leave the key out for an
      unclaimed namespace.
  meow-meow directory mantle-claim <namespace> [recovery-email]
      Claim a MantleDB namespace and store its write key in mesh.toml.
  meow-meow directory invite
      Print one line that carries the directory settings to a new member.
  meow-meow directory join <invite>
      Store directory settings from an invite line.
  meow-meow directory clear
      Stop using the shared directory.
Changes take effect the next time the mesh starts.";

fn cmd_directory(args: &[String]) {
    match args.get(2).map(|s| s.as_str()) {
        Some("status") => cmd_directory_status(),
        Some("github") => cmd_directory_github(&args[3.min(args.len())..]),
        Some("mantle") => cmd_directory_mantle(&args[3.min(args.len())..]),
        Some("mantle-claim") => cmd_directory_mantle_claim(&args[3.min(args.len())..]),
        Some("invite") => cmd_directory_invite(),
        Some("join") => match args.get(3) {
            Some(invite) => cmd_directory_join(invite),
            None => eprintln!("Usage: meow-meow directory join <invite>"),
        },
        Some("clear") => cmd_directory_clear(),
        _ => println!("{DIRECTORY_USAGE}"),
    }
}

fn load_config_or_explain() -> Option<Config> {
    match Config::load() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `meow-meow init` first.", config::CONFIG_PATH);
            None
        }
    }
}

fn human_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Describes the configured services without printing any secret.
fn print_settings_summary(settings: &config::DirectorySettings) {
    if let Some(g) = &settings.github {
        println!(
            "  github:  {} (branch {}, file {})",
            g.repo,
            g.branch.as_deref().unwrap_or("default"),
            g.path
        );
    }
    if let Some(m) = &settings.mantle {
        let key = if m.write_key.is_some() { "claimed" } else { "unclaimed" };
        println!("  mantle:  namespace {} ({key})", m.namespace);
    }
}

/// Reads the directory without publishing and prints what it lists.
fn print_directory_contents(cfg: &Config, settings: &config::DirectorySettings) {
    let mut client = match DirectoryClient::from_settings(settings, &cfg.me.psk) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let me = Member { name: cfg.me.name.clone(), virtual_ip: cfg.me.virtual_ip };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let result = client.sync(&me, None, now);
    if result.members.is_empty() {
        println!("Members: none listed yet (the first member appears after its mesh starts).");
    } else {
        println!("Members ({}):", result.members.len());
        for p in &result.members {
            println!(
                "  {:<14} {:<11} {}:{:<6} updated {}",
                p.name,
                p.virtual_ip,
                p.public_ip,
                p.public_port,
                human_age(now.saturating_sub(p.updated_at))
            );
        }
    }
    for note in &result.notes {
        println!("  problem: {note}");
    }
}

fn cmd_directory_status() {
    let Some(cfg) = load_config_or_explain() else { return };
    let settings = match config::DirectorySettings::load_optional() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Could not read {}: {e}", config::DIRECTORY_PATH);
            return;
        }
    };
    let Some(settings) = settings else {
        println!("No shared directory configured.");
        println!("Set one with `meow-meow directory github ...` or `meow-meow directory mantle ...`,");
        println!("or paste an invite with `meow-meow directory join <invite>`.");
        return;
    };
    println!("Shared directory services:");
    print_settings_summary(&settings);
    print_directory_contents(&cfg, &settings);
}

/// Validates, stores, and reads back the new settings.
fn save_directory(cfg: Config, settings: config::DirectorySettings) {
    if let Err(e) = backends_validate(&settings) {
        eprintln!("Not saved: {e}");
        return;
    }
    if let Err(e) = settings.save() {
        eprintln!("Could not write {}: {e}", config::DIRECTORY_PATH);
        return;
    }
    println!("Saved to {} (file mode 0600 on Unix).", config::DIRECTORY_PATH);
    print_settings_summary(&settings);
    print_directory_contents(&cfg, &settings);
    println!("Restart `meow-meow run` (or the GUI) to start syncing with the directory.");
}

fn backends_validate(settings: &config::DirectorySettings) -> Result<(), String> {
    meow_meow_core::backends::client::validate_settings(settings)
}

fn cmd_directory_github(args: &[String]) {
    let (Some(repo), Some(token)) = (args.first(), args.get(1)) else {
        println!("{DIRECTORY_USAGE}");
        return;
    };
    let mut branch = None;
    let mut path = None;
    let mut i = 2;
    while i < args.len() {
        let value = args.get(i + 1).cloned();
        match (args[i].as_str(), value) {
            ("--branch", Some(v)) => branch = Some(v),
            ("--path", Some(v)) => path = Some(v),
            (other, _) => {
                eprintln!("Unknown or incomplete option {other:?}.");
                return;
            }
        }
        i += 2;
    }
    let Some(cfg) = load_config_or_explain() else { return };
    let mut settings = current_directory_or_default();
    settings.github = Some(config::GithubSettings {
        repo: repo.clone(),
        branch,
        path: path.unwrap_or_else(|| meow_meow_core::backends::github::DEFAULT_PATH.to_string()),
        token: token.clone(),
    });
    save_directory(cfg, settings);
}

fn cmd_directory_mantle(args: &[String]) {
    let Some(namespace) = args.first() else {
        println!("{DIRECTORY_USAGE}");
        return;
    };
    let Some(cfg) = load_config_or_explain() else { return };
    let mut settings = current_directory_or_default();
    settings.mantle = Some(config::MantleSettings {
        namespace: namespace.clone(),
        write_key: args.get(1).cloned(),
    });
    save_directory(cfg, settings);
}

fn cmd_directory_mantle_claim(args: &[String]) {
    let Some(namespace) = args.first() else {
        println!("{DIRECTORY_USAGE}");
        return;
    };
    let Some(cfg) = load_config_or_explain() else { return };
    println!("Claiming MantleDB namespace {namespace}...");
    let key = match meow_meow_core::backends::mantle::claim(namespace, args.get(1).map(String::as_str)) {
        Ok(key) => key,
        Err(e) => {
            eprintln!("Claim failed: {e}");
            return;
        }
    };
    let mut settings = current_directory_or_default();
    settings.mantle = Some(config::MantleSettings {
        namespace: namespace.clone(),
        write_key: Some(key),
    });
    // The service shows the key only once, so it is stored right away and never printed.
    println!("Claimed. The write key is stored in {} and is not shown again.", config::DIRECTORY_PATH);
    save_directory(cfg, settings);
}

/// Existing directory settings, or empty ones. An unreadable file is reported
/// and treated as empty, so it is overwritten only by an explicit command.
fn current_directory_or_default() -> config::DirectorySettings {
    match config::DirectorySettings::load_optional() {
        Ok(Some(s)) => s,
        Ok(None) => config::DirectorySettings::default(),
        Err(e) => {
            eprintln!("Warning: could not read {}: {e}", config::DIRECTORY_PATH);
            config::DirectorySettings::default()
        }
    }
}

fn cmd_directory_invite() {
    if load_config_or_explain().is_none() {
        return;
    }
    let settings = match config::DirectorySettings::load_optional() {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("No directory configured to invite people to.");
            return;
        }
        Err(e) => {
            eprintln!("Could not read {}: {e}", config::DIRECTORY_PATH);
            return;
        }
    };
    match meow_meow_core::backends::client::encode_invite(&settings) {
        Ok(line) => {
            println!("{line}");
            println!();
            println!("This line contains the GitHub token and/or the MantleDB write key.");
            println!("Send it the same way as the network key: privately, never publicly.");
        }
        Err(e) => eprintln!("Could not create an invite: {e}"),
    }
}

fn cmd_directory_join(invite: &str) {
    let Some(cfg) = load_config_or_explain() else { return };
    match meow_meow_core::backends::client::decode_invite(invite) {
        Ok(settings) => save_directory(cfg, settings),
        Err(e) => eprintln!("Invite rejected: {e}"),
    }
}

fn cmd_directory_clear() {
    match config::DirectorySettings::remove() {
        Ok(()) => println!("Shared directory cleared. Peers you already know keep working."),
        Err(e) => eprintln!("Could not remove {}: {e}", config::DIRECTORY_PATH),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("directory") => cmd_directory(&args),
        Some("init") => cmd_init(),
        Some("add-peer") => cmd_add_peer(),
        Some("export") => cmd_export(),
        Some("import") => match args.get(2) {
            Some(card) => cmd_import(card),
            None => eprintln!("Usage: meow-meow import <card>"),
        },
        Some("list-peers") => cmd_list_peers(),
        Some("myaddr") => cmd_myaddr(),
        Some("ping") => {
            let count: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
            cmd_ping(count);
        }
        Some("genkey") => cmd_genkey(),
        Some("domains") => cmd_domains(),
        Some("add-service") => match (args.get(2), args.get(3)) {
            (Some(name), Some(port)) => cmd_add_service(name, port),
            _ => eprintln!("Usage: meow-meow add-service <name> <port>"),
        },
        Some("remove-service") => match args.get(2) {
            Some(name) => cmd_remove_service(name),
            None => eprintln!("Usage: meow-meow remove-service <name>"),
        },
        Some("list-services") => cmd_list_services(),
        Some("set-public-addr") => match (args.get(2), args.get(3)) {
            (Some(ip), Some(port)) => cmd_set_public_addr(ip, port),
            _ => eprintln!("Usage: meow-meow set-public-addr <ip> <port>"),
        },
        Some("clear-public-addr") => cmd_clear_public_addr(),
        Some("reset-public-addr") => cmd_reset_public_addr(),
        Some("warp-compat") => match args.get(2) {
            Some(arg) => cmd_warp_compat(arg),
            None => eprintln!("Usage: meow-meow warp-compat <on|off>"),
        },
        Some("cache-public-addr") => match args.get(2) {
            Some(arg) => cmd_cache_public_addr(arg),
            None => eprintln!("Usage: meow-meow cache-public-addr <on|off>"),
        },
        Some("run") => cmd_run(),
        _ => print_usage(),
    }
}
