//! Pluggable storage for the shared peer directory.
//!
//! The directory is a small signed document listing every member's current
//! public endpoint. It is published to one or more *backends* and read back
//! from them. A backend is anything that can store one opaque blob: a local
//! file, a free hosted JSON store, a file in a GitHub repository, a cloud
//! drive. This module knows nothing about any particular service. Each service
//! lives in its own adapter (a separate crate, or a module) that implements
//! [`Backend`].
//!
//! Trust model
//! -----------
//! The document is authenticated with HMAC-SHA256 under a key derived from the
//! shared network key (PSK). Only members can produce a directory that
//! verifies, so an outsider who can write to an open store can at best
//! *delete* or *withhold* a directory, not forge one. Clients take the valid
//! document with the highest `seq` and refuse anything below a floor they have
//! already accepted (anti-rollback).
//!
//! Failure model
//! -------------
//! Backends are independent. A backend that is down, read-only, or serving
//! garbage is recorded in the report and skipped; the rest still count.

pub mod client;
pub mod github;
pub mod mantle;

use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::PeerConfig;
use crate::registry::public_ipv4;

type HmacSha256 = Hmac<Sha256>;

pub const DOC_VERSION: u32 = 1;
/// Upper bound for a stored directory, shared by every backend. Free hosted
/// stores commonly cap items at 64 KB, so the format is designed to fit.
pub const MAX_DOC_BYTES: usize = 64 * 1024;
const MAX_PEERS: usize = 254;
const NETWORK_PREFIX: [u8; 3] = [10, 66, 0];
/// How far ahead of our clock a directory may claim to be issued.
const CLOCK_SKEW_SECS: u64 = 60;
const KEY_LABEL: &[u8] = b"faeknet-directory-key-v1";

/// One member's published endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRecord {
    pub name: String,
    pub virtual_ip: Ipv4Addr,
    pub public_ip: Ipv4Addr,
    pub public_port: u16,
    pub updated_at: u64,
}

/// The directory itself: the full current membership, versioned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub version: u32,
    /// Publication counter. Higher wins; ties are not allowed to win.
    pub seq: u64,
    /// Unix seconds when the publisher built this document.
    pub issued_at: u64,
    pub peers: Vec<PeerRecord>,
}

/// Stored form: `doc` is the exact JSON text that was authenticated, so the
/// MAC never depends on re-serialisation.
#[derive(Serialize, Deserialize)]
struct Envelope {
    doc: String,
    mac: String,
}

/// Authentication key for directory documents, derived from the PSK.
#[derive(Clone)]
pub struct DirectoryKey([u8; 32]);

impl DirectoryKey {
    pub fn from_psk_b64(psk_b64: &str) -> Result<Self, String> {
        let psk = base64::engine::general_purpose::STANDARD
            .decode(psk_b64.trim())
            .map_err(|e| format!("PSK is not valid base64: {e}"))?;
        if psk.len() != 32 {
            return Err(format!("PSK must decode to 32 bytes, got {}", psk.len()));
        }
        let mut kdf = HmacSha256::new_from_slice(&psk).expect("HMAC accepts any key length");
        kdf.update(KEY_LABEL);
        let mut key = [0u8; 32];
        key.copy_from_slice(&kdf.finalize().into_bytes());
        Ok(DirectoryKey(key))
    }

    fn tag(&self, data: &[u8]) -> Vec<u8> {
        let mut m = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts any key length");
        m.update(data);
        m.finalize().into_bytes().to_vec()
    }
}

