// Self-updater for the GUI (Linux + Windows), driven from the GitHub
// Releases the CI workflow publishes (`lan_mesh-<tag>-<platform>.tar.gz`
// / `.zip` + `.sha256`; the older hand-made `LINUX.zip` / `WINDOWS.zip`
// naming is also accepted).
//
// Design constraints that shaped this module:
// - No heavy new dependencies: downloads go through the platform's own
//   tools (curl is standard on Linux and ships with Windows 10+;
//   PowerShell/wget are fallbacks), SHA-256 verification uses
//   sha256sum/openssl on Linux and certutil/Get-FileHash on Windows, and
//   extraction uses the platform tar (bsdtar handles .zip on Windows).
// - Never half-install: the archive is fully downloaded, checksum-verified
//   and extracted to a temp dir first. Replacing the running binaries
//   happens only through a small detached script that waits for this
//   process to exit, swaps the files, and relaunches the GUI.
// - No silent downgrades: the update is only offered when the latest
//   release tag parses to a version *greater* than this build's own
//   version. Release tags are expected to track the Cargo.toml versions
//   (e.g. tag v0.31.0 for version 0.31.0).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

pub const REPO: &str = "faeker55555/FaekNET";
const USER_AGENT: &str = "lan_mesh-updater";

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// Full release tag, e.g. "v0.31.0".
    pub tag: String,
    /// Tag with any leading "v" stripped, for display.
    pub version: String,
    /// Release notes (truncated).
    pub notes: String,
    pub archive_url: String,
    pub archive_size: u64,
    pub checksum_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate { version: String },
    Available { version: String },
    NoRelease,
    Downloading { bytes: u64, total: u64 },
    Verifying,
    Extracting,
    Applying,
    Failed(String),
}

pub struct UpdaterState {
    pub status: UpdateStatus,
    pub info: Option<UpdateInfo>,
}

pub fn shared() -> Arc<Mutex<UpdaterState>> {
    Arc::new(Mutex::new(UpdaterState {
        status: UpdateStatus::Idle,
        info: None,
    }))
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parses "0.25", "0.3", "v0.31.0", ... into (major, minor, patch).
/// Returns None for tags that don't start with a number ("BETA").
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v').trim_start_matches('V');
    let mut parts = [0u64; 3];
    let mut filled = 0usize;
    for (i, chunk) in s.split('.').take(3).enumerate() {
        let digits: String = chunk.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        if let Ok(v) = digits.parse::<u64>() {
            parts[i] = v;
            filled = i + 1;
        }
    }
    if filled == 0 {
        None
    } else {
        Some((parts[0], parts[1], parts[2]))
    }
}

fn set_status(state: &Arc<Mutex<UpdaterState>>, status: UpdateStatus) {
    state.lock().unwrap().status = status;
}

/// Kicks off a background "is there a newer release?" check. The result
/// (including a ready-to-install `UpdateInfo`) is written into `state`.
pub fn start_check(state: Arc<Mutex<UpdaterState>>) {
    std::thread::spawn(move || {
        set_status(&state, UpdateStatus::Checking);
        let outcome = fetch_latest_release();
        let mut guard = state.lock().unwrap();
        guard.info = None;
        match outcome {
            Ok(CheckOutcome::Available(info)) => {
                guard.status = UpdateStatus::Available {
                    version: info.version.clone(),
                };
                guard.info = Some(info);
            }
            Ok(CheckOutcome::UpToDate(version)) => {
                guard.status = UpdateStatus::UpToDate { version };
            }
            Ok(CheckOutcome::NoUsableRelease) => {
                guard.status = UpdateStatus::NoRelease;
            }
            Err(e) => {
                guard.status = UpdateStatus::Failed(e);
            }
        }
    });
}

/// Downloads, verifies and stages the update, then asks the caller's
/// `quit` flag to be set once the swap script has been launched (the GUI
/// must then exit so the script can replace the binaries).
pub fn start_install(state: Arc<Mutex<UpdaterState>>, info: UpdateInfo, quit: Arc<AtomicBool>) {
    std::thread::spawn(move || match install(&state, &info) {
        Ok(()) => {
            set_status(&state, UpdateStatus::Applying);
            quit.store(true, Ordering::SeqCst);
        }
        Err(e) => {
            set_status(&state, UpdateStatus::Failed(e));
        }
    });
}

