use std::cell::RefCell;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::backends::PeerRecord;

/// A network stand-in. `claim` returns `key`; `members` returns `members`.
struct Fake {
    key: Result<String, String>,
    members: Result<Vec<PeerRecord>, String>,
    claimed: RefCell<Vec<String>>,
    reads: RefCell<u32>,
}

impl Fake {
    fn ok(key: &str) -> Self {
        Fake {
            key: Ok(key.to_string()),
            members: Ok(Vec::new()),
            claimed: RefCell::new(Vec::new()),
            reads: RefCell::new(0),
        }
    }
    fn with_members(mut self, members: Vec<PeerRecord>) -> Self {
        self.members = Ok(members);
        self
    }
    fn unreachable(mut self) -> Self {
        self.members = Err("no response".into());
        self
    }
    fn failing_claim(mut self) -> Self {
        self.key = Err("HTTP 503: service error".into());
        self
    }
}

impl Network for Fake {
    fn claim(&self, namespace: &str) -> Result<String, String> {
        self.claimed.borrow_mut().push(namespace.to_string());
        self.key.clone()
    }
    fn members(&self, _: &DirectorySettings, _: &str) -> Result<Vec<PeerRecord>, String> {
        *self.reads.borrow_mut() += 1;
        self.members.clone()
    }
}

fn member(last: u8, name: &str) -> PeerRecord {
    PeerRecord {
        name: name.into(),
        virtual_ip: Ipv4Addr::new(10, 66, 0, last),
        public_ip: Ipv4Addr::new(8, 8, 8, last),
        public_port: 54321,
        updated_at: 1,
    }
}

