//! Simulates several members sharing one directory. Each member has its own
//! client, and all clients read and write the same store, the way two machines
//! would share a MantleDB entry or a GitHub file.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use super::*;
use crate::backends::{next_document, seal, Backend, DirectoryKey, MemoryBackend, PeerRecord};
use crate::config::{DirectorySettings, GithubSettings, MantleSettings};
use crate::crypto::Cipher;

const T0: u64 = 1_790_000_000;

/// One shared blob, cloned into every member's backend list.
#[derive(Clone, Default)]
struct SharedStore(Arc<Mutex<Option<Vec<u8>>>>);

impl Backend for SharedStore {
    fn name(&self) -> String {
        "shared".into()
    }
    fn fetch(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}

fn member(last: u8, name: &str) -> Member {
    Member { name: name.into(), virtual_ip: Ipv4Addr::new(10, 66, 0, last) }
}

fn addr(a: u8, port: u16) -> SocketAddr {
    SocketAddr::from(([8, 8, 8, a], port))
}

/// A client for one member, sharing `store` and `key` with the other members.
fn client(store: &SharedStore, key: &DirectoryKey) -> DirectoryClient {
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(store.clone())];
    DirectoryClient::with_backends(backends, key.clone())
}

fn key() -> DirectoryKey {
    DirectoryKey::from_psk_b64(&Cipher::generate_psk_b64()).unwrap()
}

fn find(members: &[PeerRecord], vip: u8) -> Option<PeerRecord> {
    members.iter().find(|p| p.virtual_ip == Ipv4Addr::new(10, 66, 0, vip)).cloned()
}

#[test]
fn a_member_learns_another_members_endpoint_from_the_directory() {
    let (store, k) = (SharedStore::default(), key());
    let (mut alice, mut bob) = (client(&store, &k), client(&store, &k));
    alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0);
    let seen = bob.sync(&member(3, "bob"), Some(addr(3, 40000)), T0 + 1);
    let alice_view = find(&seen.members, 2).expect("bob sees alice");
    assert_eq!(alice_view.public_port, 54321);
    assert_eq!(alice_view.name, "alice");
}

/// The headline case: a member's public IP or port changes, and everyone else
/// picks up the new endpoint on their next sync, with no manual step.
#[test]
fn an_endpoint_change_propagates_to_every_other_member() {
    let (store, k) = (SharedStore::default(), key());
    let (mut alice, mut bob, mut carol) = (client(&store, &k), client(&store, &k), client(&store, &k));
    alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0);
    bob.sync(&member(3, "bob"), Some(addr(3, 40000)), T0);
    carol.sync(&member(4, "carol"), Some(addr(4, 50000)), T0);

    // Alice's ISP reassigns her address: new IP and a new NAT port.
    let moved = alice.sync(&member(2, "alice"), Some(addr(9, 61000)), T0 + 60);
    assert!(moved.published, "a changed endpoint must be published immediately");

    let bob_view = bob.sync(&member(3, "bob"), Some(addr(3, 40000)), T0 + 61);
    let carol_view = carol.sync(&member(4, "carol"), Some(addr(4, 50000)), T0 + 61);
    for (who, view) in [("bob", bob_view), ("carol", carol_view)] {
        let alice_view = find(&view.members, 2).expect("sees alice");
        assert_eq!(alice_view.public_ip, Ipv4Addr::new(8, 8, 8, 9), "{who} sees the new IP");
        assert_eq!(alice_view.public_port, 61000, "{who} sees the new port");
    }
}

#[test]
fn an_unchanged_endpoint_is_not_republished_until_the_heartbeat() {
    let (store, k) = (SharedStore::default(), key());
    let mut alice = client(&store, &k);
    let me = member(2, "alice");
    assert!(alice.sync(&me, Some(addr(2, 54321)), T0).published);
    assert!(!alice.sync(&me, Some(addr(2, 54321)), T0 + 60).published);
    assert!(!alice.sync(&me, Some(addr(2, 54321)), T0 + HEARTBEAT_SECS - 1).published);
    assert!(alice.sync(&me, Some(addr(2, 54321)), T0 + HEARTBEAT_SECS).published);
}

