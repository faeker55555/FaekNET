//! Coexistence with always-on VPN clients -- currently Cloudflare WARP.
//!
//! Background: `warp_compat` (see mesh.rs) pins the mesh's UDP socket to the
//! real internet-facing NIC, so mesh traffic isn't dragged into the VPN's
//! virtual adapter. That fixes *routing* -- but Cloudflare's Linux client
//! (warp-svc) also installs a firewall-level anti-leak rule: any packet that
//! leaves via the real NIC toward a destination the tunnel would have carried
//! (i.e. anything outside WARP's default split-tunnel exclusions -- RFC1918
//! LAN ranges, link-local, multicast, ...) is silently DROPPED unless it
//! carries WARP's own tunnel fwmark. The pinned socket's packets don't carry
//! that mark, so with WARP connected every mesh datagram to a peer's public
//! endpoint vanished on send -- "can't connect to anyone" with zero errors in
//! the mesh's own logs -- while LAN-destined traffic (gateway pings,
//! same-subnet peers) kept working, which is exactly what makes the failure
//! so confusing to diagnose.
//!
//! The supported, non-disabling remedy is WARP's consumer split-tunnel
//! exclude list (`warp-cli tunnel ip add <addr>`): destinations on that list
//! bypass both the tunnel AND the anti-leak drop, for every application on
//! the machine, and WARP stays fully on for everything else. This module
//! detects a connected WARP client and idempotently ensures that list covers
//! every public endpoint the mesh currently talks to: configured peers,
//! gossip-discovered/roamed live endpoints (refreshed periodically, since
//! peers can appear or roam after startup), and our own manual public
//! address (so same-WAN peers can hairpin through the router). It only ever
//! ADDS single-host (/32) entries -- never removes or reconfigures anything
//! else -- and every action is logged through the normal mesh log sink.
//! Opt out with `warp_split_tunnel_auto = false` under `[me]` in mesh.toml.
//!
//! On Windows, warp-cli's split-tunnel management is Zero-Trust/MDM territory
//! and the client is usually not on PATH, so this module is a no-op there
//! (Windows WARP doesn't drop same-host pinned traffic the way the Linux
//! client's nftables anti-leak does).

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use crate::mesh::log;

/// warp-cli binary we invoke. On Linux the client package puts it on PATH.
#[cfg(target_os = "linux")]
const WARP_CLI: &str = "warp-cli";

/// Hard cap on a single warp-cli invocation. warp-cli normally answers in
/// well under a second, but it talks to warp-svc over IPC, and when the
/// service is busy/restarting an invocation can sit there for many seconds.
/// The GUI calls into this module from near its startup path, and a caller
/// thread blocked that long is exactly what makes desktop environments
/// (GNOME/Wayland) pop the "application is not responding" prompt -- so
/// every invocation is bounded and the child killed on expiry.
const CLI_TIMEOUT: Duration = Duration::from_secs(4);

/// Where the Windows client traditionally installs warp-cli.exe. Only used
/// for the connectivity *detection* -- the actual exclusion management is
/// deliberately not attempted on Windows (see module docs).
#[cfg(target_os = "windows")]
const WARP_CLI: &str = r"C:\Program Files\Cloudflare\Cloudflare WARP\warp-cli.exe";

/// Which direction the client's split-tunnel configuration faces. The
/// consumer client is exclude-mode by default ("tunnel everything except
/// this list"), where adding our peers to the list is exactly right. An
/// include-mode (usually Zero-Trust managed) client only lets *listed*
/// ranges through the tunnel at all, so blindly adding entries there would
/// send mesh traffic INTO the tunnel -- the opposite of what we want -- so
/// we refuse and tell the user instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitTunnelMode {
    Excluded,
    Included,
}

/// Ensures WARP split-tunnel exclusions cover `targets`, gated on `enabled`
/// and on a connected WARP client actually being present. No-op on
/// non-Linux (see module docs for why Windows is left alone).
pub(crate) fn maybe_ensure_exclusions(enabled: bool, targets: &[Ipv4Addr]) {
    #[cfg(target_os = "linux")]
    if enabled && is_warp_connected() {
        ensure_split_tunnel_exclusions(targets);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (enabled, targets);
    }
}

/// True if `warp-cli status` reports a connected WARP tunnel. Also false in
/// every "can't tell" case (client not installed, not on PATH, command
/// failed) -- callers must treat this module as strictly best-effort.
pub fn is_warp_connected() -> bool {
    let Some(out) = run_cli(&["status"]) else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // `warp-cli status` prints "Status update: Connected" when the tunnel is
    // up; anything else (Disconnected, Connecting, ...) means we shouldn't
    // touch the split-tunnel config.
    text.contains("Connected")
}

