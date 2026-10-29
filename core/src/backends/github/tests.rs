//! Tests run against an in-process mock of the GitHub Contents API, so they need
//! no network. One `#[ignore]`d test talks to the real service; run it with
//! `cargo test -p meow-meow_core backends::github -- --ignored`
//! (set FAEKNET_GH_TEST_REPO and FAEKNET_GH_TEST_TOKEN, and use a throwaway branch).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use super::*;
use crate::backends::{fetch_latest, next_document, publish_all, DirectoryKey, PeerRecord, MAX_DOC_BYTES};
use crate::crypto::Cipher;

const TOKEN: &str = "github_pat_TESTTOKEN0123456789abcdef";
const REPO: &str = "owner/faeknet-dir";
const NOW: u64 = 1_790_000_000;

#[derive(Default)]
struct MockState {
    /// path -> (sha, bytes)
    files: HashMap<String, (String, Vec<u8>)>,
    writes: u64,
    /// The next N PUTs are answered with 409 without writing, as if another
    /// member had just updated the file.
    conflicts_remaining: usize,
    last_query: String,
    puts: usize,
}

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

fn respond(mut stream: TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
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
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };
    let mut authorized = false;
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if line.eq_ignore_ascii_case(&format!("authorization: Bearer {TOKEN}")) {
            authorized = true;
        }
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        let _ = reader.read_exact(&mut body);
    }
    if !authorized {
        return respond(stream, "401 Unauthorized", r#"{"message":"Bad credentials"}"#);
    }

    let prefix = "/repos/owner/faeknet-dir/contents/";
    let Some(file) = path.strip_prefix(prefix) else {
        return respond(stream, "404 Not Found", r#"{"message":"Not Found"}"#);
    };
    let mut st = state.lock().unwrap();
    st.last_query = query;
    match method.as_str() {
        "GET" => match st.files.get(file).cloned() {
            None => respond(stream, "404 Not Found", r#"{"message":"Not Found"}"#),
            Some((sha, bytes)) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                // GitHub wraps base64 lines, so mimic that.
                let wrapped: String = b64
                    .as_bytes()
                    .chunks(60)
                    .map(|c| format!("{}\n", std::str::from_utf8(c).unwrap()))
                    .collect();
                let json = serde_json::json!({"sha": sha, "content": wrapped, "encoding": "base64"});
                respond(stream, "200 OK", &json.to_string());
            }
        },
        "PUT" => {
            st.puts += 1;
            if st.conflicts_remaining > 0 {
                st.conflicts_remaining -= 1;
                return respond(stream, "409 Conflict", r#"{"message":"does not match"}"#);
            }
            let req: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let content = req["content"].as_str().unwrap();
            let bytes = base64::engine::general_purpose::STANDARD.decode(content).unwrap();
            let given_sha = req.get("sha").and_then(|s| s.as_str()).map(str::to_string);
            let current = st.files.get(file).map(|(sha, _)| sha.clone());
            match (current, given_sha) {
                (Some(cur), Some(given)) if cur == given => {}
                (None, None) => {}
                (Some(_), _) => return respond(stream, "409 Conflict", r#"{"message":"sha mismatch"}"#),
                (None, Some(_)) => return respond(stream, "422 Unprocessable Entity", r#"{"message":"sha given for new file"}"#),
            }
            st.writes += 1;
            let sha = format!("sha{}", st.writes);
            let created = !st.files.contains_key(file);
            st.files.insert(file.to_string(), (sha.clone(), bytes));
            let status = if created { "201 Created" } else { "200 OK" };
            respond(stream, status, &serde_json::json!({"content": {"sha": sha}}).to_string());
        }
        _ => respond(stream, "405 Method Not Allowed", "{}"),
    }
}

fn backend(base: &str) -> GithubBackend {
    GithubBackend::new(REPO, None, DEFAULT_PATH, TOKEN).unwrap().with_api_base(base).unwrap()
}

fn key() -> DirectoryKey {
    DirectoryKey::from_psk_b64(&Cipher::generate_psk_b64()).unwrap()
}

fn peer(last: u8, name: &str) -> PeerRecord {
    PeerRecord {
        name: name.into(),
        virtual_ip: std::net::Ipv4Addr::new(10, 66, 0, last),
        public_ip: std::net::Ipv4Addr::new(8, 8, 8, last),
        public_port: 54321,
        updated_at: NOW,
    }
}

#[test]
fn rejects_bad_repo_path_branch_and_token() {
    assert!(GithubBackend::new("no-slash", None, DEFAULT_PATH, TOKEN).is_err());
    assert!(GithubBackend::new("a/b/c", None, DEFAULT_PATH, TOKEN).is_err());
    assert!(GithubBackend::new("../etc", None, DEFAULT_PATH, TOKEN).is_err());
    assert!(GithubBackend::new(REPO, Some("bad..branch"), DEFAULT_PATH, TOKEN).is_err());
    assert!(GithubBackend::new(REPO, None, "../escape.json", TOKEN).is_err());
    assert!(GithubBackend::new(REPO, None, DEFAULT_PATH, "short").is_err());
    // A quote would break out of the curl config line, so tokens with one are refused.
    assert!(GithubBackend::new(REPO, None, DEFAULT_PATH, "abcdefghijklmnopqrst\"x").is_err());
}

#[test]
fn missing_file_is_not_an_error() {
    let (base, _) = spawn_mock();
    assert!(backend(&base).fetch().unwrap().is_none());
}

#[test]
fn publish_creates_then_updates_with_sha() {
    let (base, state) = spawn_mock();
    let k = key();
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(backend(&base))];
    let first = next_document(None, vec![peer(2, "alice")], NOW);
    publish_all(&backends, &k, &first).unwrap();
    let second = next_document(Some(&first), vec![peer(2, "alice"), peer(3, "bob")], NOW + 10);
    publish_all(&backends, &k, &second).unwrap();
    assert_eq!(fetch_latest(&backends, &k, 0, NOW + 10).best.unwrap(), second);
    assert_eq!(state.lock().unwrap().writes, 2);
}

#[test]
fn a_concurrent_writer_causes_a_reread_and_retry_not_an_overwrite() {
    let (base, state) = spawn_mock();
    let k = key();
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(backend(&base))];
    let doc = next_document(None, vec![peer(2, "alice")], NOW);
    publish_all(&backends, &k, &doc).unwrap();
    state.lock().unwrap().conflicts_remaining = 1;
    let next = next_document(Some(&doc), vec![peer(2, "alice-moved")], NOW + 5);
    publish_all(&backends, &k, &next).unwrap();
    assert_eq!(state.lock().unwrap().puts, 3, "one 409 then one success on the retry");
    assert_eq!(fetch_latest(&backends, &k, 0, NOW + 5).best.unwrap(), next);
}

#[test]
fn gives_up_after_repeated_conflicts() {
    let (base, state) = spawn_mock();
    state.lock().unwrap().conflicts_remaining = 100;
    let err = backend(&base).publish(b"{}").unwrap_err();
    assert!(err.contains("gave up"), "{err}");
    assert_eq!(state.lock().unwrap().puts, PUBLISH_ATTEMPTS);
}

#[test]
fn bad_token_is_reported_and_never_echoed() {
    let (base, _) = spawn_mock();
    let wrong = GithubBackend::new(REPO, None, DEFAULT_PATH, "github_pat_WRONGWRONGWRONGWRONG").unwrap()
        .with_api_base(&base)
        .unwrap();
    let err = wrong.fetch().unwrap_err();
    assert!(err.contains("token rejected"), "{err}");
    assert!(!err.contains("WRONG"), "token must not appear in errors: {err}");
    assert!(!err.contains(TOKEN));
}

#[test]
fn branch_is_sent_as_ref_on_reads_and_as_branch_on_writes() {
    let (base, state) = spawn_mock();
    let b = GithubBackend::new(REPO, Some("directory-test"), DEFAULT_PATH, TOKEN)
        .unwrap()
        .with_api_base(&base)
        .unwrap();
    let _ = b.fetch();
    assert_eq!(state.lock().unwrap().last_query, "ref=directory-test");
    b.publish(b"{\"doc\":\"x\"}").unwrap();
    assert_eq!(state.lock().unwrap().writes, 1);
}

#[test]
fn oversize_publish_is_refused_before_any_request() {
    let (base, state) = spawn_mock();
    assert!(backend(&base).publish(&vec![b'x'; MAX_DOC_BYTES + 1]).is_err());
    assert_eq!(state.lock().unwrap().puts, 0);
}

/// Talks to the real GitHub API. Use a throwaway branch of a repository you own:
/// the test writes and then leaves the file in place, so delete the branch after.
#[test]
#[ignore = "touches the real GitHub API; needs FAEKNET_GH_TEST_REPO and FAEKNET_GH_TEST_TOKEN"]
fn live_github_roundtrip() {
    let repo = std::env::var("FAEKNET_GH_TEST_REPO").expect("set FAEKNET_GH_TEST_REPO");
    let token = std::env::var("FAEKNET_GH_TEST_TOKEN").expect("set FAEKNET_GH_TEST_TOKEN");
    let branch = std::env::var("FAEKNET_GH_TEST_BRANCH").ok();
    let b = GithubBackend::new(&repo, branch.as_deref(), "faeknet-live-test.json", &token).unwrap();
    let k = key();
    let backends: Vec<Box<dyn Backend>> = vec![Box::new(b)];
    let doc = next_document(
        fetch_latest(&backends, &k, 0, now()).best.as_ref(),
        vec![peer(2, "live-test")],
        now(),
    );
    publish_all(&backends, &k, &doc).expect("publish to GitHub");
    assert_eq!(fetch_latest(&backends, &k, 0, now()).best.expect("read back"), doc);
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}