enum CheckOutcome {
    Available(UpdateInfo),
    UpToDate(String),
    NoUsableRelease,
}

fn fetch_latest_release() -> Result<CheckOutcome, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let text = http_get_text(&url)?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad response from GitHub API: {e}"))?;

    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let Some(latest) = parse_version(&tag) else {
        // Not a versioned release (e.g. a "BETA" tag) -- nothing sensible
        // to upgrade to.
        return Ok(CheckOutcome::NoUsableRelease);
    };
    let current = parse_version(current_version()).unwrap_or((0, 0, 0));
    if latest <= current {
        return Ok(CheckOutcome::UpToDate(tag));
    }

    let Some(assets) = json.get("assets").and_then(|a| a.as_array()) else {
        return Ok(CheckOutcome::NoUsableRelease);
    };
    let Some((archive_url, archive_size, checksum_url)) = pick_assets(assets) else {
        // Newer release exists, but it has no package for this platform.
        return Ok(CheckOutcome::NoUsableRelease);
    };

    let notes = json
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or("")
        .to_string();
    let notes: String = notes.chars().take(2000).collect();

    Ok(CheckOutcome::Available(UpdateInfo {
        version: tag.trim_start_matches('v').trim_start_matches('V').to_string(),
        tag,
        notes,
        archive_url,
        archive_size,
        checksum_url,
    }))
}

/// Picks the platform's release package from the assets list. Accepts
/// both the CI naming (`lan_mesh-<tag>-linux-x86_64.tar.gz` /
/// `...-windows-x86_64.zip`) and the legacy hand-made naming
/// (`LINUX.zip` / `WINDOWS.zip`). Returns (archive url, size, optional
/// .sha256 url).
fn pick_assets(assets: &[serde_json::Value]) -> Option<(String, u64, Option<String>)> {
    let is_windows = cfg!(target_os = "windows");
    let platform = if is_windows { "windows" } else { "linux" };

    // Candidate archives: CI-style first (prefer .tar.gz on Linux), then
    // the legacy flat names.
    let mut candidates: Vec<(i32, &serde_json::Value)> = Vec::new();
    for a in assets {
        let Some(name) = a.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let lower = name.to_lowercase();
        let rank = if lower.contains(platform) {
            if lower.ends_with(".tar.gz") {
                0
            } else if lower.ends_with(".zip") {
                1
            } else {
                continue;
            }
        } else if lower == format!("{platform}.zip") {
            2
        } else {
            continue;
        };
        candidates.push((rank, a));
    }
    candidates.sort_by_key(|(rank, _)| *rank);
    let (_, archive) = candidates.first()?;

    let name = archive.get("name")?.as_str()?;
    let url = archive.get("browser_download_url")?.as_str()?.to_string();
    let size = archive.get("size").and_then(|s| s.as_u64()).unwrap_or(0);

    let checksum_name = format!("{name}.sha256");
    let checksum_url = assets.iter().find_map(|a| {
        let n = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if n.eq_ignore_ascii_case(&checksum_name) {
            a.get("browser_download_url")
                .and_then(|u| u.as_str())
                .map(|u| u.to_string())
        } else {
            None
        }
    });

    Some((url, size, checksum_url))
}

// ---------------------------------------------------------------------
// Download plumbing
// ---------------------------------------------------------------------

fn download(
    url: &str,
    dest: &Path,
    progress: Option<(&Arc<Mutex<UpdaterState>>, u64)>,
) -> Result<(), String> {
    match curl_download(url, dest, progress) {
        Ok(()) => return Ok(()),
        Err(curl_err) => {
            #[cfg(target_os = "windows")]
            {
                match ps_download(url, dest) {
                    Ok(()) => return Ok(()),
                    Err(ps_err) => {
                        return Err(format!("download failed -- curl: {curl_err} | powershell: {ps_err}"))
                    }
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                match wget_download(url, dest) {
                    Ok(()) => return Ok(()),
                    Err(wget_err) => {
                        return Err(format!("download failed -- curl: {curl_err} | wget: {wget_err}"))
                    }
                }
            }
        }
    }
}

fn hide_console(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = cmd;
    }
}