/// Parses which direction the split-tunnel list faces, from the first line
/// of `warp-cli tunnel ip list` output ("Excluded routes:" /
/// "Included routes:"). Returns None for unrecognizable output.
fn parse_list_mode(output: &str) -> Option<SplitTunnelMode> {
    let first = output.lines().next()?;
    let lower = first.to_lowercase();
    if lower.contains("excluded") {
        Some(SplitTunnelMode::Excluded)
    } else if lower.contains("included") {
        Some(SplitTunnelMode::Included)
    } else {
        None
    }
}

/// Extracts the IPv4 addresses already present in `warp-cli tunnel ip list`
/// output. Lines look like `  10.0.0.0/8` or `  155.133.224.0/19 (CLI
/// exclude)`; IPv6 entries are skipped (the mesh is IPv4-only).
fn parse_listed_ips(output: &str) -> Vec<Ipv4Addr> {
    let mut found = Vec::new();
    for line in output.lines() {
        for token in line.split_whitespace() {
            // strip an optional /prefix
            let ip_part = token.split('/').next().unwrap_or(token);
            if let Ok(ip) = ip_part.parse::<Ipv4Addr>() {
                found.push(ip);
            }
        }
    }
    found
}

/// Filter + dedup for exclusion candidates: mesh traffic only ever needs an
/// exclusion if its destination is one WARP's anti-leak rule would drop, i.e.
/// anything that is NOT already covered by WARP's default excludes (LAN
/// ranges, link-local, loopback, multicast, ...). Peers configured with
/// private/LAN addresses (or the same-router LAN fallback candidates) are
/// therefore deliberately skipped -- WARP doesn't touch those anyway, and
/// stuffing the user's split-tunnel list with LAN noise would be rude.
fn sanitize_targets<I: IntoIterator<Item = Ipv4Addr>>(candidates: I) -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for ip in candidates {
        if !needs_exclusion(ip) || out.contains(&ip) {
            continue;
        }
        out.push(ip);
    }
    out
}

/// Would WARP's default split-tunnel configuration (which already exempts
/// RFC1918, loopback, link-local, multicast, CGNAT 100.64/10 and reserved
/// ranges) drop traffic to this address when the tunnel is up?
fn needs_exclusion(ip: Ipv4Addr) -> bool {
    if ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || ip.is_multicast()
    {
        return false;
    }
    let o = ip.octets();
    // 100.64.0.0/10 (CGNAT / shared address space) is in WARP's default
    // excludes, and 240.0.0.0/4 (reserved, incl. 255.255.255.255) too.
    if o[0] == 100 && (o[1] & 0xC0) == 64 {
        return false;
    }
    if o[0] >= 240 {
        return false;
    }
    true
}

/// Ensures every address in `targets` that needs one has a WARP split-tunnel
/// exclusion. Idempotent: already-listed addresses are skipped silently (no
/// log spam on the periodic re-check), newly added ones and any failures are
/// logged. Never removes entries. Best-effort throughout -- any error just
/// logs a line with the manual equivalent and returns.
pub fn ensure_split_tunnel_exclusions(targets: &[Ipv4Addr]) {
    let targets = sanitize_targets(targets.iter().copied());
    if targets.is_empty() {
        return;
    }

    let Some(list_out) = run_cli(&["tunnel", "ip", "list"]) else {
        log("Cloudflare WARP detected, but 'warp-cli' could not be executed -- \
cannot manage split-tunnel exclusions automatically. If mesh traffic is being \
dropped while WARP is connected, run for each peer's public IP manually: \
warp-cli tunnel ip add <ip>");
        return;
    };
    if !list_out.status.success() {
        log(&format!(
            "Cloudflare WARP detected, but reading its split-tunnel list failed \
(exit {:?}) -- skipping automatic exclusions.",
            list_out.status.code()
        ));
        return;
    }
    let list_text = String::from_utf8_lossy(&list_out.stdout).to_string();
    match parse_list_mode(&list_text) {
        Some(SplitTunnelMode::Excluded) => {} // the mode we can help in
        Some(SplitTunnelMode::Included) => {
            log("Cloudflare WARP detected, but its split-tunnel list is include-mode \
(usually Zero-Trust managed) -- skipping automatic exclusions. Mesh traffic to \
peers must be allowed by your organization's split-tunnel policy.");
            return;
        }
        None => {
            log("Cloudflare WARP detected, but its split-tunnel list output was \
unrecognized -- skipping automatic exclusions.");
            return;
        }
    }

    let listed = parse_listed_ips(&list_text);
    let mut added = 0usize;
    let mut failed = 0usize;
    for ip in targets {
        if listed.contains(&ip) {
            continue;
        }
        match run_cli(&["tunnel", "ip", "add", &ip.to_string()]) {
            Some(out) if out.status.success() => {
                added += 1;
                log(&format!(
                    "WARP split-tunnel: added exclusion for {ip}/32 -- mesh traffic \
to this address now bypasses the WARP tunnel (and its anti-leak drop). \
WARP itself stays connected."
                ));
            }
            other => {
                failed += 1;
                let why = match other {
                    Some(out) => format!("exit status {:?}", out.status.code()),
                    None => "warp-cli not executable".to_string(),
                };
                log(&format!(
                    "WARP split-tunnel: could not exclude {ip} ({why}). If mesh traffic \
to this peer is dropped while WARP is connected, run manually: \
warp-cli tunnel ip add {ip}"
                ));
            }
        }
    }
    if added > 0 || failed > 0 {
        log(&format!(
            "WARP split-tunnel sync finished: {added} added, {failed} failed, {} already present.",
            listed.len()
        ));
    }
}

