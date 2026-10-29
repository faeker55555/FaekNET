//! One-step network setup for the GUI: create a network, or join one from an
//! invite. Each call either completes fully or leaves no trace behind.
//!
//! Order of operations:
//! 1. Check the inputs and that this folder has no network yet.
//! 2. Write both files to temporary names. This proves the folder is writable
//!    before anything is claimed or read from the network.
//! 3. Talk to the network (claim a namespace, or read the member list).
//! 4. Move the files into place.
//!
//! If step 3 fails, the temporary files are removed and nothing is claimed.
//! The only unrecoverable case is a failure after a claim has succeeded. Then
//! the error text contains the invite, so the write key is not lost.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::backends::client::{sanitize_name, DirectoryClient, Member};
use crate::backends::{mantle, PeerRecord};
use crate::config::{self, Config, DirectorySettings, MantleSettings, MeConfig};
use crate::crypto::Cipher;
use crate::invite;

/// The mesh subnet. Virtual IPs must be 10.66.0.1 to 10.66.0.254.
pub const SUBNET: [u8; 3] = [10, 66, 0];
pub const DEFAULT_LISTEN_PORT: u16 = 54321;
const NAMESPACE_LABEL: &[u8] = b"faeknet-namespace-v1";

/// Network access used by setup. The live implementation talks to MantleDB;
/// tests substitute a fake.
pub trait Network {
    /// Claims a directory namespace and returns its write key. The key is shown
    /// once by the service, so the caller must store it.
    fn claim(&self, namespace: &str) -> Result<String, String>;
    /// Reads the current members of a network. Errors mean no service answered,
    /// which is different from an empty network (`Ok(vec![])`).
    fn members(&self, directory: &DirectorySettings, psk: &str) -> Result<Vec<PeerRecord>, String>;
}

/// The real network: MantleDB, through the same code the mesh uses.
pub struct Live;

impl Network for Live {
    fn claim(&self, namespace: &str) -> Result<String, String> {
        mantle::claim(namespace, None)
    }

    fn members(&self, directory: &DirectorySettings, psk: &str) -> Result<Vec<PeerRecord>, String> {
        let mut client = DirectoryClient::from_settings(directory, psk)?;
        // A placeholder that no member uses, so we only read.
        let reader = Member { name: "setup".into(), virtual_ip: Ipv4Addr::UNSPECIFIED };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let result = client.sync(&reader, None, now);
        if !result.reachable {
            let why = result.notes.first().cloned().unwrap_or_else(|| "no response".into());
            return Err(format!("could not reach the network directory ({why})"));
        }
        Ok(result.members)
    }
}

/// Where the two setup files are written. Production uses the working directory.
#[derive(Clone, Debug)]
pub struct Location {
    pub config: PathBuf,
    pub directory: PathBuf,
}

impl Location {
    pub fn current_dir() -> Self {
        Location { config: PathBuf::from(config::CONFIG_PATH), directory: PathBuf::from(config::DIRECTORY_PATH) }
    }

    fn already_set_up(&self) -> bool {
        self.config.exists() || self.directory.exists()
    }
}

/// Creates a new network. Returns the invite to share with friends.
/// `virtual_ip` of `None` means 10.66.0.1.
pub fn create_network(net: &dyn Network, loc: &Location, name: &str, virtual_ip: Option<Ipv4Addr>) -> Result<String, String> {
    let vip = virtual_ip.unwrap_or(Ipv4Addr::new(10, 66, 0, 1));
    let name = check_name(name)?;
    check_vip(vip)?;
    if loc.already_set_up() {
        return Err(already_set_up_message());
    }

    let psk = Cipher::generate_psk_b64();
    let namespace = namespace_for_psk(&psk)?;
    let directory = DirectorySettings {
        github: None,
        mantle: Some(MantleSettings { namespace: namespace.clone(), write_key: None }),
    };
    let config = make_config(&name, vip, &psk);

    let staged = Staged::prepare(loc, &config, &directory)?;
    let write_key = match net.claim(&namespace) {
        Ok(key) => key,
        Err(e) => {
            staged.discard();
            return Err(format!("Could not create the network: {e}. Nothing was saved."));
        }
    };

    let mut claimed = directory.clone();
    if let Some(m) = claimed.mantle.as_mut() {
        m.write_key = Some(write_key);
    }
    let invite = invite::encode(&psk, &claimed)?;
    if let Err(e) = staged.commit(&claimed) {
        return Err(format!(
            "The network was created on MantleDB, but saving it on this computer failed ({e}). \
             Keep this invite, it is the only copy of the write key: {invite}"
        ));
    }
    Ok(invite)
}

/// Joins a network from an invite. Reads the member list first, so a taken
/// virtual IP is refused and an unreachable directory saves nothing.
/// `virtual_ip` of `None` picks the first free address.
pub fn join_network(net: &dyn Network, loc: &Location, invite_text: &str, name: &str, virtual_ip: Option<Ipv4Addr>) -> Result<Config, String> {
    let (psk, directory) = invite::decode(invite_text)?;
    let name = check_name(name)?;
    if let Some(ip) = virtual_ip {
        check_vip(ip)?;
    }
    if loc.already_set_up() {
        return Err(already_set_up_message());
    }
    let members = net
        .members(&directory, &psk)
        .map_err(|e| format!("Could not join the network: {e}. Nothing was saved. Check your internet connection and try again."))?;

    let vip = match virtual_ip {
        Some(ip) => {
            if let Some(taken) = members.iter().find(|m| m.virtual_ip == ip) {
                let free = suggest_free_ip(&members);
                return Err(format!(
                    "{ip} is already used by \"{}\" in this network. Try {free} or leave the field empty.",
                    taken.name
                ));
            }
            ip
        }
        None => suggest_free_ip(&members),
    };

    let config = make_config(&name, vip, &psk);
    let staged = Staged::prepare(loc, &config, &directory)?;
    staged.commit(&directory).map_err(|e| format!("Could not save the network settings: {e}"))?;
    Ok(config)
}

