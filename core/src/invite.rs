//! The network invite: one line that carries everything a new member needs.
//!
//! It holds the network key and the shared-directory settings (MantleDB
//! namespace and write key), so nothing else has to be exchanged. Anyone who
//! has the invite can join and can publish to the directory, so share it like
//! a password, over a channel you trust.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::backends::client::validate_settings;
use crate::config::DirectorySettings;
use crate::crypto::Cipher;

pub const PREFIX: &str = "FNNET1:";
const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    v: u8,
    psk: String,
    directory: DirectorySettings,
}

/// Encodes a network key and its directory settings as one invite line.
pub fn encode(psk: &str, directory: &DirectorySettings) -> Result<String, String> {
    Cipher::from_psk_b64(psk.trim()).map_err(|e| format!("invalid network key: {e}"))?;
    require_mantle(directory)?;
    validate_settings(directory)?;
    let payload = Payload { v: VERSION, psk: psk.trim().to_string(), directory: directory.clone() };
    let json = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    Ok(format!("{PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)))
}

/// Decodes an invite line into (network key, directory settings). Every part
/// is checked, so a damaged or foreign code is rejected before anything is saved.
pub fn decode(text: &str) -> Result<(String, DirectorySettings), String> {
    let text = text.trim();
    let body = text
        .strip_prefix(PREFIX)
        .ok_or_else(|| format!("This is not a FaekNET invite. It should start with {PREFIX}"))?;
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| "The invite is damaged. Copy the whole code again.".to_string())?;
    let payload: Payload =
        serde_json::from_slice(&json).map_err(|_| "The invite is damaged. Copy the whole code again.".to_string())?;
    if payload.v != VERSION {
        return Err(format!("This invite needs a newer FaekNET (invite version {}).", payload.v));
    }
    Cipher::from_psk_b64(&payload.psk).map_err(|_| "The invite's network key is damaged.".to_string())?;
    require_mantle(&payload.directory)?;
    validate_settings(&payload.directory)?;
    Ok((payload.psk, payload.directory))
}

fn require_mantle(directory: &DirectorySettings) -> Result<(), String> {
    if directory.mantle.is_none() {
        return Err("The invite has no directory service.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MantleSettings;

    fn directory(write_key: Option<&str>) -> DirectorySettings {
        DirectorySettings {
            github: None,
            mantle: Some(MantleSettings {
                namespace: "faeknet-test-1234".into(),
                write_key: write_key.map(str::to_string),
            }),
        }
    }

    #[test]
    fn invite_roundtrips_key_namespace_and_write_key() {
        let psk = Cipher::generate_psk_b64();
        let invite = encode(&psk, &directory(Some("writekey12345"))).unwrap();
        assert!(invite.starts_with(PREFIX));
        let (back_psk, back_dir) = decode(&format!("  {invite}\n")).unwrap();
        assert_eq!(back_psk, psk);
        let m = back_dir.mantle.unwrap();
        assert_eq!(m.namespace, "faeknet-test-1234");
        assert_eq!(m.write_key.as_deref(), Some("writekey12345"));
    }

    #[test]
    fn foreign_damaged_and_incomplete_invites_are_refused() {
        let psk = Cipher::generate_psk_b64();
        assert!(decode("LMESH1:whatever").unwrap_err().contains("not a FaekNET invite"));
        assert!(decode("FNNET1:!!!").unwrap_err().contains("damaged"));
        assert!(decode("FNDIR1:e30").is_err());
        let no_directory = DirectorySettings::default();
        assert!(encode(&psk, &no_directory).is_err());
        assert!(encode("not-a-key", &directory(None)).is_err());
    }

    #[test]
    fn github_directory_is_never_put_in_an_invite() {
        let psk = Cipher::generate_psk_b64();
        let mut d = directory(None);
        d.github = Some(crate::config::GithubSettings {
            repo: "owner/repo".into(),
            branch: None,
            path: "x.json".into(),
            token: "github_pat_0123456789abcdefghij".into(),
        });
        assert!(encode(&psk, &d).is_err());
    }
}
