//! GitHub adapter for the shared directory.
//!
//! The directory is one file in a GitHub repository, read and written through
//! the Contents API. Writes are compare-and-swap: the request carries the blob
//! `sha` we read, so GitHub refuses (409/422) if another member changed the
//! file in between. We then re-read and retry, instead of silently overwriting
//! someone else's update.
//!
//! Setup: create a repository (private or public) and a fine-grained token that
//! can read and write contents of that one repository only. Every member gets
//! the same token through the directory invite. A token holder can delete or
//! withhold the directory, but cannot forge one (see the module-level trust
//! model in `backends/mod.rs`).
//!
//! Requests go through the system `curl`, like the MantleDB adapter, and the
//! token is passed through a curl config read from stdin, so it never appears
//! in the process list or in error messages.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::Deserialize;

use super::{Backend, MAX_DOC_BYTES};

pub const DEFAULT_API: &str = "https://api.github.com";
pub const DEFAULT_PATH: &str = "faeknet-directory.json";
const TIMEOUT_SECS: &str = "20";
/// Attempts per publish when another writer keeps winning the race.
const PUBLISH_ATTEMPTS: usize = 3;
const COMMIT_MESSAGE: &str = "faeknet: directory update";

/// Storage adapter for one file in one GitHub repository.
///
/// Deliberately does not implement `Debug`, so the token cannot end up in logs.
pub struct GithubBackend {
    api: String,
    owner: String,
    repo: String,
    branch: Option<String>,
    path: String,
    token: String,
}

#[derive(Deserialize)]
struct ContentsResponse {
    sha: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    encoding: String,
}

impl GithubBackend {
    /// `repo` is `owner/name`. `branch` of `None` uses the repository's default
    /// branch. `path` is the file inside the repository.
    pub fn new(repo: &str, branch: Option<&str>, path: &str, token: &str) -> Result<Self, String> {
        let (owner, name) = repo.split_once('/').ok_or("repository must be written as owner/name")?;
        validate_name_part(owner, "owner")?;
        validate_name_part(name, "repository")?;
        if let Some(branch) = branch {
            validate_branch(branch)?;
        }
        validate_path(path)?;
        validate_token(token)?;
        Ok(GithubBackend {
            api: DEFAULT_API.to_string(),
            owner: owner.to_string(),
            repo: name.to_string(),
            branch: branch.map(str::to_string),
            path: path.to_string(),
            token: token.to_string(),
        })
    }

    /// Points at another GitHub-API-compatible host. Only `https://` is accepted,
    /// plus plain `http://127.0.0.1` for local testing.
    pub fn with_api_base(mut self, base: &str) -> Result<Self, String> {
        validate_base(base)?;
        self.api = base.trim_end_matches('/').to_string();
        Ok(self)
    }

    fn url(&self) -> String {
        format!("{}/repos/{}/{}/contents/{}", self.api, self.owner, self.repo, self.path)
    }

    /// Runs one request. `body` is sent as JSON.
    fn request(&self, method: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>), String> {
        let mut config = String::new();
        config.push_str(&format!("header = \"Authorization: Bearer {}\"\n", self.token));
        config.push_str("header = \"Accept: application/vnd.github+json\"\n");
        config.push_str("header = \"X-GitHub-Api-Version: 2022-11-28\"\n");
        config.push_str("header = \"User-Agent: faeknet-directory\"\n");

        let mut url = self.url();
        if let Some(branch) = &self.branch {
            // Only the branch charset is allowed (validated), so no escaping is needed.
            url.push_str(&format!("?ref={branch}"));
        }

        let proto = if self.api.starts_with("http://") { "=https,http" } else { "=https" };
        let mut cmd = Command::new("curl");
        cmd.args(["-q", "--silent", "--show-error", "--proto", proto])
            .args(["--connect-timeout", "5", "--max-time", TIMEOUT_SECS])
            .args(["--max-filesize", &(MAX_DOC_BYTES * 2).to_string()])
            .args(["-X", method])
            .args(["-w", "\n%{http_code}"])
            .args(["-K", "-"]);

        let body_file = match body {
            Some(bytes) => {
                config.push_str("header = \"Content-Type: application/json\"\n");
                let file = temp_file(bytes)?;
                cmd.arg("--data-binary").arg(format!("@{}", file.display()));
                Some(file)
            }
            None => None,
        };
        cmd.arg(url);

        let result = run_curl(cmd, &config);
        if let Some(file) = body_file {
            let _ = std::fs::remove_file(file);
        }
        result
    }