#[test]
fn members_not_refreshed_for_a_day_are_dropped() {
    let (store, k) = (SharedStore::default(), key());
    let mut alice = client(&store, &k);
    let mut bob = client(&store, &k);
    alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0);
    let fresh = bob.sync(&member(3, "bob"), None, T0 + STALE_AFTER_SECS - 1);
    assert!(find(&fresh.members, 2).is_some());
    let stale = bob.sync(&member(3, "bob"), None, T0 + STALE_AFTER_SECS + 1);
    assert!(find(&stale.members, 2).is_none());
}

#[test]
fn a_private_address_is_never_published_because_receivers_would_reject_the_whole_directory() {
    let (store, k) = (SharedStore::default(), key());
    let mut alice = client(&store, &k);
    let mut bob = client(&store, &k);
    let me = member(2, "alice");
    let private = SocketAddr::from(([192, 168, 1, 20], 54321));
    let result = alice.sync(&me, Some(private), T0);
    assert!(!result.published);
    assert!(result.notes.iter().any(|n| n.contains("not routable")), "{:?}", result.notes);
    // Bob must still be able to read the directory: nothing invalid was written.
    assert!(bob.sync(&member(3, "bob"), None, T0 + 1).notes.is_empty());
}

#[test]
fn records_outside_the_mesh_subnet_are_not_published() {
    let (store, k) = (SharedStore::default(), key());
    let mut alice = client(&store, &k);
    let outside = Member { name: "alice".into(), virtual_ip: Ipv4Addr::new(10, 99, 0, 2) };
    let result = alice.sync(&outside, Some(addr(2, 54321)), T0);
    assert!(!result.published);
    assert!(result.notes.iter().any(|n| n.contains("outside the mesh subnet")));
}

#[test]
fn display_names_are_mapped_onto_the_directory_rules() {
    assert_eq!(sanitize_name("alice"), "alice");
    assert_eq!(sanitize_name("my laptop!"), "my-laptop-");
    assert_eq!(sanitize_name("  --__x"), "x");
    assert_eq!(sanitize_name("!!!"), "peer");
    assert_eq!(sanitize_name(""), "peer");
    assert_eq!(sanitize_name(&"a".repeat(80)).len(), 32);
}

/// A signed directory with a lower sequence number than one we already accepted
/// must not roll our view back.
#[test]
fn a_replayed_older_directory_does_not_roll_the_view_back() {
    let (store, k) = (SharedStore::default(), key());
    let mut alice = client(&store, &k);
    let mut bob = client(&store, &k);
    // Seq 1: alice at her first endpoint. Bob accepts it.
    alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0);
    bob.sync(&member(3, "bob"), None, T0 + 1);
    // Seq 2: alice moved. Bob accepts that too, so his floor is now 2.
    alice.sync(&member(2, "alice"), Some(addr(2, 55555)), T0 + 5);
    let moved = bob.sync(&member(3, "bob"), None, T0 + 6);
    assert_eq!(find(&moved.members, 2).unwrap().public_port, 55555);

    // An attacker with write access to the store replays the seq 1 document.
    let replay = seal(&k, &next_document(None, vec![PeerRecord {
        name: "alice".into(),
        virtual_ip: Ipv4Addr::new(10, 66, 0, 2),
        public_ip: Ipv4Addr::new(8, 8, 8, 1),
        public_port: 1111,
        updated_at: T0,
    }], T0)).unwrap();
    *store.0.lock().unwrap() = Some(replay);
    let view = bob.sync(&member(3, "bob"), None, T0 + 7);
    assert_eq!(find(&view.members, 2).unwrap().public_port, 55555, "kept the newer endpoint");
    assert!(view.notes.iter().any(|n| n.contains("stale")), "{:?}", view.notes);
}

