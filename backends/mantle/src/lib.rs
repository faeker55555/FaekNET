//! MantleDB adapter for the FaekNET peer directory (<https://mantledb.sh>).
//!
//! The directory blob is stored as one JSON entry, `POST`ed to
//! `/v2/<namespace>/<path>` and read back with `GET`. On a claimed namespace,
//! writes need the namespace key in `X-Mantle-Key`.
//!
//! Requests go through the system `curl`, as `registry.rs` does, so no TLS
//! stack is compiled into the mesh binaries. The key is passed through a curl
//! config read from stdin, so it never appears in the process list.
//!
//! Integrity does not depend on this service. The blob is authenticated by
//! `meow_meow_core::backends` with a key derived from the network PSK, so a
//! compromised or hostile MantleDB can only delete or withhold the directory.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use meow_meow_core::backends::{Backend, MAX_DOC_BYTES};

pub const DEFAULT_BASE: &str = "https://mantledb.sh";
/// Entry path used when the caller does not choose one.
pub const DEFAULT_PATH: &str = "faeknet/directory";
const TIMEOUT_SECS: &str = "20";
const MAX_NAMESPACE_LEN: usize = 64;

/// Storage adapter for one MantleDB entry.
///
/// Deliberately does not implement `Debug`, so the write key cannot end up in
/// logs by accident.
pub struct MantleBackend {
    base: String,
    namespace: String,
    path: String,
    write_key: Option<String>,
}

impl MantleBackend {
    /// `write_key` is required for claimed namespaces. Leave it `None` for an
    /// unclaimed namespace, which anyone can write to.
    pub fn new(namespace: &str, write_key: Option<&str>) -> Result<Self, String> {
        validate_namespace(namespace)?;
        if let Some(key) = write_key {
            validate_key(key)?;
        }
        Ok(MantleBackend {
            base: DEFAULT_BASE.to_string(),
            namespace: namespace.to_string(),
            path: DEFAULT_PATH.to_string(),
            write_key: write_key.map(str::to_string),
        })
    }

    /// Points at another MantleDB-compatible host. Only `https://` is accepted,
    /// plus plain `http://127.0.0.1` for local testing.
    pub fn with_base(mut self, base: &str) -> Result<Self, String> {
        validate_base(base)?;
        self.base = base.trim_end_matches('/').to_string();
        Ok(self)
    }

    pub fn with_path(mut self, path: &str) -> Result<Self, String> {
        validate_path(path)?;
        self.path = path.to_string();
        Ok(self)
    }

    fn url(&self) -> String {
        format!("{}/v2/{}/{}", self.base, self.namespace, self.path)
    }

    /// Runs one request and returns `(status, body)`.
    fn request(&self, method: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>), String> {
        let mut config = String::new();
        if body.is_some() {
            config.push_str("header = \"Content-Type: application/json\"\n");
        }
        if let Some(key) = &self.write_key {
            // Key charset is validated, so no escaping is needed here.
            config.push_str(&format!("header = \"X-Mantle-Key: {key}\"\n"));
        }

        let proto = if self.base.starts_with("http://") { "=https,http" } else { "=https" };
        let mut cmd = Command::new("curl");
        cmd.args(["-q", "--silent", "--show-error", "--proto", proto])
            .args(["--connect-timeout", "5", "--max-time", TIMEOUT_SECS])
            .args(["--max-filesize", &(MAX_DOC_BYTES + 512).to_string()])
            .args(["-X", method])
            // Append the status code after the body, separated by a newline.
            .args(["-w", "\n%{http_code}"])
            .args(["-K", "-"]);

        let body_file = match body {
            Some(bytes) => {
                let file = temp_file(bytes)?;
                cmd.arg("--data-binary").arg(format!("@{}", file.display()));
                Some(file)
            }
            None => None,
        };
        cmd.arg(self.url());

        let result = run_curl(cmd, &config);
        if let Some(file) = body_file {
            let _ = std::fs::remove_file(file);
        }
        result
    }
}

impl Backend for MantleBackend {
    fn name(&self) -> String {
        format!("mantle:{}/{}", self.namespace, self.path)
    }