fn valid_name(name: &str) -> bool {
    let len = name.chars().count();
    (1..=32).contains(&len)
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Document {
    /// Structural checks that apply to every document, whoever signed it.
    pub fn validate(&self, now: u64) -> Result<(), String> {
        if self.version != DOC_VERSION {
            return Err(format!("unsupported directory version {}", self.version));
        }
        if self.issued_at > now.saturating_add(CLOCK_SKEW_SECS) {
            return Err("directory is future-dated".into());
        }
        if self.peers.len() > MAX_PEERS {
            return Err("directory lists too many peers".into());
        }
        let mut seen = HashSet::new();
        for p in &self.peers {
            let o = p.virtual_ip.octets();
            if o[..3] != NETWORK_PREFIX || o[3] == 0 || o[3] == 255 {
                return Err(format!("{} is outside the mesh subnet", p.virtual_ip));
            }
            if !seen.insert(p.virtual_ip) {
                return Err(format!("duplicate virtual IP {}", p.virtual_ip));
            }
            if !public_ipv4(p.public_ip) {
                return Err(format!("{} is not a public address", p.public_ip));
            }
            if p.public_port == 0 {
                return Err(format!("peer {} has port 0", p.name));
            }
            if !valid_name(&p.name) {
                return Err(format!("invalid peer name {:?}", p.name));
            }
        }
        Ok(())
    }

    /// The membership as peer configs, ready for the mesh runtime.
    pub fn peer_configs(&self) -> Vec<PeerConfig> {
        self.peers
            .iter()
            .map(|p| PeerConfig {
                name: p.name.clone(),
                virtual_ip: p.virtual_ip,
                public_ip: p.public_ip.to_string(),
                public_port: p.public_port,
            })
            .collect()
    }
}

/// Builds the document to publish next: one above the newest version we know.
/// Callers should fetch first, merge their own change into the latest peer
/// list, then publish; a concurrent publisher can still win the race, and the
/// `seq` ordering makes that visible rather than silently lossy.
pub fn next_document(latest: Option<&Document>, peers: Vec<PeerRecord>, now: u64) -> Document {
    Document {
        version: DOC_VERSION,
        seq: latest.map_or(0, |d| d.seq).saturating_add(1),
        issued_at: now,
        peers,
    }
}

/// Serialises and authenticates a document into the bytes stored by backends.
pub fn seal(key: &DirectoryKey, doc: &Document) -> Result<Vec<u8>, String> {
    let text = serde_json::to_string(doc).map_err(|e| e.to_string())?;
    let mac = base64::engine::general_purpose::STANDARD.encode(key.tag(text.as_bytes()));
    let out = serde_json::to_vec(&Envelope { doc: text, mac }).map_err(|e| e.to_string())?;
    if out.len() > MAX_DOC_BYTES {
        return Err(format!("directory is {} bytes, limit is {MAX_DOC_BYTES}", out.len()));
    }
    Ok(out)
}

/// Verifies, parses and validates bytes read from a backend.
pub fn open(key: &DirectoryKey, bytes: &[u8], now: u64) -> Result<Document, String> {
    if bytes.len() > MAX_DOC_BYTES {
        return Err("stored directory exceeds size limit".into());
    }
    let env: Envelope = serde_json::from_slice(bytes).map_err(|e| format!("not a directory: {e}"))?;
    let mac = base64::engine::general_purpose::STANDARD
        .decode(&env.mac)
        .map_err(|_| "directory signature is malformed".to_string())?;
    let mut m = HmacSha256::new_from_slice(&key.0).expect("HMAC accepts any key length");
    m.update(env.doc.as_bytes());
    m.verify_slice(&mac)
        .map_err(|_| "signature does not match this network key".to_string())?;
    let doc: Document = serde_json::from_str(&env.doc).map_err(|e| format!("bad directory body: {e}"))?;
    doc.validate(now)?;
    Ok(doc)
}

/// Storage that holds one opaque directory blob.
///
/// Implementations must be safe to call from several threads. `fetch` returns
/// `Ok(None)` when nothing has been published yet, which is not an error.
pub trait Backend: Send + Sync {
    /// Stable label for logs and reports, for example `file:/path` or `mantle`.
    fn name(&self) -> String;
    /// Returns the stored bytes, or `None` if nothing has been published.
    fn fetch(&self) -> Result<Option<Vec<u8>>, String>;
    /// Replaces the stored bytes. Read-only backends return an error.
    fn publish(&self, bytes: &[u8]) -> Result<(), String>;
}

/// A plain file on disk. Useful on its own (a shared folder, a synced
/// directory) and as the reference implementation of the trait.
pub struct FileBackend {
    path: PathBuf,
}

impl FileBackend {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileBackend { path: path.into() }
    }
}

impl Backend for FileBackend {
    fn name(&self) -> String {
        format!("file:{}", self.path.display())
    }