#[test]
fn forged_directories_are_ignored() {
    let store = SharedStore::default();
    let real_key = key();
    let mut bob = client(&store, &real_key);
    let attacker_key = key();
    let forged = seal(&attacker_key, &next_document(None, vec![PeerRecord {
        name: "mallory".into(),
        virtual_ip: Ipv4Addr::new(10, 66, 0, 9),
        public_ip: Ipv4Addr::new(8, 8, 8, 9),
        public_port: 9999,
        updated_at: T0,
    }], T0)).unwrap();
    *store.0.lock().unwrap() = Some(forged);
    let view = bob.sync(&member(3, "bob"), None, T0);
    assert!(view.members.is_empty());
    assert!(view.notes.iter().any(|n| n.contains("does not match")), "{:?}", view.notes);
}

#[test]
fn one_failing_service_does_not_block_publishing_to_the_others() {
    let k = key();
    let store = SharedStore::default();
    let backends: Vec<Box<dyn Backend>> = vec![
        Box::new(MemoryBackend::new("down").offline()),
        Box::new(store.clone()),
    ];
    let mut alice = DirectoryClient::with_backends(backends, k.clone());
    let result = alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0);
    assert!(result.published);
    assert!(result.notes.iter().any(|n| n.contains("down")), "{:?}", result.notes);
    let mut bob = client(&store, &k);
    assert!(find(&bob.sync(&member(3, "bob"), None, T0 + 1).members, 2).is_some());
}

#[test]
fn invites_roundtrip_and_reject_damage() {
    let settings = DirectorySettings {
        github: None,
        mantle: Some(MantleSettings { namespace: "faeknet-abc123".into(), write_key: None }),
    };
    let invite = encode_invite(&settings).unwrap();
    assert!(invite.starts_with("FNDIR1:"));
    let back = decode_invite(&invite).unwrap();
    assert_eq!(back.mantle.unwrap().namespace, "faeknet-abc123");

    // GitHub storage is off for now, so an invite carrying a GitHub token is refused
    // and never produced, which keeps the token out of shared invite lines.
    let with_github = DirectorySettings {
        github: Some(GithubSettings {
            repo: "owner/faeknet-dir".into(),
            branch: None,
            path: "faeknet-directory.json".into(),
            token: "github_pat_0123456789abcdefghij".into(),
        }),
        mantle: None,
    };
    assert!(encode_invite(&with_github).is_err());

    assert!(decode_invite("LMESH1:whatever").is_err());
    assert!(decode_invite("FNDIR1:!!!").is_err());
    assert!(decode_invite("FNDIR1:e30").is_err(), "no services configured");
    assert!(encode_invite(&DirectorySettings::default()).is_err());
}

#[test]
fn debug_output_never_shows_secrets() {
    let g = GithubSettings {
        repo: "o/r".into(),
        branch: None,
        path: "faeknet-directory.json".into(),
        token: "github_pat_SECRETSECRETSECRETSECRET".into(),
    };
    let m = MantleSettings { namespace: "ns".into(), write_key: Some("mantlekey-SECRETSECRET".into()) };
    let text = format!("{g:?} {m:?}");
    assert!(!text.contains("SECRET"), "{text}");
    assert!(text.contains("<redacted>"));
}

