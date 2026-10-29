//! The directory client: what the mesh runtime calls every minute.
//!
//! Each sync does three things:
//! 1. Reads every configured backend and keeps the newest valid directory.
//! 2. Publishes our own record when our public endpoint or name changed, or as
//!    a heartbeat every `HEARTBEAT_SECS`. Other members' records are carried
//!    forward unchanged, and records not refreshed for `STALE_AFTER_SECS` are
//!    dropped.
//! 3. Returns the other members' records, so the caller can update its peer
//!    table. A member that changes IP or port is picked up by everyone within
//!    about one sync interval, with no manual step.

use std::net::{Ipv4Addr, SocketAddr};

use base64::Engine;

use super::github::GithubBackend;
use super::mantle::MantleBackend;
use super::{fetch_latest, next_document, publish_all, Backend, Document, DirectoryKey, PeerRecord};
use crate::config::DirectorySettings;
use crate::registry::public_ipv4;

/// How often the mesh runs a sync.
pub const SYNC_INTERVAL_SECS: u64 = 60;
/// Re-publish our record at least this often, even when nothing changed.
/// This keeps us from going stale and heals records lost to a concurrent write.
pub const HEARTBEAT_SECS: u64 = 30 * 60;
/// Members not refreshed for this long are dropped from the directory.
pub const STALE_AFTER_SECS: u64 = 24 * 3600;

const INVITE_PREFIX: &str = "FNDIR1:";
const NETWORK_PREFIX: [u8; 3] = [10, 66, 0];

/// Who we are, as the directory should describe us.
#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub virtual_ip: Ipv4Addr,
}

/// What one sync produced.
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Other members' current records, from the newest valid directory.
    pub members: Vec<PeerRecord>,
    /// True if this sync published a new directory.
    pub published: bool,
    /// Human-readable problems. The caller logs them, deduplicated across syncs.
    pub notes: Vec<String>,
}

pub struct DirectoryClient {
    backends: Vec<Box<dyn Backend>>,
    key: DirectoryKey,
    /// Highest directory sequence number accepted so far. Anti-rollback floor.
    floor_seq: u64,
    latest: Option<Document>,
    /// Last address we published and when. None until the first publish.
    last_publish: Option<(SocketAddr, u64)>,
}

impl DirectoryClient {
    /// Builds a client over the given backends. Used by tests and by callers that
    /// bring their own storage.
    pub fn with_backends(backends: Vec<Box<dyn Backend>>, key: DirectoryKey) -> Self {
        DirectoryClient { backends, key, floor_seq: 0, latest: None, last_publish: None }
    }

    /// Builds the client from `[directory]` settings and the network PSK.
    pub fn from_settings(settings: &DirectorySettings, psk_b64: &str) -> Result<Self, String> {
        let key = DirectoryKey::from_psk_b64(psk_b64)?;
        let backends = build_backends(settings)?;
        Ok(DirectoryClient { backends, key, floor_seq: 0, latest: None, last_publish: None })
    }

    /// Runs one sync. `my_addr` is our current public endpoint, or `None` if we
    /// do not know it yet, in which case the sync only reads.
    pub fn sync(&mut self, me: &Member, my_addr: Option<SocketAddr>, now: u64) -> SyncResult {
        let mut result = SyncResult::default();
        let report = fetch_latest(&self.backends, &self.key, self.floor_seq, now);
        result.notes.extend(report.problems.iter().cloned());
        if let Some(best) = report.best {
            self.floor_seq = self.floor_seq.max(best.seq);
            self.latest = Some(best);
        }

        if let Some(addr) = my_addr {
            if report.answered == 0 {
                // Every service failed to answer. We cannot tell the current
                // membership, so publishing now would replace the real directory
                // with one that lists only us.
                result.notes.push("not publishing: no directory service answered this sync".into());
            } else {
                match self.publish_if_due(me, addr, now, &mut result.notes) {
                    Ok(true) => result.published = true,
                    Ok(false) => {}
                    Err(note) => result.notes.push(note),
                }
            }
        }

        if let Some(doc) = &self.latest {
            result.members = doc
                .peers
                .iter()
                .filter(|p| p.virtual_ip != me.virtual_ip && is_fresh(p, now))
                .cloned()
                .collect();
        }
        result
    }