    fn fetch(&self) -> Result<Option<Vec<u8>>, String> {
        match std::fs::metadata(&self.path) {
            Ok(meta) if meta.len() as usize > MAX_DOC_BYTES => Err("file exceeds size limit".into()),
            Ok(_) => std::fs::read(&self.path).map(Some).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_DOC_BYTES {
            return Err("directory exceeds size limit".into());
        }
        // Write beside the target and rename, so readers never see a torn file.
        let tmp = PathBuf::from(format!("{}.tmp", self.path.display()));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

/// In-process backend. Used as a test double and as a stand-in for
/// unreachable or read-only services.
pub struct MemoryBackend {
    label: String,
    data: Mutex<Option<Vec<u8>>>,
    read_only: bool,
    offline: bool,
}

impl MemoryBackend {
    pub fn new(label: impl Into<String>) -> Self {
        MemoryBackend { label: label.into(), data: Mutex::new(None), read_only: false, offline: false }
    }
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
    pub fn offline(mut self) -> Self {
        self.offline = true;
        self
    }
    pub fn with_contents(self, bytes: Vec<u8>) -> Self {
        *self.data.lock().unwrap() = Some(bytes);
        self
    }
}

impl Backend for MemoryBackend {
    fn name(&self) -> String {
        format!("memory:{}", self.label)
    }
    fn fetch(&self) -> Result<Option<Vec<u8>>, String> {
        if self.offline {
            return Err("service unreachable".into());
        }
        Ok(self.data.lock().unwrap().clone())
    }
    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        if self.offline {
            return Err("service unreachable".into());
        }
        if self.read_only {
            return Err("backend is read-only".into());
        }
        if bytes.len() > MAX_DOC_BYTES {
            return Err("directory exceeds size limit".into());
        }
        *self.data.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}

/// Result of reading every backend.
#[derive(Debug, Default)]
pub struct FetchReport {
    /// The accepted directory: valid, not below the floor, highest `seq`.
    pub best: Option<Document>,
    /// Name of the backend that supplied `best`.
    pub source: Option<String>,
    /// One line per backend that failed, was stale, or was rejected.
    pub problems: Vec<String>,
}

/// Reads every backend and returns the newest valid directory.
///
/// `floor_seq` is the highest `seq` the caller has already accepted. Anything
/// older is refused, which stops an attacker who can write to one store from
/// replaying an old, now-wrong membership list.
pub fn fetch_latest(backends: &[Box<dyn Backend>], key: &DirectoryKey, floor_seq: u64, now: u64) -> FetchReport {
    let mut report = FetchReport::default();
    for backend in backends {
        let name = backend.name();
        let bytes = match backend.fetch() {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(e) => {
                report.problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        let doc = match open(key, &bytes, now) {
            Ok(doc) => doc,
            Err(e) => {
                report.problems.push(format!("{name}: rejected: {e}"));
                continue;
            }
        };
        if doc.seq < floor_seq {
            report.problems.push(format!("{name}: stale (seq {} below floor {floor_seq})", doc.seq));
            continue;
        }
        let better = match &report.best {
            None => true,
            Some(current) => (doc.seq, doc.issued_at) > (current.seq, current.issued_at),
        };
        if better {
            report.best = Some(doc);
            report.source = Some(name);
        }
    }
    report
}

/// Result of writing to every backend.
#[derive(Debug, Default)]
pub struct PublishReport {
    pub succeeded: Vec<String>,
    pub failed: Vec<String>,
}

/// Writes the document to every backend. Succeeds if at least one accepted it,
/// so one flaky service never blocks an update that others can still carry.
pub fn publish_all(backends: &[Box<dyn Backend>], key: &DirectoryKey, doc: &Document) -> Result<PublishReport, String> {
    let bytes = seal(key, doc)?;
    let mut report = PublishReport::default();
    for backend in backends {
        let name = backend.name();
        match backend.publish(&bytes) {
            Ok(()) => report.succeeded.push(name),
            Err(e) => report.failed.push(format!("{name}: {e}")),
        }
    }
    if report.succeeded.is_empty() {
        return Err(format!("no backend accepted the directory: {}", report.failed.join("; ")));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Cipher;

    const NOW: u64 = 1_790_000_000;

    fn key() -> DirectoryKey {
        DirectoryKey::from_psk_b64(&Cipher::generate_psk_b64()).unwrap()
    }

    fn peer(last: u8, name: &str) -> PeerRecord {
        PeerRecord {
            name: name.into(),
            virtual_ip: Ipv4Addr::new(10, 66, 0, last),
            public_ip: Ipv4Addr::new(8, 8, 8, last),
            public_port: 54321,
            updated_at: NOW,
        }
    }

    fn doc(seq: u64, peers: Vec<PeerRecord>) -> Document {
        Document { version: DOC_VERSION, seq, issued_at: NOW, peers }
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let k = key();
        let d = doc(1, vec![peer(2, "alice"), peer(3, "bob")]);
        let bytes = seal(&k, &d).unwrap();
        assert_eq!(open(&k, &bytes, NOW).unwrap(), d);
    }

    #[test]
    fn wrong_network_key_is_rejected() {
        let bytes = seal(&key(), &doc(1, vec![peer(2, "alice")])).unwrap();
        let err = open(&key(), &bytes, NOW).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn tampered_body_is_rejected() {
        let k = key();
        let bytes = seal(&k, &doc(1, vec![peer(2, "alice")])).unwrap();
        // The envelope stores the body as an escaped JSON string, so the seq
        // appears as \"seq\":1 in the raw bytes.
        let raw = String::from_utf8(bytes).unwrap();
        let forged = raw.replacen("\\\"seq\\\":1", "\\\"seq\\\":9", 1);
        assert_ne!(forged, raw, "test setup: seq field not found");
        assert!(open(&k, forged.as_bytes(), NOW).is_err());
    }

    #[test]
    fn structurally_bad_documents_are_rejected_even_when_signed() {
        let k = key();
        let mut private_ip = peer(2, "alice");
        private_ip.public_ip = Ipv4Addr::new(192, 168, 1, 2);
        let mut dup = peer(3, "bob");
        dup.virtual_ip = Ipv4Addr::new(10, 66, 0, 2);
        let cases = vec![
            doc(1, vec![private_ip]),
            doc(1, vec![peer(2, "alice"), dup]),
            doc(1, vec![peer(2, "bad name")]),
            doc(1, vec![peer(0, "alice")]),
            doc(1, vec![{ let mut p = peer(2, "alice"); p.public_port = 0; p }]),
            Document { issued_at: NOW + 3600, ..doc(1, vec![peer(2, "alice")]) },
            Document { version: 9, ..doc(1, vec![peer(2, "alice")]) },
        ];
        for d in cases {
            let bytes = seal(&k, &d).unwrap();
            assert!(open(&k, &bytes, NOW).is_err(), "should reject {d:?}");
        }
    }

    #[test]
    fn key_from_psk_rejects_bad_material() {
        assert!(DirectoryKey::from_psk_b64("not base64!!").is_err());
        assert!(DirectoryKey::from_psk_b64(&base64::engine::general_purpose::STANDARD.encode([1u8; 16])).is_err());
    }

    #[test]
    fn highest_seq_wins_and_broken_backends_are_skipped() {
        let k = key();
        let backends: Vec<Box<dyn Backend>> = vec![
            Box::new(MemoryBackend::new("old").with_contents(seal(&k, &doc(3, vec![peer(2, "alice")])).unwrap())),
            Box::new(MemoryBackend::new("new").with_contents(seal(&k, &doc(5, vec![peer(2, "alice-new")])).unwrap())),
            Box::new(MemoryBackend::new("down").offline()),
            Box::new(MemoryBackend::new("forged").with_contents(seal(&key(), &doc(99, vec![peer(9, "mallory")])).unwrap())),
            Box::new(MemoryBackend::new("empty")),
        ];
        let report = fetch_latest(&backends, &k, 0, NOW);
        let best = report.best.expect("a valid directory exists");
        assert_eq!(best.seq, 5);
        assert_eq!(report.source.as_deref(), Some("memory:new"));
        assert_eq!(report.problems.len(), 2, "{:?}", report.problems);
        assert!(report.problems.iter().any(|p| p.contains("down")));
        assert!(report.problems.iter().any(|p| p.contains("forged") && p.contains("does not match")));
    }

    #[test]
    fn floor_blocks_rollback_to_an_older_directory() {
        let k = key();
        let backends: Vec<Box<dyn Backend>> =
            vec![Box::new(MemoryBackend::new("replayed").with_contents(seal(&k, &doc(2, vec![peer(2, "old-ip")])).unwrap()))];
        let report = fetch_latest(&backends, &k, 5, NOW);
        assert!(report.best.is_none());
        assert!(report.problems[0].contains("stale"));
    }

    #[test]
    fn publish_reaches_writable_backends_and_reports_the_rest() {
        let k = key();
        let backends: Vec<Box<dyn Backend>> = vec![
            Box::new(MemoryBackend::new("rw")),
            Box::new(MemoryBackend::new("ro").read_only()),
        ];
        let d = next_document(None, vec![peer(2, "alice")], NOW);
        let report = publish_all(&backends, &k, &d).unwrap();
        assert_eq!(report.succeeded, vec!["memory:rw".to_string()]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(fetch_latest(&backends, &k, 0, NOW).best.unwrap(), d);
    }

    #[test]
    fn publish_fails_only_when_nothing_accepts_it() {
        let backends: Vec<Box<dyn Backend>> = vec![Box::new(MemoryBackend::new("ro").read_only())];
        let d = next_document(None, vec![], NOW);
        assert!(publish_all(&backends, &key(), &d).is_err());
    }

    #[test]
    fn next_document_increments_seq() {
        let first = next_document(None, vec![], NOW);
        assert_eq!(first.seq, 1);
        assert_eq!(next_document(Some(&first), vec![], NOW + 5).seq, 2);
    }

    #[test]
    fn file_backend_roundtrip_and_missing_file() {
        let dir = std::env::temp_dir().join(format!("faeknet-backend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("directory.json");
        let _ = std::fs::remove_file(&path);
        let k = key();
        let backend = FileBackend::new(&path);
        assert!(backend.fetch().unwrap().is_none());
        let d = next_document(None, vec![peer(4, "carol")], NOW);
        backend.publish(&seal(&k, &d).unwrap()).unwrap();
        let backends: Vec<Box<dyn Backend>> = vec![Box::new(FileBackend::new(&path))];
        assert_eq!(fetch_latest(&backends, &k, 0, NOW).best.unwrap(), d);
        assert!(backend.publish(&vec![b'x'; MAX_DOC_BYTES + 1]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