/// First address in the subnet that no listed member uses. Starts at .2 so that
/// .1 stays the usual address of the network's creator.
pub fn suggest_free_ip(members: &[PeerRecord]) -> Ipv4Addr {
    (2u8..=254)
        .map(|last| Ipv4Addr::new(SUBNET[0], SUBNET[1], SUBNET[2], last))
        .find(|ip| !members.iter().any(|m| m.virtual_ip == *ip))
        .unwrap_or(Ipv4Addr::new(SUBNET[0], SUBNET[1], SUBNET[2], 254))
}

/// Derives the directory namespace from the network key. Only people with the
/// key can compute it, so outsiders cannot guess the namespace.
pub fn namespace_for_psk(psk_b64: &str) -> Result<String, String> {
    Cipher::from_psk_b64(psk_b64).map_err(|e| format!("invalid network key: {e}"))?;
    let psk = base64::engine::general_purpose::STANDARD
        .decode(psk_b64.trim())
        .map_err(|e| format!("invalid network key: {e}"))?;
    let mut h = Sha256::new();
    h.update(NAMESPACE_LABEL);
    h.update(&psk);
    let digest = h.finalize();
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("faeknet-{hex}"))
}

fn already_set_up_message() -> String {
    format!(
        "This folder already has a network ({} or {} exists). Remove those files to start over.",
        config::CONFIG_PATH,
        config::DIRECTORY_PATH
    )
}

/// Validates the display name. Returns the name as it will be stored.
fn check_name(name: &str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("Enter a display name.".into());
    }
    Ok(sanitize_name(name.trim()))
}

fn check_vip(vip: Ipv4Addr) -> Result<(), String> {
    let o = vip.octets();
    if o[..3] != SUBNET || o[3] == 0 || o[3] == 255 {
        return Err("The virtual IP must be between 10.66.0.1 and 10.66.0.254.".into());
    }
    Ok(())
}

fn make_config(name: &str, vip: Ipv4Addr, psk: &str) -> Config {
    Config {
        me: MeConfig {
            // The GitHub Issues bootstrap is off: a shared directory replaces it,
            // and it would publish endpoints in public issues.
            repository_discovery: false,
            name: name.to_string(),
            virtual_ip: vip,
            prefix: 24,
            listen_port: DEFAULT_LISTEN_PORT,
            psk: psk.to_string(),
            mtu: 1400,
            domain_suffix: "mesh".to_string(),
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
    }
}

fn tmp_path(p: &Path) -> PathBuf {
    PathBuf::from(format!("{}.tmp", p.display()))
}

fn write_directory(path: &Path, directory: &DirectorySettings) -> std::io::Result<()> {
    let text = toml::to_string_pretty(directory).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    config::write_private(&path.to_string_lossy(), text.as_bytes())
}

/// Both setup files, written to temporary names first.
struct Staged {
    config_tmp: PathBuf,
    directory_tmp: PathBuf,
    config_final: PathBuf,
    directory_final: PathBuf,
}

impl Staged {
    fn prepare(loc: &Location, config: &Config, directory: &DirectorySettings) -> Result<Staged, String> {
        let staged = Staged {
            config_tmp: tmp_path(&loc.config),
            directory_tmp: tmp_path(&loc.directory),
            config_final: loc.config.clone(),
            directory_final: loc.directory.clone(),
        };
        let written = (|| -> Result<(), String> {
            let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
            fs::write(&staged.config_tmp, text).map_err(|e| e.to_string())?;
            write_directory(&staged.directory_tmp, directory).map_err(|e| e.to_string())
        })();
        match written {
            Ok(()) => Ok(staged),
            Err(e) => {
                staged.discard();
                Err(format!("Could not write the network files here ({e}). Check that this folder is writable. Nothing was claimed or saved."))
            }
        }
    }

    /// Writes the final directory settings (which may now include the write
    /// key) and moves both files into place. Leaves no temporary files behind.
    fn commit(self, directory: &DirectorySettings) -> Result<(), String> {
        let result = (|| -> std::io::Result<()> {
            write_directory(&self.directory_tmp, directory)?;
            fs::rename(&self.config_tmp, &self.config_final)?;
            if let Err(e) = fs::rename(&self.directory_tmp, &self.directory_final) {
                // Roll back, so the folder never holds a mesh.toml without its directory.
                let _ = fs::remove_file(&self.config_final);
                return Err(e);
            }
            Ok(())
        })();
        if result.is_err() {
            self.discard();
        }
        result.map_err(|e| e.to_string())
    }

    fn discard(&self) {
        let _ = fs::remove_file(&self.config_tmp);
        let _ = fs::remove_file(&self.directory_tmp);
    }
}

#[cfg(test)]
mod tests;