fn curl_download(
    url: &str,
    dest: &Path,
    progress: Option<(&Arc<Mutex<UpdaterState>>, u64)>,
) -> Result<(), String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-L", "--fail", "--connect-timeout", "20", "-A", USER_AGENT, "-o"])
        .arg(dest)
        .arg(url);
    hide_console(&mut cmd);
    match progress {
        Some((state, total)) => run_with_progress(cmd, dest, state, total),
        None => {
            let out = cmd.output().map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(())
            } else {
                Err(format!("curl exited with {}", out.status))
            }
        }
    }
}

fn wget_download(url: &str, dest: &Path) -> Result<(), String> {
    let header = format!("--header=User-Agent: {USER_AGENT}");
    let out = Command::new("wget")
        .arg("-q")
        .arg("--timeout=20")
        .arg(&header)
        .arg("-O")
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("wget exited with {}", out.status))
    }
}

#[cfg(target_os = "windows")]
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn ps_download(url: &str, dest: &Path) -> Result<(), String> {
    let script = format!(
        "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -UseBasicParsing -Uri '{}' -OutFile '{}'",
        ps_quote(url),
        ps_quote(&dest.to_string_lossy())
    );
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Runs the given downloader child while polling the destination file
/// size to feed the progress UI.
fn run_with_progress(
    mut cmd: Command,
    dest: &Path,
    state: &Arc<Mutex<UpdaterState>>,
    total: u64,
) -> Result<(), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                if status.success() {
                    return Ok(());
                }
                return Err(format!("downloader exited with {status}"));
            }
            None => {}
        }
        let bytes = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        set_status(state, UpdateStatus::Downloading { bytes, total });
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn http_get_text(url: &str) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!("lan_mesh_api_{}.json", std::process::id()));
    download(url, &tmp, None)?;
    let text = fs::read_to_string(&tmp).map_err(|e| format!("cannot read response: {e}"))?;
    let _ = fs::remove_file(&tmp);
    Ok(text)
}

// ---------------------------------------------------------------------
// Checksum + extraction
// ---------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
fn sha256_of(path: &Path) -> Result<String, String> {
    if let Ok(out) = Command::new("sha256sum").arg(path).output() {
        if out.status.success() {
            if let Some(field) = String::from_utf8_lossy(&out.stdout).split_whitespace().next() {
                return Ok(field.to_lowercase());
            }
        }
    }
    if let Ok(out) = Command::new("openssl")
        .args(["dgst", "-sha256"])
        .arg(path)
        .output()
    {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(hash) = line.split('=').nth(1) {
                    let hash = hash.trim();
                    if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Ok(hash.to_lowercase());
                    }
                }
            }
        }
    }
    Err("no SHA-256 tool available (tried sha256sum, openssl)".to_string())
}

#[cfg(target_os = "windows")]
fn sha256_of(path: &Path) -> Result<String, String> {
    let mut cmd = Command::new("certutil");
    cmd.args(["-hashfile"]).arg(path).arg("SHA256");
    hide_console(&mut cmd);
    if let Ok(out) = cmd.output() {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let t = line.trim();
                if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Ok(t.to_lowercase());
                }
            }
        }
    }
    // Fallback: PowerShell's Get-FileHash.
    let script = format!(
        "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash",
        ps_quote(&path.to_string_lossy())
    );
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        let t = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
        if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(t);
        }
    }
    Err("no SHA-256 tool available (tried certutil, powershell)".to_string())
}