    fn fetch(&self) -> Result<Option<Vec<u8>>, String> {
        match self.request("GET", None)? {
            (200, body) => Ok(Some(body)),
            (404, _) => Ok(None),
            (status, body) => Err(http_error(status, &body)),
        }
    }

    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_DOC_BYTES {
            return Err("directory exceeds size limit".into());
        }
        match self.request("POST", Some(bytes))? {
            (200, _) => Ok(()),
            (status, body) => Err(http_error(status, &body)),
        }
    }
}

/// Claims a namespace and returns its write key. The key is shown once by the
/// service and cannot be recovered, so the caller must store it immediately.
pub fn claim(namespace: &str, email: Option<&str>) -> Result<String, String> {
    claim_at(DEFAULT_BASE, namespace, email)
}

/// Same as [`claim`], against another base URL (see [`MantleBackend::with_base`]).
pub fn claim_at(base: &str, namespace: &str, email: Option<&str>) -> Result<String, String> {
    validate_namespace(namespace)?;
    validate_base(base)?;
    let mut url = format!("{}/v2/claim/{namespace}", base.trim_end_matches('/'));
    if let Some(email) = email {
        if !email.contains('@') || email.contains(['&', '?', ' ']) {
            return Err("recovery email looks invalid".into());
        }
        url.push_str(&format!("?email={email}"));
    }
    let proto = if base.starts_with("http://") { "=https,http" } else { "=https" };
    let mut cmd = Command::new("curl");
    cmd.args(["-q", "--silent", "--show-error", "--proto", proto])
        .args(["--connect-timeout", "5", "--max-time", TIMEOUT_SECS])
        .args(["-w", "\n%{http_code}", &url])
        .args(["-K", "-"]);
    let (status, body) = run_curl(cmd, "")?;
    if status != 200 {
        return Err(http_error(status, &body));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("unexpected claim response: {e}"))?;
    json.get("key")
        .and_then(|k| k.as_str())
        .map(str::to_string)
        .ok_or_else(|| "claim response had no key".to_string())
}

fn validate_namespace(ns: &str) -> Result<(), String> {
    let ok = !ns.is_empty()
        && ns.len() <= MAX_NAMESPACE_LEN
        && ns.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid namespace {ns:?}: use 1-{MAX_NAMESPACE_LEN} letters, digits, '-' or '_'"))
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    let ok = (8..=128).contains(&key.len()) && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if ok {
        Ok(())
    } else {
        Err("write key has an unexpected format".into())
    }
}

fn validate_base(base: &str) -> Result<(), String> {
    let ok = base.starts_with("https://") || base.starts_with("http://127.0.0.1:");
    if ok && !base.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err("base URL must start with https:// (http://127.0.0.1 is allowed for tests)".into())
    }
}

/// Runs curl with `config` on stdin and returns `(status, body)`.
fn run_curl(mut cmd: Command, config: &str) -> Result<(u16, Vec<u8>), String> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run curl: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(config.as_bytes())
        .map_err(|e| format!("curl config: {e}"))?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("request failed: {}", err.trim()));
    }
    split_status(&out.stdout)
}

fn validate_path(path: &str) -> Result<(), String> {
    let ok = !path.is_empty()
        && path.len() <= 256
        && !path.starts_with('/')
        && !path.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..")
        && path.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_./".contains(&c));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid entry path {path:?}"))
    }
}

/// curl was run with `-w "\n<code>"`, so the status is the last line.
fn split_status(stdout: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let text = stdout;
    let idx = text.iter().rposition(|&b| b == b'\n').ok_or("malformed response from curl")?;
    let code = std::str::from_utf8(&text[idx + 1..])
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or("malformed status from curl")?;
    Ok((code, text[..idx].to_vec()))
}

fn http_error(status: u16, body: &[u8]) -> String {
    let hint = match status {
        401 | 403 => "write key rejected or missing",
        404 => "not found",
        413 => "entry too large for this namespace",
        429 => "rate limited; try again later",
        500..=599 => "service error",
        _ => "unexpected response",
    };
    // Only a short, non-sensitive snippet of the body is included.
    let snippet: String = String::from_utf8_lossy(body).chars().take(120).collect();
    format!("HTTP {status} ({hint}): {snippet}")
}

fn temp_file(bytes: &[u8]) -> Result<PathBuf, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let path = std::env::temp_dir().join(format!("faeknet-mantle-{}-{nanos}.json", std::process::id()));
    std::fs::write(&path, bytes).map_err(|e| format!("temp file: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests;
