//! Optional built-in local DNS resolver for mesh domain names.
//!
//! Binds a UDP socket on 127.0.0.1:`dns_port` and answers standard DNS
//! queries: anything under the configured `domain_suffix` (e.g.
//! `alice.mesh`) is answered directly from the live peer table (always
//! up to date, no caching lag), and everything else is forwarded
//! upstream to a real resolver and the response relayed back unmodified
//! -- so pointing the OS at 127.0.0.1 as its DNS server doesn't break
//! normal internet browsing.
//!
//! Two kinds of names are served, both without any caching lag since the
//! table is rebuilt from the live peer/service state on every change:
//! - **Exact names** -- a peer's root name (`alice.mesh`) or one of its
//!   advertised services (`game.alice.mesh`) -- matched case-
//!   insensitively against the flat table `hosts.rs` builds.
//! - **Wildcard subdomains of a peer root** -- *any* name ending in
//!   `.<peer-root>` that isn't already a registered service (e.g.
//!   `whatever.alice.mesh`, `dev.alice.mesh`) resolves to that peer's
//!   virtual IP too. This is what makes subdomains genuinely open-ended
//!   rather than requiring every service to be pre-registered: a peer
//!   can point application-level virtual-hosting (e.g. an nginx
//!   server_name block, or a game server that inspects SNI/Host headers)
//!   at any subdomain of their own name without touching mesh
//!   config at all. A *registered* service name still wins over the
//!   wildcard fallback if both would match, since it's more specific.
//!
//! This is the heavier-weight alternative to `hosts.rs`'s hosts-file
//! sync (which can't do wildcards -- a hosts file only ever maps exact
//! names) and doesn't require rewriting a system file, but it depends on
//! the OS/network stack actually being pointed at this resolver, which
//! some platforms (especially Windows, and especially with a VPN/
//! always-on-DNS client like WARP active) don't always respect for every
//! process. `dns_auto_configure` makes a best-effort attempt to point
//! the OS at it; it's not guaranteed to stick in every environment,
//! which is why hosts-file sync remains the default and this stays
//! opt-in.
//!
//! Deliberately implements only the minimal wire-format subset needed:
//! single-question A-record queries, iterative label parsing (including
//! compression pointers when relaying upstream responses raw), and a
//! straight pass-through forwarder for anything not ours to answer.

use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

const RECV_TIMEOUT: Duration = Duration::from_millis(500);
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(2);
/// Fallback public resolvers used when forwarding non-mesh queries
/// upstream, tried in order until one answers. Deliberately not
/// configurable from a file the mesh doesn't otherwise touch -- keeping
/// this a fixed, well-known list avoids yet another thing that can be
/// silently misconfigured.
const UPSTREAM_SERVERS: &[&str] = &["1.1.1.1:53", "8.8.8.8:53"];

/// One resolvable name in the live DNS table.
#[derive(Debug, Clone)]
pub struct DnsEntry {
    pub hostname: String,
    pub virtual_ip: Ipv4Addr,
    /// True for a peer's own root name -- these additionally act as a
    /// wildcard base, so any subdomain of them that isn't itself a
    /// registered entry still resolves to the same IP. False for a
    /// specific registered service name, which only ever matches
    /// exactly.
    pub is_peer_root: bool,
}

/// Live table the DNS thread reads from every query -- kept as a plain
/// `RwLock<Vec<..>>` rather than reaching into `mesh::MeshState` directly,
/// so this module has no dependency on `mesh.rs` and could in principle
/// be reused/tested standalone.
pub type DnsTable = Arc<RwLock<Vec<DnsEntry>>>;

pub fn new_table() -> DnsTable {
    Arc::new(RwLock::new(Vec::new()))
}

pub fn update_table(table: &DnsTable, entries: Vec<DnsEntry>) {
    *table.write().unwrap() = entries;
}

pub struct DnsHandle {
    running: Arc<AtomicBool>,
}