fn extract(archive: &Path, dest_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dest_dir).map_err(|e| format!("cannot create {}: {e}", dest_dir.display()))?;

    #[cfg(target_os = "windows")]
    {
        // bsdtar ships with Windows 10+ and reads .zip transparently.
        let tar = Command::new("tar")
            .args(["-xf"])
            .arg(archive)
            .arg("-C")
            .arg(dest_dir)
            .output();
        if let Ok(out) = tar {
            if out.status.success() {
                return Ok(());
            }
        }
        let script = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            ps_quote(&archive.to_string_lossy()),
            ps_quote(&dest_dir.to_string_lossy())
        );
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ]);
        hide_console(&mut cmd);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(());
        }
        return Err(format!(
            "could not extract {} (tried tar, powershell)",
            archive.display()
        ));
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut tried = Vec::new();
        let is_tgz = archive
            .to_string_lossy()
            .to_lowercase()
            .ends_with(".tar.gz");
        let tar = Command::new("tar")
            .args(if is_tgz { ["-xzf"] } else { ["-xf"] })
            .arg(archive)
            .arg("-C")
            .arg(dest_dir)
            .output();
        if let Ok(out) = tar {
            if out.status.success() {
                return Ok(());
            }
            tried.push("tar");
        }
        // Legacy LINUX.zip releases need a zip-capable tool.
        let unzip = Command::new("unzip")
            .args(["-q", "-o"])
            .arg(archive)
            .arg("-d")
            .arg(dest_dir)
            .output();
        if let Ok(out) = unzip {
            if out.status.success() {
                return Ok(());
            }
            tried.push("unzip");
        }
        let bsdtar = Command::new("bsdtar")
            .args(["-xf"])
            .arg(archive)
            .arg("-C")
            .arg(dest_dir)
            .output();
        if let Ok(out) = bsdtar {
            if out.status.success() {
                return Ok(());
            }
            tried.push("bsdtar");
        }
        Err(format!(
            "could not extract {} (tried {})",
            archive.display(),
            tried.join(", ")
        ))
    }
}

// ---------------------------------------------------------------------
// Staging + swap script
// ---------------------------------------------------------------------

/// Recursively locates the directory containing the GUI binary inside
/// the extracted package (layout varies between release namings).
fn find_payload_dir(root: &Path) -> Result<PathBuf, String> {
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 8 {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            let name = entry
                .file_name()
                .to_string_lossy()
                .to_lowercase();
            if name == "lan_mesh_gui" || name == "lan_mesh_gui.exe" {
                return Ok(path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| root.to_path_buf()));
            }
        }
    }
    Err("downloaded package does not contain the lan_mesh GUI binary".to_string())
}

/// Pairs each updated file in the package with its counterpart next to
/// the currently running executable.
fn staged_replacements(payload_dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate this executable: {e}"))?;
    let exe_dir = exe
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "executable has no parent directory".to_string())?;

    let mut names: Vec<String> = vec![
        "lan_mesh_gui".to_string(),
        "lan_mesh".to_string(),
        "lan_mesh_browser".to_string(),
    ];
    #[cfg(target_os = "windows")]
    {
        for n in &mut names {
            n.push_str(".exe");
        }
        names.push("wintun.dll".to_string());
    }

    let mut pairs = Vec::new();
    for name in names {
        let payload = payload_dir.join(&name);
        let target = exe_dir.join(&name);
        if payload.is_file() && target.is_file() {
            pairs.push((payload, target));
        }
    }
    if pairs.is_empty() {
        return Err("no installable files found in the downloaded package".to_string());
    }
    Ok(pairs)
}

fn is_gui_binary(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name == "lan_mesh_gui" || name == "lan_mesh_gui.exe"
}

#[cfg(not(target_os = "windows"))]
fn sh_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

#[cfg(not(target_os = "windows"))]
fn write_apply_script(work: &Path, pairs: &[(PathBuf, PathBuf)]) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let script = work.join("apply_update.sh");
    let pid = std::process::id();
    let mut s = format!("#!/bin/bash\n# lan_mesh self-update\nwhile kill -0 {pid} 2>/dev/null; do sleep 0.25; done\n");
    for (from, to) in pairs {
        s.push_str(&format!(
            "cp -f '{}' '{}' 2>/dev/null || true\n",
            sh_escape(&from.to_string_lossy()),
            sh_escape(&to.to_string_lossy())
        ));
    }
    if let Some((_, gui)) = pairs.iter().find(|(_, t)| is_gui_binary(t)) {
        s.push_str(&format!("chmod +x '{}' 2>/dev/null || true\n", sh_escape(&gui.to_string_lossy())));
        s.push_str(&format!("nohup '{}' >/dev/null 2>&1 &\n", sh_escape(&gui.to_string_lossy())));
    }
    fs::write(&script, &s).map_err(|e| format!("cannot write apply script: {e}"))?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot chmod apply script: {e}"))?;
    Ok(script)
}