#[cfg(target_os = "linux")]
fn make_cli(args: &[&str]) -> Option<std::process::Command> {
    let mut cmd = std::process::Command::new(WARP_CLI);
    cmd.args(args);
    Some(cmd)
}

#[cfg(target_os = "windows")]
fn make_cli(args: &[&str]) -> Option<std::process::Command> {
    if std::path::Path::new(WARP_CLI).exists() {
        let mut cmd = std::process::Command::new(WARP_CLI);
        cmd.args(args);
        Some(cmd)
    } else {
        None
    }
}

/// Runs one warp-cli command with a hard timeout (see `CLI_TIMEOUT`).
/// Returns None if it couldn't spawn or didn't finish in time (the hung
/// child is killed so it can't pile up). warp-cli's output is tiny, so
/// draining the pipes only after the process has exited is safe.
fn run_cli(args: &[&str]) -> Option<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = make_cli(args)?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                // Timed out (or wait errored): kill and give up.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_end(&mut stdout);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_end(&mut stderr);
    }
    Some(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn list_mode_parsing() {
        let excluded = "Excluded routes:\n  10.0.0.0/8\n  5.44.174.16/32 (CLI exclude)\n";
        assert_eq!(parse_list_mode(excluded), Some(SplitTunnelMode::Excluded));
        let included = "Included routes:\n  0.0.0.0/0\n";
        assert_eq!(parse_list_mode(included), Some(SplitTunnelMode::Included));
        assert_eq!(parse_list_mode("garbage"), None);
        assert_eq!(parse_list_mode(""), None);
    }

    #[test]
    fn listed_ip_parsing_handles_prefixes_annotatations_and_v6() {
        let out = "Excluded routes:\n  10.0.0.0/8\n  155.133.224.0/19 (CLI exclude)\n  5.44.174.16/32\n  fe80::/10\n";
        let ips = parse_listed_ips(out);
        assert!(ips.contains(&ip("10.0.0.0")));
        assert!(ips.contains(&ip("155.133.224.0")));
        assert!(ips.contains(&ip("5.44.174.16")));
        // v6 must be skipped, not crash or mis-parse
        assert!(!ips.iter().any(|i| i.is_multicast()));
        assert_eq!(ips.iter().filter(|i| **i != ip("5.44.174.16")).count(), 2);
    }

    #[test]
    fn sanitization_skips_lan_and_dedups() {
        let targets = vec![
            ip("5.44.174.16"),
            ip("5.44.174.16"),          // duplicate
            ip("192.168.1.20"),         // LAN peer -> WARP doesn't touch it
            ip("10.66.0.2"),            // virtual ip -> skip
            ip("127.0.0.1"),            // loopback
            ip("169.254.1.1"),          // link-local
            ip("100.100.1.1"),          // CGNAT range (default-excluded)
            ip("88.201.174.26"),
        ];
        let out = sanitize_targets(targets);
        assert_eq!(out, vec![ip("5.44.174.16"), ip("88.201.174.26")]);
    }

    #[test]
    fn needs_exclusion_matches_warp_default_excludes() {
        assert!(needs_exclusion(ip("5.44.174.16")));
        assert!(needs_exclusion(ip("146.158.102.129")));
        assert!(!needs_exclusion(ip("192.168.1.254")));
        assert!(!needs_exclusion(ip("224.0.0.251")));
        assert!(!needs_exclusion(ip("255.255.255.255")));
    }
}
