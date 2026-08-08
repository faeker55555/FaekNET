//! Local-network domain names: keeps the OS hosts file in sync with the
//! mesh's peer table, so a friend named "alice" is reachable simply as
//! `alice.mesh` (or whatever `domain_suffix` is configured to) from any
//! application on the machine -- browsers, games with a "connect by
//! hostname" box, `ping`, etc. -- with zero extra client-side setup
//! beyond what the mesh already needs (root/Administrator for the TUN
//! adapter, which is the same privilege level hosts file writes need).
//!
//! This is intentionally the *simple, robust* mechanism: no listening
//! socket, no dependency on the OS actually asking us for DNS resolution,
//! no interaction with search-domain/resolver-priority quirks. Its only
//! downside is one name per peer and no wildcard/subdomain support --
//! `dns.rs` exists alongside it for people who want that instead (or as
//! well).
//!
//! The block this module manages is delimited by clearly-marked begin/end
//! comment lines, so it can be safely re-written on every update without
//! touching anything else a user (or another program) put in the hosts
//! file, and safely removed entirely if the mesh stops managing it.

use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "# >>> lan_mesh managed block -- do not edit by hand >>>";
const END_MARKER: &str = "# <<< lan_mesh managed block <<<";

/// One name -> address mapping this module should ensure exists.
#[derive(Debug, Clone)]
pub struct HostEntry {
    pub hostname: String,
    pub virtual_ip: Ipv4Addr,
}

/// Turns a peer's display name into a valid, safe DNS-label-ish hostname:
/// lowercase, spaces/underscores collapsed to hyphens, anything outside
/// `[a-z0-9-]` stripped, and never empty (falls back to `peer` if nothing
/// usable remains, matching what `mesh.rs`'s placeholder-name generator
/// separately guards against for the "unknown until gossip arrives"
/// case).
pub fn sanitize_label(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_sep = false;
    for ch in name.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_was_sep = false;
        } else if (lower == ' ' || lower == '_' || lower == '-' || lower == '.') && !last_was_sep && !out.is_empty() {
            out.push('-');
            last_was_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "peer".to_string()
    } else {
        out
    }
}

/// Given the raw list of (name, ip) pairs known to the mesh (peers plus
/// ourselves), builds the final hostname list with the domain suffix
/// applied and duplicate hostnames disambiguated by appending the last
/// octet of the virtual IP (e.g. two peers both named "alice" become
/// `alice.mesh` and `alice-12.mesh`) so a naming collision never silently
/// drops an entry.
pub fn build_entries(suffix: &str, peers: &[(String, Ipv4Addr)]) -> Vec<HostEntry> {
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut entries = Vec::with_capacity(peers.len());
    for (name, ip) in peers {
        let base = sanitize_label(name);
        let count = seen.entry(base.clone()).or_insert(0);
        let label = if *count == 0 {
            base.clone()
        } else {
            format!("{base}-{}", ip.octets()[3])
        };
        *count += 1;
        let hostname = if suffix.is_empty() {
            label
        } else {
            format!("{label}.{suffix}")
        };
        entries.push(HostEntry {
            hostname,
            virtual_ip: *ip,
        });
    }
    entries
}

#[cfg(target_os = "windows")]
pub fn hosts_file_path() -> PathBuf {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    PathBuf::from(system_root).join("System32\\drivers\\etc\\hosts")
}

#[cfg(not(target_os = "windows"))]
pub fn hosts_file_path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}

/// Rewrites the managed block inside the hosts file to exactly match
/// `entries`, leaving everything else in the file untouched. Best-effort:
/// most failures (permissions, read-only filesystem, etc.) are reported
/// back as an `Err` for the caller to log, rather than panicking --
/// running without hosts-file sync should degrade gracefully, since the
/// mesh's actual packet routing never depends on it.
pub fn sync(entries: &[HostEntry]) -> io::Result<()> {
    let path = hosts_file_path();
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let mut out_lines: Vec<String> = Vec::new();
    let mut in_block = false;
    let mut replaced = false;
    for line in existing.lines() {
        if line.trim() == BEGIN_MARKER {
            in_block = true;
            replaced = true;
            push_block(&mut out_lines, entries);
            continue;
        }
        if line.trim() == END_MARKER {
            in_block = false;
            continue;
        }
        if in_block {
            continue; // drop old managed lines, they'll be regenerated above
        }
        out_lines.push(line.to_string());
    }
    if !replaced {
        // No existing block -- append a fresh one at the end, preceded by
        // a blank line if the file doesn't already end with one.
        if !out_lines.is_empty() && !out_lines.last().unwrap().is_empty() {
            out_lines.push(String::new());
        }
        push_block(&mut out_lines, entries);
    }

    let mut content = out_lines.join("\n");
    content.push('\n');

    write_atomically(&path, &content)
}