/// A fresh, empty folder for one test.
fn scratch(tag: &str) -> Location {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir: PathBuf = std::env::temp_dir().join(format!("faeknet-setup-{tag}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    Location { config: dir.join("mesh.toml"), directory: dir.join("directory.toml") }
}

fn read_config(loc: &Location) -> Config {
    toml::from_str(&fs::read_to_string(&loc.config).unwrap()).unwrap()
}

fn read_directory(loc: &Location) -> DirectorySettings {
    toml::from_str(&fs::read_to_string(&loc.directory).unwrap()).unwrap()
}

fn no_leftovers(loc: &Location) -> bool {
    !loc.config.exists() && !loc.directory.exists() && !tmp_path(&loc.config).exists() && !tmp_path(&loc.directory).exists()
}

#[test]
fn create_writes_both_files_and_the_invite_carries_the_claimed_key() {
    let loc = scratch("create");
    let net = Fake::ok("writekey-abcdef12");
    let invite = create_network(&net, &loc, "alice", None).unwrap();

    let cfg = read_config(&loc);
    assert_eq!(cfg.me.name, "alice");
    assert_eq!(cfg.me.virtual_ip, Ipv4Addr::new(10, 66, 0, 1));
    let dir = read_directory(&loc);
    let ns = namespace_for_psk(&cfg.me.psk).unwrap();
    assert_eq!(dir.mantle.as_ref().unwrap().namespace, ns);
    assert_eq!(dir.mantle.as_ref().unwrap().write_key.as_deref(), Some("writekey-abcdef12"));
    assert_eq!(*net.claimed.borrow(), vec![ns.clone()]);

    let (psk, invited) = invite::decode(&invite).unwrap();
    assert_eq!(psk, cfg.me.psk);
    assert_eq!(invited.mantle.unwrap().write_key.as_deref(), Some("writekey-abcdef12"));
    assert!(!tmp_path(&loc.config).exists() && !tmp_path(&loc.directory).exists());
}

#[cfg(unix)]
#[test]
fn directory_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let loc = scratch("mode");
    create_network(&Fake::ok("writekey-abcdef12"), &loc, "alice", None).unwrap();
    let mode = fs::metadata(&loc.directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_failed_claim_saves_nothing_and_says_so() {
    let loc = scratch("failed-claim");
    let net = Fake::ok("unused").failing_claim();
    let err = create_network(&net, &loc, "alice", None).unwrap_err();
    assert!(err.contains("Nothing was saved"), "{err}");
    assert!(no_leftovers(&loc));
    assert_eq!(net.claimed.borrow().len(), 1, "one claim was attempted, and it failed");
}

#[test]
fn an_unwritable_folder_is_detected_before_anything_is_claimed() {
    let loc = Location {
        config: PathBuf::from("/definitely/not/a/real/folder/mesh.toml"),
        directory: PathBuf::from("/definitely/not/a/real/folder/directory.toml"),
    };
    let net = Fake::ok("writekey-abcdef12");
    let err = create_network(&net, &loc, "alice", None).unwrap_err();
    assert!(err.contains("Nothing was claimed or saved"), "{err}");
    assert!(net.claimed.borrow().is_empty(), "must not claim when files cannot be written");
}

#[test]
fn an_existing_network_is_never_overwritten() {
    let loc = scratch("existing");
    create_network(&Fake::ok("writekey-abcdef12"), &loc, "alice", None).unwrap();
    let before = fs::read_to_string(&loc.config).unwrap();
    let net = Fake::ok("writekey-zzzzzzzz");
    let err = create_network(&net, &loc, "mallory", None).unwrap_err();
    assert!(err.contains("already has a network"), "{err}");
    assert!(net.claimed.borrow().is_empty());
    assert_eq!(fs::read_to_string(&loc.config).unwrap(), before);
}

#[test]
fn join_picks_the_first_free_address_and_saves_everything() {
    let host = scratch("host");
    let invite = create_network(&Fake::ok("writekey-abcdef12"), &host, "alice", None).unwrap();

    let loc = scratch("joiner");
    let net = Fake::ok("unused").with_members(vec![member(1, "alice"), member(2, "bob")]);
    let cfg = join_network(&net, &loc, &invite, "carol", None).unwrap();
    assert_eq!(cfg.me.virtual_ip, Ipv4Addr::new(10, 66, 0, 3));
    assert_eq!(read_config(&loc).me.psk, read_config(&host).me.psk);
    assert!(read_directory(&loc).mantle.unwrap().write_key.is_some());
    assert_eq!(*net.reads.borrow(), 1);
    assert!(!tmp_path(&loc.config).exists() && !tmp_path(&loc.directory).exists());
}

#[test]
fn join_refuses_a_taken_address_and_suggests_a_free_one() {
    let host = scratch("host2");
    let invite = create_network(&Fake::ok("writekey-abcdef12"), &host, "alice", None).unwrap();
    let loc = scratch("joiner2");
    let net = Fake::ok("unused").with_members(vec![member(2, "alice")]);
    let err = join_network(&net, &loc, &invite, "bob", Some(Ipv4Addr::new(10, 66, 0, 2))).unwrap_err();
    assert!(err.contains("10.66.0.2") && err.contains("10.66.0.3"), "{err}");
    assert!(no_leftovers(&loc));
}

#[test]
fn join_saves_nothing_when_the_directory_cannot_be_read() {
    let host = scratch("host3");
    let invite = create_network(&Fake::ok("writekey-abcdef12"), &host, "alice", None).unwrap();
    let loc = scratch("joiner3");
    let net = Fake::ok("unused").unreachable();
    let err = join_network(&net, &loc, &invite, "bob", None).unwrap_err();
    assert!(err.contains("Nothing was saved"), "{err}");
    assert!(no_leftovers(&loc));
}

#[test]
fn an_empty_network_can_be_joined() {
    let host = scratch("host4");
    let invite = create_network(&Fake::ok("writekey-abcdef12"), &host, "alice", None).unwrap();
    let loc = scratch("joiner4");
    let cfg = join_network(&Fake::ok("unused"), &loc, &invite, "bob", None).unwrap();
    assert_eq!(cfg.me.virtual_ip, Ipv4Addr::new(10, 66, 0, 2));
}

#[test]
fn a_damaged_invite_never_reaches_the_network() {
    let loc = scratch("bad-invite");
    let net = Fake::ok("unused");
    assert!(join_network(&net, &loc, "FNNET1:garbage!!", "bob", None).is_err());
    assert!(join_network(&net, &loc, "not an invite", "bob", None).is_err());
    assert_eq!(*net.reads.borrow(), 0);
    assert!(no_leftovers(&loc));
}

#[test]
fn virtual_ips_outside_the_subnet_are_refused() {
    let host = scratch("host5");
    let invite = create_network(&Fake::ok("writekey-abcdef12"), &host, "alice", None).unwrap();
    let loc = scratch("joiner5");
    for bad in [Ipv4Addr::new(10, 66, 0, 0), Ipv4Addr::new(10, 66, 0, 255), Ipv4Addr::new(192, 168, 1, 5)] {
        assert!(join_network(&Fake::ok("unused"), &loc, &invite, "bob", Some(bad)).is_err(), "{bad}");
    }
    assert!(create_network(&Fake::ok("k"), &scratch("bad-create"), "alice", Some(Ipv4Addr::new(10, 66, 0, 255))).is_err());
}

#[test]
fn a_blank_name_is_refused() {
    let loc = scratch("blank");
    assert!(create_network(&Fake::ok("k"), &loc, "   ", None).is_err());
    assert!(no_leftovers(&loc));
}

#[test]
fn namespace_is_stable_per_key_and_differs_between_keys() {
    let a = Cipher::generate_psk_b64();
    let b = Cipher::generate_psk_b64();
    let ns = namespace_for_psk(&a).unwrap();
    assert_eq!(ns, namespace_for_psk(&a).unwrap());
    assert_ne!(ns, namespace_for_psk(&b).unwrap());
    assert!(ns.starts_with("faeknet-") && ns.len() == "faeknet-".len() + 16);
    assert!(namespace_for_psk("nope").is_err());
}

#[test]
fn suggest_free_ip_skips_taken_addresses() {
    let taken = vec![member(2, "a"), member(3, "b"), member(5, "c")];
    assert_eq!(suggest_free_ip(&taken), Ipv4Addr::new(10, 66, 0, 4));
    assert_eq!(suggest_free_ip(&[]), Ipv4Addr::new(10, 66, 0, 2));
}

/// Real MantleDB. Creates one permanent namespace on the public service, so it
/// runs only on request: `cargo test -p meow-meow_core setup -- --ignored`.
#[test]
#[ignore = "touches the public MantleDB service and claims a permanent namespace"]
fn live_create_then_join_over_mantledb() {
    let host = scratch("live-host");
    let invite = create_network(&Live, &host, "live-host", None).unwrap();
    let loc = scratch("live-join");
    let cfg = join_network(&Live, &loc, &invite, "live-join", None).unwrap();
    assert_eq!(cfg.me.virtual_ip, Ipv4Addr::new(10, 66, 0, 2));
}
