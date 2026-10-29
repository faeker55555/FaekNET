//! Tests run against an in-process mock of the MantleDB HTTP API, so they need
//! no network. One `#[ignore]`d test talks to the real service; run it with
//! `cargo test -p meow-meow_core backends::mantle -- --ignored`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backends::{fetch_latest, next_document, publish_all, Backend, DirectoryKey, PeerRecord, MAX_DOC_BYTES};
use crate::crypto::Cipher;

use super::*;

const KEY: &str = "test-key-0123456789";
const NS: &str = "faeknet-test";

#[derive(Default)]
struct MockState {
    entries: HashMap<String, Vec<u8>>,
    claimed: bool,
    requests: usize,
}

/// Starts a mock server on a random local port and returns its base URL.
fn spawn_mock() -> (String, Arc<Mutex<MockState>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(MockState::default()));
    let shared = state.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle(stream, &shared);
        }
    });
    (base, state)
}

fn handle(mut stream: TcpStream, state: &Mutex<MockState>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut key = None;
    let mut len = 0usize;
    let mut expects_continue = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "x-mantle-key" => key = Some(value.to_string()),
                "content-length" => len = value.parse().unwrap_or(0),
                "expect" => expects_continue = value.eq_ignore_ascii_case("100-continue"),
                _ => {}
            }
        }
    }
    if expects_continue {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();

    let (status, resp) = route(&method, &target, key.as_deref(), body, state);
    let head = format!(
        "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&resp);
}

fn route(method: &str, target: &str, key: Option<&str>, body: Vec<u8>, state: &Mutex<MockState>) -> (u16, Vec<u8>) {
    let path = target.split('?').next().unwrap_or("");
    let mut s = state.lock().unwrap();
    s.requests += 1;

    if let Some(ns) = path.strip_prefix("/v2/claim/") {
        if s.claimed {
            return (409, br#"{"error":"namespace already claimed"}"#.to_vec());
        }
        s.claimed = true;
        return (200, format!(r#"{{"success":true,"namespace":"{ns}","key":"{KEY}"}}"#).into_bytes());
    }

    let entry = path.trim_start_matches("/v2/").to_string();
    let authorized = key == Some(KEY);
    if s.claimed && !authorized {
        return (401, br#"{"error":"namespace key required"}"#.to_vec());
    }
    match method {
        "GET" => match s.entries.get(&entry) {
            Some(bytes) => (200, bytes.clone()),
            None => (404, br#"{"error":"Path not found at this namespace"}"#.to_vec()),
        },
        "POST" => {
            if body.len() > MAX_DOC_BYTES {
                return (413, br#"{"error":"entry exceeds 64 KB"}"#.to_vec());
            }
            s.entries.insert(entry, body);
            (200, br#"{"success":true}"#.to_vec())
        }
        _ => (405, b"{}".to_vec()),
    }
}

fn directory_key() -> DirectoryKey {
    DirectoryKey::from_psk_b64(&Cipher::generate_psk_b64()).unwrap()
}

fn peer(last: u8) -> PeerRecord {
    PeerRecord {
        name: format!("peer{last}"),
        virtual_ip: std::net::Ipv4Addr::new(10, 66, 0, last),
        public_ip: std::net::Ipv4Addr::new(8, 8, 4, last),
        public_port: 54321,
        updated_at: 1_790_000_000,
    }
}

fn now() -> u64 {
    1_790_000_000
}

#[test]
fn publish_then_fetch_roundtrips_through_the_directory_layer() {
    let (base, state) = spawn_mock();
    state.lock().unwrap().claimed = true;
    let backend = MantleBackend::new(NS, Some(KEY)).unwrap().with_base(&base).unwrap();
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(backend)];
    let key = directory_key();

    let doc = next_document(None, vec![peer(2), peer(3)], now());
    let report = publish_all(&backends, &key, &doc).unwrap();
    assert_eq!(report.succeeded.len(), 1);

    let fetched = fetch_latest(&backends, &key, 0, now());
    assert!(fetched.problems.is_empty(), "{:?}", fetched.problems);
    assert_eq!(fetched.best.unwrap(), doc);
}

#[test]
fn wrong_write_key_is_refused_with_a_clear_error() {
    let (base, state) = spawn_mock();
    state.lock().unwrap().claimed = true;
    let backend = MantleBackend::new(NS, Some("not-the-right-key-1")).unwrap().with_base(&base).unwrap();
    let err = backend.publish(b"{\"doc\":\"x\",\"mac\":\"y\"}").unwrap_err();
    assert!(err.contains("401") && err.contains("write key rejected"), "{err}");
}

#[test]
fn unclaimed_namespace_needs_no_key() {
    let (base, _state) = spawn_mock();
    let backend = MantleBackend::new(NS, None).unwrap().with_base(&base).unwrap();
    backend.publish(b"{\"doc\":\"x\",\"mac\":\"y\"}").unwrap();
    assert_eq!(backend.fetch().unwrap().unwrap(), b"{\"doc\":\"x\",\"mac\":\"y\"}");
}

#[test]
fn missing_entry_is_not_an_error() {
    let (base, _state) = spawn_mock();
    let backend = MantleBackend::new(NS, None).unwrap().with_base(&base).unwrap();
    assert_eq!(backend.fetch().unwrap(), None);
}

#[test]
fn oversize_directory_is_refused_before_any_request() {
    let (base, state) = spawn_mock();
    let backend = MantleBackend::new(NS, None).unwrap().with_base(&base).unwrap();
    let before = state.lock().unwrap().requests;
    assert!(backend.publish(&vec![b'a'; MAX_DOC_BYTES + 1]).is_err());
    assert_eq!(state.lock().unwrap().requests, before, "no request should be sent");
}

#[test]
fn claim_returns_the_key_once() {
    let (base, _state) = spawn_mock();
    let key = claim_at(&base, NS, None).unwrap();
    assert_eq!(key, KEY);
    let err = claim_at(&base, NS, None).unwrap_err();
    assert!(err.contains("409"), "{err}");
}

#[test]
fn claim_rejects_bad_email() {
    assert!(claim_at("https://mantledb.sh", NS, Some("a&b=c@x")).is_err());
}

#[test]
fn server_errors_are_mapped_to_readable_messages() {
    assert!(http_error(413, b"{}").contains("too large"));
    assert!(http_error(429, b"{}").contains("rate limited"));
    assert!(http_error(503, b"{}").contains("service error"));
}

#[test]
fn split_status_takes_the_last_line_as_the_code() {
    let (code, body) = split_status(b"{\"a\":1}\n200").unwrap();
    assert_eq!(code, 200);
    assert_eq!(body, b"{\"a\":1}");
    assert!(split_status(b"no newline here").is_err());
}

#[test]
fn inputs_that_could_escape_the_url_are_rejected() {
    assert!(MantleBackend::new("../etc", None).is_err());
    assert!(MantleBackend::new("", None).is_err());
    assert!(MantleBackend::new(NS, Some("short")).is_err());
    assert!(MantleBackend::new(NS, Some("abc\"\nheader = x")).is_err());
    let base = MantleBackend::new(NS, None).unwrap();
    assert!(base.with_base("http://evil.example").is_err());
    assert!(MantleBackend::new(NS, None).unwrap().with_path("../x").is_err());
    assert!(MantleBackend::new(NS, None).unwrap().with_path("/abs").is_err());
    assert!(MantleBackend::new(NS, None).unwrap().with_path("a//b").is_err());
    assert!(MantleBackend::new(NS, None).unwrap().with_path("faeknet/directory").is_ok());
}

/// Talks to the real MantleDB. Writes to a throwaway unclaimed namespace that
/// the service purges after 30 days without writes.
#[test]
#[ignore = "touches the public MantleDB service"]
fn live_mantledb_roundtrip() {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let ns = format!("faeknet-live-{nanos}");
    let backend = MantleBackend::new(&ns, None).unwrap();
    let key = directory_key();
    let doc = next_document(None, vec![peer(2)], now());
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(backend)];
    publish_all(&backends, &key, &doc).expect("publish to MantleDB");
    let fetched = fetch_latest(&backends, &key, 0, now());
    assert!(fetched.problems.is_empty(), "{:?}", fetched.problems);
    assert_eq!(fetched.best.unwrap(), doc);
}

#[test]
fn claim_accepts_201_created_and_never_echoes_a_key_on_failure() {
    let ok = parse_claim_response(201, br#"{"success":true,"key":"abcdefgh1234"}"#).unwrap();
    assert_eq!(ok, "abcdefgh1234");
    let err = parse_claim_response(200, br#"{"success":true,"secret":"abcdefgh1234"}"#).unwrap_err();
    assert!(!err.contains("abcdefgh1234"), "{err}");
    let err = parse_claim_response(200, b"not json abcdefgh1234").unwrap_err();
    assert!(!err.contains("abcdefgh1234"), "{err}");
    assert!(parse_claim_response(409, br#"{"error":"namespace already claimed"}"#).unwrap_err().contains("409"));
}

