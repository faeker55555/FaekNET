mod config;
mod crypto;
mod mesh;
mod peer;
mod proto;
mod stun;

use config::{Config, MeConfig, PeerConfig};
use std::io::{self, BufRead, Write};
use std::net::Ipv4Addr;

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
  lan_mesh list-peers      Show configured peers and their reachability
  lan_mesh myaddr          Discover your own external ip:port via STUN
                           (share this with peers so they can add you)
  lan_mesh run             Start the mesh (creates the virtual adapter,
                           needs root/Administrator)
  lan_mesh genkey          Generate a fresh pre-shared key to share with
                           everyone in your mesh
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

    let cfg = Config {
        me: MeConfig {
            virtual_ip,
            prefix,
            listen_port,
            psk,
            mtu: 1400,
        },
        peers: Vec::new(),
    };
    cfg.save().expect("failed to write mesh.toml");
    println!("\nSaved {}.", config::CONFIG_PATH);
    println!("Next: run `lan_mesh myaddr` to find your external ip:port to share with peers,");
    println!("then have each peer run `lan_mesh add-peer` to add you (and vice versa).");
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
        Some("list-peers") => cmd_list_peers(),
        Some("myaddr") => cmd_myaddr(),
        Some("genkey") => cmd_genkey(),
        Some("run") => cmd_run(),
        _ => print_usage(),
    }
}
