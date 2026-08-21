// "Start meow-meow with the session" support, per platform:
//
// - Linux: an XDG autostart `.desktop` entry in
//   `$XDG_CONFIG_HOME/autostart` (or `~/.config/autostart`), which all
//   mainstream desktops (GNOME, KDE, XFCE, ...) honor at login.
// - Windows: the `meow-meow_gui` value under
//   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, managed via
//   the system `reg.exe` (no admin rights needed for HKCU, and no extra
//   dependencies in the app itself).
//
// Both launch the GUI with `--minimized`, so the mesh starts quietly in
// the tray at login instead of popping a window in the user's face.

use std::path::PathBuf;

pub fn is_enabled() -> bool {
    platform::is_enabled()
}

pub fn enable() -> Result<(), String> {
    platform::enable()
}

pub fn disable() -> Result<(), String> {
    platform::disable()
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot locate this executable: {e}"))
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::path::PathBuf;

    pub fn autostart_dir() -> Option<PathBuf> {
        if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
            if !x.is_empty() {
                return Some(PathBuf::from(x).join("autostart"));
            }
        }
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config").join("autostart"))
    }

    pub fn desktop_file() -> Option<PathBuf> {
        autostart_dir().map(|d| d.join("meow-meow-gui.desktop"))
    }

    pub fn is_enabled() -> bool {
        desktop_file().map(|p| p.exists()).unwrap_or(false)
    }

    pub fn enable() -> Result<(), String> {
        let dir = autostart_dir().ok_or_else(|| "no home directory found".to_string())?;
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let file = desktop_file().ok_or_else(|| "no autostart directory found".to_string())?;

        let exe = super::current_exe()?;
        // Desktop Entry Exec quoting: wrap the path in double quotes and
        // escape any embedded backslashes/quotes.
        let exe_escaped = exe
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");

        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=meow-meow\n\
             Comment=meow-meow virtual LAN\n\
             Exec=\"{exe_escaped}\" --minimized\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        );
        fs::write(&file, content).map_err(|e| format!("cannot write {}: {e}", file.display()))
    }

    pub fn disable() -> Result<(), String> {
        if let Some(file) = desktop_file() {
            // A missing file is already the desired end state.
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot remove {}: {e}", file.display())),
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "meow-meow_gui";

    fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("reg")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    }

    fn exe_cmdline() -> Result<String, String> {
        let exe = super::current_exe()?;
        Ok(format!("\"{}\" --minimized", exe.display()))
    }

    pub fn is_enabled() -> bool {
        reg(&["query", RUN_KEY, "/v", VALUE])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn enable() -> Result<(), String> {
        let data = exe_cmdline()?;
        match reg(&["add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &data, "/f"]) {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(e) => Err(format!("cannot run reg.exe: {e}")),
        }
    }

    pub fn disable() -> Result<(), String> {
        // Deleting an absent value reports an error; that's fine.
        let _ = reg(&["delete", RUN_KEY, "/v", VALUE, "/f"]);
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    pub fn is_enabled() -> bool {
        false
    }
    pub fn enable() -> Result<(), String> {
        Err("autostart is not supported on this platform".to_string())
    }
    pub fn disable() -> Result<(), String> {
        Ok(())
    }
}