impl DnsHandle {
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Starts the resolver thread. Binding failure (e.g. port 53 already
/// owned by systemd-resolved or another resolver) is returned as an Err
/// rather than panicking -- the mesh itself works fine without it, this
/// is purely an optional convenience layer.
pub fn start(bind_port: u16, table: DnsTable) -> std::io::Result<DnsHandle> {
    let addr: SocketAddr = ([127, 0, 0, 1], bind_port).into();
    let socket = UdpSocket::bind(addr)?;
    socket.set_read_timeout(Some(RECV_TIMEOUT))?;

    let running = Arc::new(AtomicBool::new(true));
    let running_thread = running.clone();

    thread::spawn(move || {
        let mut buf = [0u8; 512];
        crate::logsink::emit(&format!(
            "Local DNS resolver listening on 127.0.0.1:{bind_port}"
        ));
        loop {
            if !running_thread.load(Ordering::Relaxed) {
                break;
            }
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => continue,
                    _ => continue,
                },
            };
            let query = &buf[..n];
            match handle_query(query, &table) {
                QueryOutcome::Answered(response) => {
                    let _ = socket.send_to(&response, from);
                }
                QueryOutcome::Forward => {
                    if let Some(response) = forward_upstream(query) {
                        let _ = socket.send_to(&response, from);
                    }
                }
                QueryOutcome::Malformed => {}
            }
        }
        crate::logsink::emit("Local DNS resolver stopped.");
    });

    Ok(DnsHandle { running })
}

enum QueryOutcome {
    Answered(Vec<u8>),
    Forward,
    Malformed,
}

/// Parses the (single) question out of a DNS query packet, per RFC 1035
/// section 4.1: 12-byte header, then a sequence of length-prefixed
/// labels terminated by a zero-length label, then QTYPE (2 bytes) and
/// QCLASS (2 bytes).
fn parse_question(query: &[u8]) -> Option<(String, u16, u16)> {
    if query.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount == 0 {
        return None;
    }
    let mut pos = 12;
    let mut labels = Vec::new();
    loop {
        let len = *query.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 != 0 {
            return None; // compression pointer in a query is unusual; not handled
        }
        pos += 1;
        let label = query.get(pos..pos + len)?;
        labels.push(String::from_utf8_lossy(label).to_string());
        pos += len;
    }
    let qtype = u16::from_be_bytes([*query.get(pos)?, *query.get(pos + 1)?]);
    let qclass = u16::from_be_bytes([*query.get(pos + 2)?, *query.get(pos + 3)?]);
    Some((labels.join("."), qtype, qclass))
}

const QTYPE_A: u16 = 1;
const QCLASS_IN: u16 = 1;

fn handle_query(query: &[u8], table: &DnsTable) -> QueryOutcome {
    let Some((name, qtype, qclass)) = parse_question(query) else {
        return QueryOutcome::Malformed;
    };
    if qtype != QTYPE_A || qclass != QCLASS_IN {
        // Anything other than a plain A-record lookup (AAAA, MX, TXT,
        // browser-probe HTTPS/SVCB records, etc.) is forwarded upstream
        // untouched rather than answered/rejected here -- this resolver
        // only ever *adds* mesh names, it doesn't try to replace a real
        // resolver's behavior for record types it doesn't understand.
        return QueryOutcome::Forward;
    }

    let lower = name.to_ascii_lowercase();
    match resolve(&lower, table) {
        Some(ip) => QueryOutcome::Answered(build_a_response(query, ip)),
        None => QueryOutcome::Forward,
    }
}

/// Resolves one lowercased query name against the live table: an exact
/// match (peer root or registered service) always wins; failing that,
/// falls back to wildcard matching against every peer root's `.<root>`
/// suffix, so an arbitrary never-registered subdomain of a peer's own
/// name (`whatever.alice.mesh`) still resolves to that peer.
fn resolve(lower_name: &str, table: &DnsTable) -> Option<Ipv4Addr> {
    let guard = table.read().unwrap();

    if let Some(entry) = guard.iter().find(|e| e.hostname.eq_ignore_ascii_case(lower_name)) {
        return Some(entry.virtual_ip);
    }

    guard
        .iter()
        .filter(|e| e.is_peer_root)
        .find(|e| lower_name.ends_with(&format!(".{}", e.hostname.to_ascii_lowercase())))
        .map(|e| e.virtual_ip)
}

