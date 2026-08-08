use lan_mesh_core::{config, crypto, hosts, mesh, share, stun};

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
        "lan_mesh -- pure P2P virtual LAN for games, no third-party VPN service

Usage:
  lan_mesh init            Interactive first-time setup (creates mesh.toml)
  lan_mesh add-peer        Interactively add a peer to mesh.toml
  lan_mesh export          Print a one-line peer card to send to a friend
                           (they run `lan_mesh import <the line>`)
  lan_mesh import <card>   Add a peer from a card produced by their `export`
  lan_mesh list-peers      Show configured peers and their reachability
  lan_mesh myaddr          Discover your own external ip:port via STUN
                           (only needed if NOT using export/import)
  lan_mesh ping [N]        Measure round-trip latency to every peer over
                           the mesh transport (N probes, default 5). Does
                           NOT need root/Administrator.
  lan_mesh run             Start the mesh (creates the virtual adapter,
                           needs root/Administrator)
  lan_mesh genkey          Generate a fresh pre-shared key to share with
                           everyone in your mesh
  lan_mesh domains         Show the local mesh domain name (<name>.<suffix>)
                           for yourself and every configured peer

Quickest way to connect with a friend:
  1. Both run `lan_mesh init` (one of you leaves the PSK prompt empty to
     generate a new key, and sends that exact key to the other over chat/
     voice -- do this once per group, never post it publicly).
  2. Both run `lan_mesh export`, and send each other the single printed
     line.
  3. Both run `lan_mesh import <the line your friend sent you>`.
  4. Both run `lan_mesh run`.
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

    println!("=== lan_mesh setup ===");
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

    let prefix_str = prompt_with_default("Subnet prefix length", "24");
    let prefix: u8 = prefix_str.parse().unwrap_or(24);

    let listen_port_str = prompt_with_default("UDP port to listen on", "54321");
    let listen_port: u16 = listen_port_str.parse().unwrap_or(54321);

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

    let domain_suffix = prompt_with_default(
        "Local domain suffix (peers become reachable as <name>.<suffix>)",
        "mesh",
    );

    let cfg = Config {
        me: MeConfig {
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
        },
        peers: Vec::new(),
    };
    cfg.save().expect("failed to write mesh.toml");
    println!("\nSaved {}.", config::CONFIG_PATH);
    println!("Next: run `lan_mesh export`, send the printed line to a friend, and have them");
    println!("run `lan_mesh import <that line>` (and vice versa) to connect.");
}

fn cmd_add_peer() {
    let mut cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
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
    let public_ip = prompt("Peer's public IP (what they got from `lan_mesh myaddr`): ");
    let public_port_str = prompt("Peer's public port (from `lan_mesh myaddr`): ");
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
    println!("Peer added. Run `lan_mesh list-peers` to review, `lan_mesh run` to start the mesh.");
}

fn cmd_export() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
            return;
        }
    };
    println!(
        "Discovering your external ip:port via STUN (local port {})...",
        cfg.me.listen_port
    );
    let Some(addr) = stun::discover_external_addr_any(cfg.me.listen_port) else {
        eprintln!(
            "Could not reach any STUN server -- check your internet connection, \
or run `lan_mesh myaddr` for more detail."
        );
        return;
    };
    let card = share::encode(&cfg.me.name, cfg.me.virtual_ip, &addr.ip().to_string(), addr.port());
    println!("\nSend this exact line to your friend (they run `lan_mesh import <line>`):\n");
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
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
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
    cfg.save().expect("failed to write mesh.toml");
    println!("Run `lan_mesh list-peers` to review, `lan_mesh run` to start the mesh.");
}

fn cmd_ping(count: u32) {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
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
        println!("No peers configured yet. Use `lan_mesh add-peer`.");
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
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
            return;
        }
    };
    println!(
        "Probing STUN servers using local port {} (same port the mesh listens on)...",
        cfg.me.listen_port
    );
    let mut results = std::collections::HashSet::new();
    for (host, port) in stun::DEFAULT_SERVERS {
        match stun::discover_external_addr(cfg.me.listen_port, host, *port) {
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
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
            return;
        }
    };
    let mut raw: Vec<(String, Ipv4Addr)> = vec![(cfg.me.name.clone(), cfg.me.virtual_ip)];
    for p in &cfg.peers {
        raw.push((p.name.clone(), p.virtual_ip));
    }
    let entries = hosts::build_entries(&cfg.me.domain_suffix, &raw);
    println!("Local mesh domain names (suffix: '{}'):\n", cfg.me.domain_suffix);
    for e in &entries {
        println!("  {:<28} -> {}", e.hostname, e.virtual_ip);
    }
    println!();
    if cfg.me.sync_hosts_file {
        println!(
            "Hosts-file sync is ENABLED -- these names already work in any app on this \
             machine while `lan_mesh run` is active (managed block in {}).",
            hosts::hosts_file_path().display()
        );
    } else {
        println!("Hosts-file sync is DISABLED in mesh.toml (sync_hosts_file = false).");
    }
    if cfg.me.dns_server {
        println!(
            "Built-in DNS resolver is ENABLED on 127.0.0.1:{} -- point a device's DNS \
             settings at this machine's LAN/mesh IP to resolve these names from other \
             devices too, not just this one.",
            cfg.me.dns_port
        );
    } else {
        println!("Built-in DNS resolver is DISABLED in mesh.toml (dns_server = false).");
    }
}

fn cmd_run() {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not load {}: {e}. Run `lan_mesh init` first.", config::CONFIG_PATH);
            return;
        }
    };
    if let Err(e) = mesh::run(cfg) {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("init") => cmd_init(),
        Some("add-peer") => cmd_add_peer(),
        Some("export") => cmd_export(),
        Some("import") => match args.get(2) {
            Some(card) => cmd_import(card),
            None => eprintln!("Usage: lan_mesh import <card>"),
        },
        Some("list-peers") => cmd_list_peers(),
        Some("myaddr") => cmd_myaddr(),
        Some("ping") => {
            let count: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
            cmd_ping(count);
        }
        Some("genkey") => cmd_genkey(),
        Some("domains") => cmd_domains(),
        Some("run") => cmd_run(),
        _ => print_usage(),
    }
}