    /// Reads the file. Returns `(sha, bytes)`, or `None` if it does not exist yet.
    fn read(&self) -> Result<Option<(String, Vec<u8>)>, String> {
        match self.request("GET", None)? {
            (200, body) => {
                let parsed: ContentsResponse =
                    serde_json::from_slice(&body).map_err(|e| format!("unexpected GitHub response: {e}"))?;
                if parsed.encoding != "base64" {
                    return Err(format!("unexpected content encoding {:?}", parsed.encoding));
                }
                // GitHub wraps base64 in newlines.
                let cleaned: String = parsed.content.chars().filter(|c| !c.is_whitespace()).collect();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(cleaned)
                    .map_err(|e| format!("directory file is not valid base64: {e}"))?;
                if bytes.len() > MAX_DOC_BYTES {
                    return Err("stored directory exceeds size limit".into());
                }
                Ok(Some((parsed.sha, bytes)))
            }
            (404, _) => Ok(None),
            (status, body) => Err(http_error(status, &body)),
        }
    }
}

impl Backend for GithubBackend {
    fn name(&self) -> String {
        format!("github:{}/{}:{}", self.owner, self.repo, self.path)
    }

    fn fetch(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.read()?.map(|(_, bytes)| bytes))
    }

    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_DOC_BYTES {
            return Err("directory exceeds size limit".into());
        }
        let content = base64::engine::general_purpose::STANDARD.encode(bytes);
        for _ in 0..PUBLISH_ATTEMPTS {
            // Re-read the sha on every attempt, so the PUT is conditional on the
            // version we are about to replace.
            let sha = self.read()?.map(|(sha, _)| sha);
            let mut body = serde_json::json!({ "message": COMMIT_MESSAGE, "content": content });
            if let Some(sha) = sha {
                body["sha"] = serde_json::Value::String(sha);
            }
            if let Some(branch) = &self.branch {
                body["branch"] = serde_json::Value::String(branch.clone());
            }
            let payload = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
            match self.request("PUT", Some(&payload))? {
                (200 | 201, _) => return Ok(()),
                // 409: sha no longer current. 422: file appeared since our read.
                // Either way another writer won the race; read again and retry.
                (409 | 422, _) => continue,
                (status, body) => return Err(http_error(status, &body)),
            }
        }
        Err(format!("gave up after {PUBLISH_ATTEMPTS} attempts: the directory kept changing during the update"))
    }
}

fn validate_name_part(part: &str, what: &str) -> Result<(), String> {
    let ok = !part.is_empty()
        && part.len() <= 100
        && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.')
        && !part.starts_with('.');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid GitHub {what} name {part:?}"))
    }
}

fn validate_branch(branch: &str) -> Result<(), String> {
    let ok = !branch.is_empty()
        && branch.len() <= 200
        && !branch.contains("..")
        && !branch.starts_with('/')
        && branch.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_./".contains(&c));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid branch name {branch:?}"))
    }
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
        Err(format!("invalid file path {path:?}"))
    }
}

/// Tokens are `github_pat_...` or `ghp_...`. Anything with other characters
/// could break out of the curl config, so refuse it outright.
fn validate_token(token: &str) -> Result<(), String> {
    let ok = (20..=255).contains(&token.len())
        && token.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
    if ok {
        Ok(())
    } else {
        Err("GitHub token has an unexpected format".into())
    }
}

fn validate_base(base: &str) -> Result<(), String> {
    let ok = base.starts_with("https://") || base.starts_with("http://127.0.0.1:");
    if ok && !base.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err("API base must start with https:// (http://127.0.0.1 is allowed for tests)".into())
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

/// curl was run with `-w "\n<code>"`, so the status is the last line.
fn split_status(stdout: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let idx = stdout.iter().rposition(|&b| b == b'\n').ok_or("malformed response from curl")?;
    let code = std::str::from_utf8(&stdout[idx + 1..])
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or("malformed status from curl")?;
    Ok((code, stdout[..idx].to_vec()))
}

fn http_error(status: u16, body: &[u8]) -> String {
    let hint = match status {
        401 => "token rejected or expired",
        403 => "token lacks permission for this repository, or rate limited",
        404 => "repository, branch or token not visible",
        429 => "rate limited; try again later",
        500..=599 => "GitHub service error",
        _ => "unexpected response",
    };
    // Only a short snippet of GitHub's message is included; never the request.
    let snippet: String = String::from_utf8_lossy(body)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(120)
        .collect();
    format!("HTTP {status} ({hint}): {snippet}")
}

fn temp_file(bytes: &[u8]) -> Result<PathBuf, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let path = std::env::temp_dir().join(format!("faeknet-github-{}-{nanos}.json", std::process::id()));
    std::fs::write(&path, bytes).map_err(|e| format!("temp file: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests;