/// End to end against the real MantleDB service, with two members on separate
/// clients: A joins, B learns A, A's endpoint changes, and B sees the new one.
/// Uses a fresh unclaimed namespace, which the service purges after 30 idle days.
/// Run with `cargo test -p meow-meow_core backends::client -- --ignored`.
#[test]
#[ignore = "touches the public MantleDB service"]
fn live_endpoint_change_reaches_another_member_over_mantledb() {
    use crate::backends::mantle::MantleBackend;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let ns = format!("faeknet-live-{nanos}");
    let k = key();
    let store = |ns: &str| -> Vec<Box<dyn Backend>> { vec![Box::new(MantleBackend::new(ns, None).unwrap())] };
    let mut alice = DirectoryClient::with_backends(store(&ns), k.clone());
    let mut bob = DirectoryClient::with_backends(store(&ns), k.clone());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    let first = alice.sync(&member(2, "alice"), Some(addr(2, 54321)), now);
    assert!(first.published, "{:?}", first.notes);
    let seen = bob.sync(&member(3, "bob"), None, now);
    assert_eq!(find(&seen.members, 2).expect("bob learns alice").public_port, 54321, "{:?}", seen.notes);

    // Alice's address changes. Bob's next sync must pick up the new endpoint.
    let moved = alice.sync(&member(2, "alice"), Some(addr(9, 61000)), now + 60);
    assert!(moved.published, "{:?}", moved.notes);
    let seen = bob.sync(&member(3, "bob"), None, now + 61);
    let alice_now = find(&seen.members, 2).expect("alice still listed");
    assert_eq!(alice_now.public_ip, Ipv4Addr::new(8, 8, 8, 9));
    assert_eq!(alice_now.public_port, 61000);
}

/// A store that can be taken offline, to simulate a service outage.
#[derive(Clone)]
struct Outage {
    inner: SharedStore,
    up: Arc<Mutex<bool>>,
}

impl Backend for Outage {
    fn name(&self) -> String {
        "outage".into()
    }
    fn fetch(&self) -> Result<Option<Vec<u8>>, String> {
        if *self.up.lock().unwrap() {
            self.inner.fetch()
        } else {
            Err("service unreachable".into())
        }
    }
    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        if *self.up.lock().unwrap() {
            self.inner.publish(bytes)
        } else {
            Err("service unreachable".into())
        }
    }
}

fn stored_seq_and_peers(store: &SharedStore, k: &DirectoryKey) -> (u64, usize) {
    let bytes = store.fetch().unwrap().expect("a directory is stored");
    let doc = crate::backends::open(k, &bytes, T0 + 1000).expect("stored directory verifies");
    (doc.seq, doc.peers.len())
}

/// A member that starts during an outage must not replace the shared directory
/// with a list containing only itself.
#[test]
fn a_member_starting_during_an_outage_does_not_overwrite_the_directory() {
    let (store, k) = (SharedStore::default(), key());
    let mut bob = client(&store, &k);
    bob.sync(&member(3, "bob"), Some(addr(3, 40000)), T0);
    let before = stored_seq_and_peers(&store, &k);

    let up = Arc::new(Mutex::new(false));
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(Outage { inner: store.clone(), up })];
    let mut alice = DirectoryClient::with_backends(backends, k.clone());
    let result = alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0 + 1);

    assert!(!result.published);
    assert!(result.notes.iter().any(|n| n.contains("no directory service answered")), "{:?}", result.notes);
    assert_eq!(stored_seq_and_peers(&store, &k), before);
}

/// Same guard mid-session: once we know the directory, an outage must not make
/// us publish a version without the other members.
#[test]
fn a_member_does_not_publish_a_partial_directory_when_the_service_drops_out() {
    let (store, k) = (SharedStore::default(), key());
    let mut bob = client(&store, &k);
    bob.sync(&member(3, "bob"), Some(addr(3, 40000)), T0);

    let up = Arc::new(Mutex::new(true));
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(Outage { inner: store.clone(), up: up.clone() })];
    let mut alice = DirectoryClient::with_backends(backends, k.clone());
    let first = alice.sync(&member(2, "alice"), Some(addr(2, 54321)), T0 + 1);
    assert!(first.published);
    let seen_by_alice = first.members.len();
    assert_eq!(seen_by_alice, 1, "alice sees bob");

    // Outage, then alice's endpoint changes. She must wait, not publish.
    *up.lock().unwrap() = false;
    let during = alice.sync(&member(2, "alice"), Some(addr(2, 60000)), T0 + 120);
    assert!(!during.published);

    // Service returns: the change goes out, with bob still in the list.
    *up.lock().unwrap() = true;
    let after = alice.sync(&member(2, "alice"), Some(addr(2, 60000)), T0 + 180);
    assert!(after.published);
    assert_eq!(stored_seq_and_peers(&store, &k), (3, 2));
}
