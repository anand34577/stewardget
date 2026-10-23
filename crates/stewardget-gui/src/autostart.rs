//! "Start with your computer": a login item that launches StewardGet minimised
//! (`--background`), so the browser extension always has something to talk to.
//!
//! * Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, the same
//!   value the installer writes. A "disabled" mark from Task Manager counts as off.
//! * macOS: a LaunchAgent in `~/Library/LaunchAgents`.
//! * Linux: an XDG autostart entry in `~/.config/autostart`.
//!
//! A login item of Velox, as the app was called before 0.7, is taken over.

use std::io;
use std::path::PathBuf;

/// The executable a login item should start (the AppImage itself, not its
/// temporary mount, when running from one).
fn launcher() -> io::Result<PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
        return Ok(PathBuf::from(appimage));
    }
    std::env::current_exe()
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const NAME: &str = "StewardGet";
    const LEGACY: &str = "Velox";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn read(key: &str, name: &str, flags: u32) -> Option<Vec<u8>> {
        let (key, name) = (wide(key), wide(name));
        let mut len = 0u32;
        // SAFETY: valid NUL-terminated strings; a null buffer asks for the size.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` holds `len` bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        (status == ERROR_SUCCESS).then(|| {
            buf.truncate(len as usize);
            buf
        })
    }

    fn delete(key: &str, name: &str) -> io::Result<()> {
        let (key, name) = (wide(key), wide(name));
        // SAFETY: valid NUL-terminated strings.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }

    fn enabled(name: &str) -> bool {
        if read(RUN, name, RRF_RT_REG_SZ).is_none() {
            return false;
        }
        // Task Manager's "Disable" leaves the Run value and marks it here (odd first byte).
        !read(APPROVED, name, RRF_RT_REG_BINARY).is_some_and(|v| v.first().is_some_and(|b| b & 1 == 1))
    }

    pub fn is_enabled() -> bool {
        enabled(NAME)
    }

    pub fn take_legacy() -> bool {
        let on = enabled(LEGACY);
        let _ = delete(RUN, LEGACY);
        let _ = delete(APPROVED, LEGACY);
        on
    }

    pub fn set(enabled: bool) -> io::Result<()> {
        if !enabled {
            return delete(RUN, NAME);
        }
        let exe = super::launcher()?;
        let command = wide(&format!("\"{}\" --background", exe.display()));
        let (key, name) = (wide(RUN), wide(NAME));
        // SAFETY: valid NUL-terminated strings; the data length is in bytes.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        // Clear a "disabled in Task Manager" mark, otherwise Windows would skip it.
        delete(APPROVED, NAME)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::io;
    use std::path::PathBuf;

    fn agent(label: &str) -> Option<PathBuf> {
        directories::BaseDirs::new().map(|d| d.home_dir().join(format!("Library/LaunchAgents/{label}.plist")))
    }

    fn plist() -> Option<PathBuf> {
        agent("dev.stewardget.StewardGet")
    }

    pub fn is_enabled() -> bool {
        plist().is_some_and(|p| p.is_file())
    }

    pub fn take_legacy() -> bool {
        agent("dev.velox.Velox").is_some_and(|p| std::fs::remove_file(p).is_ok())
    }

    pub fn set(enabled: bool) -> io::Result<()> {
        let path = plist().ok_or_else(|| io::Error::other("no home folder"))?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                r => r,
            };
        }
        let exe = super::launcher()?;
        let exe = exe.display().to_string().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.stewardget.StewardGet</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--background</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, body)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::io;
    use std::path::PathBuf;

    fn desktop(id: &str) -> Option<PathBuf> {
        directories::BaseDirs::new().map(|d| d.config_dir().join(format!("autostart/{id}.desktop")))
    }

    fn entry() -> Option<PathBuf> {
        desktop("dev.stewardget.StewardGet")
    }

    fn enabled(path: Option<PathBuf>) -> bool {
        path.and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| !s.contains("Hidden=true"))
    }

    pub fn is_enabled() -> bool {
        enabled(entry())
    }

    pub fn take_legacy() -> bool {
        let legacy = desktop("dev.velox.Velox");
        let on = enabled(legacy.clone());
        legacy.is_some_and(|p| std::fs::remove_file(p).is_ok()) && on
    }

    pub fn set(enabled: bool) -> io::Result<()> {
        let path = entry().ok_or_else(|| io::Error::other("no home folder"))?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                r => r,
            };
        }
        let exe = super::launcher()?;
        // Desktop-entry quoting: wrap in double quotes, escape `"`, `` ` ``, `$` and `\`.
        let mut quoted = String::from("\"");
        for c in exe.display().to_string().chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        let body = format!(
            "[Desktop Entry]\nType=Application\nName=StewardGet\nComment=Download manager\nExec={quoted} --background\nIcon=dev.stewardget.StewardGet\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, body)
    }
}

/// Whether StewardGet starts when the user signs in.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Adds or removes the login item.
pub fn set(enabled: bool) -> io::Result<()> {
    imp::set(enabled)
}

/// Replaces a login item left by Velox, as the app was called before 0.7,
/// with one for this app.
pub fn adopt_legacy() {
    if imp::take_legacy() {
        let _ = imp::set(true);
    }
}