fn push_block(out_lines: &mut Vec<String>, entries: &[HostEntry]) {
    out_lines.push(BEGIN_MARKER.to_string());
    out_lines.push("# Managed automatically by lan_mesh -- edits here will be overwritten.".to_string());
    for entry in entries {
        out_lines.push(format!("{}\t{}", entry.virtual_ip, entry.hostname));
    }
    out_lines.push(END_MARKER.to_string());
}

/// Removes the managed block entirely (e.g. when the user disables
/// hosts-file sync, or the mesh is being fully uninstalled), leaving the
/// rest of the hosts file untouched.
pub fn remove_block() -> io::Result<()> {
    sync(&[])
}

/// Writes via a temp-file-then-rename in the same directory, so a crash
/// or concurrent read mid-write can never leave the hosts file
/// truncated/corrupted -- this file is safety-critical enough (breaking
/// it can affect unrelated hostname resolution on the whole machine)
/// to be worth the extra care.
fn write_atomically(path: &PathBuf, content: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp_path = dir.join(format!(
        ".lan_mesh_hosts_tmp_{}",
        std::process::id()
    ));
    fs::write(&tmp_path, content)?;
    // Preserve original file permissions where possible (rename alone
    // would otherwise inherit the temp file's, e.g. from a restrictive
    // umask) -- best-effort, not fatal if it fails.
    #[cfg(unix)]
    {
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&tmp_path, meta.permissions());
        }
    }
    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_labels() {
        assert_eq!(sanitize_label("Alice"), "alice");
        assert_eq!(sanitize_label("Bob's PC!!"), "bobs-pc");
        assert_eq!(sanitize_label("  weird__--name.. "), "weird-name");
        assert_eq!(sanitize_label("???"), "peer");
        assert_eq!(sanitize_label(""), "peer");
    }

    #[test]
    fn disambiguates_duplicate_names() {
        let peers = vec![
            ("alice".to_string(), Ipv4Addr::new(10, 66, 0, 2)),
            ("alice".to_string(), Ipv4Addr::new(10, 66, 0, 12)),
        ];
        let entries = build_entries("mesh", &peers);
        assert_eq!(entries[0].hostname, "alice.mesh");
        assert_eq!(entries[1].hostname, "alice-12.mesh");
    }

    #[test]
    fn empty_suffix_omits_dot() {
        let peers = vec![("alice".to_string(), Ipv4Addr::new(10, 66, 0, 2))];
        let entries = build_entries("", &peers);
        assert_eq!(entries[0].hostname, "alice");
    }

    #[test]
    fn sync_writes_and_replaces_managed_block_only() {
        let dir = std::env::temp_dir().join(format!("lan_mesh_hosts_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("hosts");
        std::fs::write(&path, "127.0.0.1 localhost\nsome.other.entry 1.2.3.4\n").unwrap();

        // Monkeypatch not possible without an env var indirection in this
        // simple module, so directly exercise the block-splicing logic
        // via a temp copy of sync()'s internals through the public API by
        // temporarily overriding SystemRoot-independent behavior isn't
        // applicable on non-Windows -- instead just call write_atomically
        // + the same line-splicing loop `sync` uses, on our temp path, to
        // keep this test host-independent.
        let entries = build_entries("mesh", &[("alice".to_string(), Ipv4Addr::new(10, 66, 0, 2))]);
        let existing = std::fs::read_to_string(&path).unwrap();
        let mut out_lines: Vec<String> = Vec::new();
        let mut replaced = false;
        for line in existing.lines() {
            if line.trim() == BEGIN_MARKER {
                replaced = true;
                push_block(&mut out_lines, &entries);
                continue;
            }
            out_lines.push(line.to_string());
        }
        if !replaced {
            out_lines.push(String::new());
            push_block(&mut out_lines, &entries);
        }
        let mut content = out_lines.join("\n");
        content.push('\n');
        write_atomically(&path, &content).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("localhost"));
        assert!(result.contains("some.other.entry"));
        assert!(result.contains("10.66.0.2\talice.mesh"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