/// Builds a minimal well-formed DNS response for a single A-record
/// question: copies the original header (fixing up flags/counts) and
/// question section verbatim, then appends one answer resource record
/// pointing back at the question name via a compression pointer to byte
/// offset 12 (where the question's name starts) rather than repeating
/// the name -- both to keep the response tiny and because that's the
/// standard, universally-supported way to do it.
fn build_a_response(query: &[u8], ip: Ipv4Addr) -> Vec<u8> {
    let mut resp = Vec::with_capacity(query.len() + 16);

    // Header: id copied from query, flags set to "standard query
    // response, no error", 1 question, 1 answer, 0 authority/additional.
    resp.extend_from_slice(&query[0..2]); // ID
    resp.push(0x81); // QR=1, Opcode=0, AA=0, TC=0, RD=1 (mirror requester's RD, but we don't parse it -- 1 is a safe default recursion-desired echo)
    resp.push(0x80); // RA=1, Z=0, RCODE=0 (no error)
    resp.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    resp.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
    resp.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    resp.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

    // Question section, copied verbatim from the original query (name +
    // qtype + qclass) so the client's own validation of "does the
    // response echo my question" passes trivially.
    let question_end = find_question_end(query).unwrap_or(query.len());
    resp.extend_from_slice(&query[12..question_end]);

    // Answer: name = pointer to offset 12, TYPE=A, CLASS=IN, TTL=30s
    // (short on purpose -- this is a live table that can change the
    // moment a peer's address roams, so nothing should cache it long),
    // RDLENGTH=4, RDATA=the IPv4 address.
    resp.extend_from_slice(&[0xC0, 0x0C]);
    resp.extend_from_slice(&QTYPE_A.to_be_bytes());
    resp.extend_from_slice(&QCLASS_IN.to_be_bytes());
    resp.extend_from_slice(&30u32.to_be_bytes());
    resp.extend_from_slice(&4u16.to_be_bytes());
    resp.extend_from_slice(&ip.octets());

    resp
}

fn find_question_end(query: &[u8]) -> Option<usize> {
    let mut pos = 12;
    loop {
        let len = *query.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        pos += len;
    }
    Some(pos + 4) // + QTYPE(2) + QCLASS(2)
}

/// Relays a query we don't own to a real upstream resolver and returns
/// its raw response bytes unmodified, so anything the mesh doesn't know
/// about (regular internet hostnames) keeps working exactly as if this
/// resolver wasn't in the path at all.
fn forward_upstream(query: &[u8]) -> Option<Vec<u8>> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    sock.set_read_timeout(Some(UPSTREAM_TIMEOUT)).ok()?;
    for server in UPSTREAM_SERVERS {
        let Ok(mut addrs) = server.to_socket_addrs() else {
            continue;
        };
        let Some(addr) = addrs.next() else { continue };
        if sock.send_to(query, addr).is_err() {
            continue;
        }
        let mut buf = [0u8; 4096];
        if let Ok((n, _)) = sock.recv_from(&mut buf) {
            return Some(buf[..n].to_vec());
        }
    }
    None
}