    fn publish_if_due(&mut self, me: &Member, addr: SocketAddr, now: u64, notes: &mut Vec<String>) -> Result<bool, String> {
        let mine = own_record(me, addr, now)?;
        let current = self.latest.as_ref().and_then(|d| d.peers.iter().find(|p| p.virtual_ip == me.virtual_ip));
        let differs = match current {
            None => true,
            Some(p) => p.name != mine.name || p.public_ip != mine.public_ip || p.public_port != mine.public_port,
        };
        let heartbeat_due = match self.last_publish {
            None => true,
            Some((last_addr, at)) => last_addr != addr || now >= at.saturating_add(HEARTBEAT_SECS),
        };
        if !differs && !heartbeat_due {
            return Ok(false);
        }

        let mut peers: Vec<PeerRecord> = self
            .latest
            .as_ref()
            .map(|d| d.peers.iter().filter(|p| p.virtual_ip != me.virtual_ip && is_fresh(p, now)).cloned().collect())
            .unwrap_or_default();
        peers.push(mine);
        peers.sort_by_key(|p| p.virtual_ip.octets());

        let mut doc = next_document(self.latest.as_ref(), peers, now);
        // Never publish at or below a sequence number we already accepted. Members
        // ignore anything below their floor, so a lower seq would be invisible to them.
        doc.seq = doc.seq.max(self.floor_seq.saturating_add(1));
        doc.validate(now).map_err(|e| format!("not publishing: {e}"))?;
        let report = publish_all(&self.backends, &self.key, &doc).map_err(|e| format!("publish failed: {e}"))?;
        // A failing service is only a note: it must not block the others.
        notes.extend(report.failed.iter().map(|f| format!("publish skipped {f}")));
        self.floor_seq = self.floor_seq.max(doc.seq);
        self.latest = Some(doc);
        self.last_publish = Some((addr, now));
        Ok(true)
    }
}

fn is_fresh(p: &PeerRecord, now: u64) -> bool {
    p.updated_at.saturating_add(STALE_AFTER_SECS) >= now
}

/// Builds our record, or explains why we should not publish one. The receiving
/// side rejects a whole directory if any record is invalid, so we must never send
/// one that fails validation.
fn own_record(me: &Member, addr: SocketAddr, now: u64) -> Result<PeerRecord, String> {
    let SocketAddr::V4(v4) = addr else {
        return Err("not publishing: our address is not IPv4".into());
    };
    if !public_ipv4(*v4.ip()) {
        return Err(format!(
            "not publishing: our public address {} is not routable on the internet (set a public address with set-public-addr)",
            v4.ip()
        ));
    }
    let o = me.virtual_ip.octets();
    if o[..3] != NETWORK_PREFIX || o[3] == 0 || o[3] == 255 {
        return Err(format!("not publishing: virtual IP {} is outside the mesh subnet", me.virtual_ip));
    }
    Ok(PeerRecord {
        name: sanitize_name(&me.name),
        virtual_ip: me.virtual_ip,
        public_ip: *v4.ip(),
        public_port: v4.port(),
        updated_at: now,
    })
}

/// Maps a display name onto the directory's name rules (1-32 characters, first
/// alphanumeric, then alphanumeric, '_' or '-'). Anything else becomes '-'.
pub fn sanitize_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let trimmed = mapped.trim_start_matches(['-', '_']);
    let cut: String = trimmed.chars().take(32).collect();
    if cut.is_empty() {
        "peer".to_string()
    } else {
        cut
    }
}

/// GitHub storage is switched off for now. The adapter stays in the tree and its
/// tests still run, but no configuration or invite can use it until this is set
/// back to true.
pub const GITHUB_STORAGE_ENABLED: bool = false;

fn build_backends(settings: &DirectorySettings) -> Result<Vec<Box<dyn Backend>>, String> {
    let mut backends: Vec<Box<dyn Backend>> = Vec::new();
    if let Some(g) = &settings.github {
        if !GITHUB_STORAGE_ENABLED {
            return Err(
                "GitHub storage is turned off in this release; use MantleDB instead: run `meow-meow directory clear`, then `meow-meow directory mantle <namespace>`".into(),
            );
        }
        backends.push(Box::new(GithubBackend::new(&g.repo, g.branch.as_deref(), &g.path, &g.token)?));
    }
    if let Some(m) = &settings.mantle {
        backends.push(Box::new(MantleBackend::new(&m.namespace, m.write_key.as_deref())?));
    }
    if backends.is_empty() {
        return Err("no directory service configured (use `meow-meow directory github|mantle`)".into());
    }
    Ok(backends)
}

/// Checks that every configured service is well-formed, without touching the network.
pub fn validate_settings(settings: &DirectorySettings) -> Result<(), String> {
    build_backends(settings).map(|_| ())
}

/// Encodes settings as a single pasteable line for `directory join`. It contains
/// the GitHub token and the MantleDB write key, so treat it like the network key.
pub fn encode_invite(settings: &DirectorySettings) -> Result<String, String> {
    validate_settings(settings)?;
    let json = serde_json::to_vec(settings).map_err(|e| e.to_string())?;
    Ok(format!(
        "{INVITE_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    ))
}

pub fn decode_invite(text: &str) -> Result<DirectorySettings, String> {
    let payload = text
        .trim()
        .strip_prefix(INVITE_PREFIX)
        .ok_or_else(|| format!("not a directory invite (expected it to start with {INVITE_PREFIX})"))?;
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| format!("invite is damaged: {e}"))?;
    let settings: DirectorySettings =
        serde_json::from_slice(&json).map_err(|e| format!("invite is damaged: {e}"))?;
    validate_settings(&settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests;
