use serde::{Deserialize, Serialize};
use socket2::{Domain, Socket, Type};
use std::fs;
use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::process::Command;
use std::thread;

const CONFIG_PATH: &str = "config.toml";
const DEFAULT_PORT: u16 = 54320;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Config {
    my_port: u16,
    peer_ip: String,
    peer_port: u16,
}

fn log(msg: &str) {
    let now = chrono::Local::now().format("%H:%M:%S");
    println!("[{}] {}", now, msg);
    io::stdout().flush().unwrap();
}

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

fn load_or_create_config() -> Config {
    if Path::new(CONFIG_PATH).exists() {
        let content = fs::read_to_string(CONFIG_PATH).expect("Failed to read config file");
        let config: Config = toml::from_str(&content).expect("Failed to parse config file");
        log(&format!(
            "Loaded config: peer {}:{}, listening on port {}",
            config.peer_ip, config.peer_port, config.my_port
        ));

        let change = prompt("Use saved peer info? (Y/n): ");
        if change.eq_ignore_ascii_case("n") {
            return create_new_config();
        }

        config
    } else {
        log("No config found, let's set one up.");
        create_new_config()
    }
}

fn create_new_config() -> Config {
    let my_port: u16 = prompt_with_default("Your listen port", &DEFAULT_PORT.to_string())
        .parse()
        .unwrap_or(DEFAULT_PORT);

    let peer_ip = loop {
        let ip = prompt("Peer's public IP: ");
        if !ip.is_empty() {
            break ip;
        }
        println!("IP cannot be empty.");
    };

    let peer_port: u16 = prompt_with_default("Peer's port", &DEFAULT_PORT.to_string())
        .parse()
        .unwrap_or(DEFAULT_PORT);

    let config = Config {
        my_port,
        peer_ip,
        peer_port,
    };

    save_config(&config);
    config
}

fn save_config(config: &Config) {
    let toml_str = toml::to_string_pretty(config).expect("Failed to serialize config");
    fs::write(CONFIG_PATH, toml_str).expect("Failed to write config file");
    log(&format!("Config saved to {}", CONFIG_PATH));
}

/// Fetch your current public IP via an external echo service.
/// Tries a couple of providers in case one is down.
fn get_public_ip() -> Option<String> {
    let providers = ["https://api.ipify.org", "https://ifconfig.me/ip"];

    for url in providers {
        match ureq::get(url).timeout(std::time::Duration::from_secs(5)).call() {
            Ok(response) => {
                if let Ok(ip) = response.into_string() {
                    let ip = ip.trim().to_string();
                    if !ip.is_empty() {
                        return Some(ip);
                    }
                }
            }
            Err(_) => continue,
        }
    }
    None
}

fn get_real_interface() -> Option<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("ip route show default | grep -v CloudflareWARP | head -1 | awk '{print $5}'")
        .output()
        .ok()?;

    let iface = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if iface.is_empty() {
        None
    } else {
        Some(iface)
    }
}

fn create_direct_socket(my_port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    socket.set_reuse_address(true)?;

    if let Some(iface) = get_real_interface() {
        #[cfg(target_os = "linux")]
        {
            match socket.bind_device(Some(iface.as_bytes())) {
                Ok(_) => log(&format!("Socket bound to device: {}", iface)),
                Err(e) => log(&format!(
                    "Could not bind to device ({}). Continuing without it.",
                    e
                )),
            }
        }
    }

    let addr: SocketAddr = format!("0.0.0.0:{}", my_port).parse().unwrap();
    socket.bind(&addr.into())?;

    Ok(socket.into())
}

fn chat(config: Config) -> io::Result<()> {
    let sock = create_direct_socket(config.my_port)?;

    log(&format!("Listening on 0.0.0.0:{}", config.my_port));

    match get_public_ip() {
        Some(ip) => log(&format!("Your public IP appears to be: {}", ip)),
        None => log("Could not determine public IP (check internet connection)"),
    }

    log(&format!(
        "Sending to peer at {}:{}",
        config.peer_ip, config.peer_port
    ));

    let friend_addr: SocketAddr = format!("{}:{}", config.peer_ip, config.peer_port)
        .parse()
        .expect("Invalid peer address");

    let recv_sock = sock.try_clone()?;

    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match recv_sock.recv_from(&mut buf) {
                Ok((len, _addr)) => {
                    let msg = String::from_utf8_lossy(&buf[..len]);
                    print!("\n[Friend] {}\n> ", msg);
                    io::stdout().flush().unwrap();
                }
                Err(e) => {
                    log(&format!("Recv error: {}", e));
                    break;
                }
            }
        }
    });

    if let Err(e) = sock.send_to(b"Hi!", friend_addr) {
        log(&format!("Failed to send initial greeting: {}", e));
    }

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }

        let msg = line.trim_end();
        if msg.is_empty() {
            continue;
        }

        if let Err(e) = sock.send_to(msg.as_bytes(), friend_addr) {
            log(&format!("Send error: {}", e));
        }
    }

    println!("\nBye!");
    Ok(())
}

fn main() {
    let config = load_or_create_config();

    if let Err(e) = chat(config) {
        log(&format!("Fatal error: {}", e));
        std::process::exit(1);
    }
}