/// Best-effort attempt to point this machine's own DNS resolution at the
/// built-in resolver, so mesh domain names resolve system-wide without
/// each application needing to be told about 127.0.0.1 individually.
///
/// This is inherently the least reliable part of the domain-name feature
/// -- modern OSes have several layers of DNS configuration (NetworkManager,
/// systemd-resolved, per-adapter settings, VPN clients that reassert their
/// own DNS server on every reconnect...) and any of them can silently
/// override what this function sets. It's opt-in (`dns_auto_configure`,
/// off by default) for exactly that reason: hosts-file sync remains the
/// dependable default, and this is offered as a "try it, and if it
/// doesn't stick on your system, you can always add 127.0.0.1 as a DNS
/// server by hand" convenience rather than something the mesh depends on
/// working.
#[cfg(target_os = "linux")]
pub fn try_auto_configure_system(dns_port: u16) -> Result<(), String> {
    if dns_port != 53 {
        return Err(format!(
            "dns_auto_configure only works with dns_port = 53 (got {dns_port}); \
             the OS resolver protocol doesn't support a custom port."
        ));
    }
    let resolv_conf = "/etc/resolv.conf";
    let backup = "/etc/resolv.conf.meow-meow_backup";
    let existing = std::fs::read_to_string(resolv_conf).unwrap_or_default();
    if existing.lines().next().map(str::trim) == Some("nameserver 127.0.0.1") {
        return Ok(()); // already applied, e.g. from a previous run
    }
    if !std::path::Path::new(backup).exists() {
        std::fs::write(backup, &existing).map_err(|e| format!("could not back up {resolv_conf}: {e}"))?;
    }
    let mut new_content = String::from("nameserver 127.0.0.1\n");
    new_content.push_str(&existing);
    std::fs::write(resolv_conf, new_content).map_err(|e| {
        format!(
            "could not write {resolv_conf} ({e}) -- if this machine uses \
             NetworkManager or systemd-resolved, {resolv_conf} may be a \
             symlink managed elsewhere; point its DNS settings at \
             127.0.0.1 manually instead."
        )
    })?;
    crate::logsink::emit(
        "Pointed /etc/resolv.conf at the built-in mesh DNS resolver \
         (127.0.0.1). Note: NetworkManager/systemd-resolved may overwrite \
         this on the next network change -- if mesh domain names stop \
         resolving after reconnecting Wi-Fi/Ethernet, re-run the mesh or \
         configure your resolver manually.",
    );
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn try_undo_auto_configure() {
    let backup = "/etc/resolv.conf.meow-meow_backup";
    if let Ok(original) = std::fs::read_to_string(backup) {
        let _ = std::fs::write("/etc/resolv.conf", original);
        let _ = std::fs::remove_file(backup);
    }
}

/// Windows equivalent: sets the given interface's DNS server to
/// 127.0.0.1 via `netsh`. `interface_name` should be the real
/// physical/internet-facing adapter (the same one `mesh::get_real_interface`
/// already picks for socket binding, deliberately skipping VPN/WARP-style
/// virtual adapters) -- setting DNS on the mesh's own virtual TUN adapter
/// would do nothing useful, since that's not what applications resolve
/// through.
#[cfg(target_os = "windows")]
pub fn try_auto_configure_system(interface_name: &str, dns_port: u16) -> Result<(), String> {
    if dns_port != 53 {
        return Err(format!(
            "dns_auto_configure only works with dns_port = 53 (got {dns_port}); \
             Windows' resolver client doesn't support a custom port."
        ));
    }
    let status = std::process::Command::new("netsh")
        .args([
            "interface",
            "ip",
            "set",
            "dns",
            &format!("name={interface_name}"),
            "static",
            "127.0.0.1",
            "primary",
        ])
        .status()
        .map_err(|e| format!("could not invoke netsh: {e}"))?;
    if !status.success() {
        return Err(format!(
            "netsh exited with {status} -- this usually means the mesh \
             isn't running as Administrator (required to change DNS \
             settings)."
        ));
    }
    crate::logsink::emit(&format!(
        "Pointed interface '{interface_name}' at the built-in mesh DNS \
         resolver (127.0.0.1). Note: some VPN clients (e.g. Cloudflare \
         WARP) reassert their own DNS server on every reconnect and may \
         override this."
    ));
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn try_undo_auto_configure(interface_name: &str) {
    let _ = std::process::Command::new("netsh")
        .args([
            "interface",
            "ip",
            "set",
            "dns",
            &format!("name={interface_name}"),
            "dhcp",
        ])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&QCLASS_IN.to_be_bytes());
        q
    }

    #[test]
    fn parses_simple_question() {
        let q = build_query("alice.mesh", QTYPE_A);
        let (name, qtype, qclass) = parse_question(&q).unwrap();
        assert_eq!(name, "alice.mesh");
        assert_eq!(qtype, QTYPE_A);
        assert_eq!(qclass, QCLASS_IN);
    }

    fn peer_root(hostname: &str, ip: Ipv4Addr) -> DnsEntry {
        DnsEntry {
            hostname: hostname.to_string(),
            virtual_ip: ip,
            is_peer_root: true,
        }
    }

    fn service_entry(hostname: &str, ip: Ipv4Addr) -> DnsEntry {
        DnsEntry {
            hostname: hostname.to_string(),
            virtual_ip: ip,
            is_peer_root: false,
        }
    }

    #[test]
    fn answers_known_mesh_name() {
        let table = new_table();
        update_table(&table, vec![peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2))]);
        let q = build_query("alice.mesh", QTYPE_A);
        match handle_query(&q, &table) {
            QueryOutcome::Answered(resp) => {
                assert_eq!(&resp[0..2], &q[0..2]); // ID echoed
                assert_eq!(&resp[resp.len() - 4..], &[10, 66, 0, 2]);
            }
            _ => panic!("expected an answer"),
        }
    }

    #[test]
    fn forwards_unknown_names() {
        let table = new_table();
        update_table(&table, vec![peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2))]);
        let q = build_query("example.com", QTYPE_A);
        assert!(matches!(handle_query(&q, &table), QueryOutcome::Forward));
    }

    #[test]
    fn forwards_non_a_queries_even_for_known_names() {
        let table = new_table();
        update_table(&table, vec![peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2))]);
        const QTYPE_AAAA: u16 = 28;
        let q = build_query("alice.mesh", QTYPE_AAAA);
        assert!(matches!(handle_query(&q, &table), QueryOutcome::Forward));
    }

    #[test]
    fn case_insensitive_match() {
        let table = new_table();
        update_table(&table, vec![peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2))]);
        let q = build_query("ALICE.MESH", QTYPE_A);
        assert!(matches!(handle_query(&q, &table), QueryOutcome::Answered(_)));
    }

    #[test]
    fn wildcard_subdomain_of_peer_root_resolves() {
        let table = new_table();
        update_table(&table, vec![peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2))]);
        // Never explicitly registered as a service -- should still
        // resolve via the wildcard-of-peer-root fallback.
        let q = build_query("whatever.alice.mesh", QTYPE_A);
        match handle_query(&q, &table) {
            QueryOutcome::Answered(resp) => assert_eq!(&resp[resp.len() - 4..], &[10, 66, 0, 2]),
            _ => panic!("expected wildcard match to resolve"),
        }
        // Multi-level subdomains should also fall through to the peer.
        let q2 = build_query("deep.sub.alice.mesh", QTYPE_A);
        assert!(matches!(handle_query(&q2, &table), QueryOutcome::Answered(_)));
    }

    #[test]
    fn registered_service_wins_over_wildcard() {
        let table = new_table();
        update_table(
            &table,
            vec![
                peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2)),
                service_entry("game.alice.mesh", Ipv4Addr::new(10, 66, 0, 2)),
            ],
        );
        // Exact registered name matches directly (not just via wildcard).
        let q = build_query("game.alice.mesh", QTYPE_A);
        assert!(matches!(handle_query(&q, &table), QueryOutcome::Answered(_)));
    }

    #[test]
    fn deep_subdomain_of_a_service_still_resolves_via_peer_wildcard() {
        let table = new_table();
        update_table(
            &table,
            vec![
                peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2)),
                service_entry("game.alice.mesh", Ipv4Addr::new(10, 66, 0, 2)),
            ],
        );
        // "sub.game.alice.mesh" isn't itself registered, but it's still a
        // subdomain of the peer root "alice.mesh" (only peer roots, not
        // service names, act as wildcard bases -- but *any* depth of
        // subdomain under a peer root should fall through to it).
        let q = build_query("sub.game.alice.mesh", QTYPE_A);
        match handle_query(&q, &table) {
            QueryOutcome::Answered(resp) => assert_eq!(&resp[resp.len() - 4..], &[10, 66, 0, 2]),
            _ => panic!("expected deep subdomain of peer root to resolve"),
        }
    }

    #[test]
    fn subdomain_of_a_different_peers_service_name_does_not_leak() {
        let table = new_table();
        update_table(
            &table,
            vec![
                peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2)),
                peer_root("bob.mesh", Ipv4Addr::new(10, 66, 0, 3)),
                service_entry("game.bob.mesh", Ipv4Addr::new(10, 66, 0, 3)),
            ],
        );
        // A name that merely contains another peer's service label as a
        // substring, but isn't actually a subdomain of any peer root,
        // must not match anything.
        let q = build_query("game-bob-mesh.example.com", QTYPE_A);
        assert!(matches!(handle_query(&q, &table), QueryOutcome::Forward));
    }

    #[test]
    fn unrelated_suffix_does_not_falsely_match() {
        let table = new_table();
        update_table(&table, vec![peer_root("alice.mesh", Ipv4Addr::new(10, 66, 0, 2))]);
        // "evilalice.mesh" ends with "alice.mesh" as a raw string but NOT
        // as a proper ".alice.mesh" dotted suffix -- must not match.
        let q = build_query("evilalice.mesh", QTYPE_A);
        assert!(matches!(handle_query(&q, &table), QueryOutcome::Forward));
    }
}