#[cfg(not(target_os = "windows"))]
fn spawn_apply(script: &Path) -> Result<(), String> {
    Command::new("bash")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start apply script: {e}"))
}

#[cfg(target_os = "windows")]
fn write_apply_script(work: &Path, pairs: &[(PathBuf, PathBuf)]) -> Result<PathBuf, String> {
    let script = work.join("apply_update.bat");
    let pid = std::process::id();
    let mut s = String::from("@echo off\r\nrem lan_mesh self-update\r\n");
    s.push_str(":wait\r\n");
    // ~1 second sleep that works without a console (unlike `timeout`).
    s.push_str("ping -n 2 127.0.0.1 >nul\r\n");
    s.push_str(&format!("tasklist /fi \"PID eq {pid}\" /nh 2>nul | findstr /c:\"{pid}\" >nul\r\n"));
    s.push_str("if %errorlevel%==0 goto wait\r\n");
    for (from, to) in pairs {
        s.push_str(&format!(
            "move /Y \"{}\" \"{}\" >nul\r\n",
            from.display(),
            to.display()
        ));
    }
    if let Some((_, gui)) = pairs.iter().find(|(_, t)| is_gui_binary(t)) {
        s.push_str(&format!("start \"\" \"{}\"\r\n", gui.display()));
    }
    fs::write(&script, &s).map_err(|e| format!("cannot write apply script: {e}"))?;
    Ok(script)
}

#[cfg(target_os = "windows")]
fn spawn_apply(script: &Path) -> Result<(), String> {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    Command::new("cmd")
        .arg("/C")
        .arg(script)
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start apply script: {e}"))
}

// ---------------------------------------------------------------------
// Install orchestration
// ---------------------------------------------------------------------

fn install(state: &Arc<Mutex<UpdaterState>>, info: &UpdateInfo) -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("lan_mesh_update_{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| format!("cannot create temp dir: {e}"))?;

    let archive = work.join("update_pkg");

    set_status(
        state,
        UpdateStatus::Downloading {
            bytes: 0,
            total: info.archive_size,
        },
    );
    download(&info.archive_url, &archive, Some((state, info.archive_size)))?;

    if let Some(checksum_url) = &info.checksum_url {
        set_status(state, UpdateStatus::Verifying);
        let checksum_file = work.join("checksum.txt");
        download(checksum_url, &checksum_file, None)?;
        let expected = fs::read_to_string(&checksum_file)
            .map_err(|e| format!("cannot read checksum file: {e}"))?;
        let expected = expected
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();
        if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("release checksum file is malformed -- aborting install".to_string());
        }
        let actual = sha256_of(&archive)?;
        if actual != expected {
            return Err(format!(
                "checksum mismatch (expected {expected}, got {actual}) -- install aborted, nothing was changed"
            ));
        }
    }

    set_status(state, UpdateStatus::Extracting);
    let extract_dir = work.join("extracted");
    extract(&archive, &extract_dir)?;

    let payload = find_payload_dir(&extract_dir)?;
    let pairs = staged_replacements(&payload)?;
    let script = write_apply_script(&work, &pairs)?;
    spawn_apply(&script)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("v0.31.0"), Some((0, 31, 0)));
        assert_eq!(parse_version("0.31.0"), Some((0, 31, 0)));
        assert_eq!(parse_version("0.3"), Some((0, 3, 0)));
        assert_eq!(parse_version("0.25"), Some((0, 25, 0)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1"), Some((1, 0, 0)));
        assert_eq!(parse_version("BETA"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn version_comparison_semantics() {
        // The updater only offers upgrades, never downgrades.
        assert!(parse_version("0.3").unwrap() < parse_version("0.31.0").unwrap());
        assert!(parse_version("0.30.0").unwrap() == parse_version("0.30").unwrap());
        assert!(parse_version("1.0.0").unwrap() > parse_version("0.99.0").unwrap());
    }
}
