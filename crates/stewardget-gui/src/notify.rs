//! System notifications for finished and failed downloads.

/// The app id the Windows installer gives the Start-menu shortcut. Toasts
/// carry StewardGet's name and icon when that shortcut exists.
#[cfg(windows)]
const APP_ID: &str = "dev.stewardget.StewardGet";

/// Shows a notification without blocking the interface. Failures are ignored:
/// a missing notification server must never get in the way of a download.
pub fn show(summary: String, body: String) {
    std::thread::spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.summary(&summary).body(&body).appname("StewardGet");
        #[cfg(windows)]
        if installed_shortcut() {
            n.app_id(APP_ID);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        n.icon("dev.stewardget.StewardGet");
        let _ = n.show();
    });
}

/// The system's "asterisk" sound, for a finished download (a freedesktop "complete"
/// sound on Linux, Glass on macOS). Silent where no player is installed.
pub fn beep() {
    #[cfg(windows)]
    // SAFETY: plain Win32 call without pointers; 0x40 is MB_ICONASTERISK.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0x40);
    }
    #[cfg(not(windows))]
    std::thread::spawn(|| {
        use std::process::{Command, Stdio};
        // The first player that exists wins; none of them is required.
        let players: &[(&str, &[&str])] = &[
            #[cfg(target_os = "macos")]
            ("afplay", &["/System/Library/Sounds/Glass.aiff"]),
            #[cfg(not(target_os = "macos"))]
            ("canberra-gtk-play", &["-i", "complete"]),
            #[cfg(not(target_os = "macos"))]
            ("paplay", &["/usr/share/sounds/freedesktop/stereo/complete.oga"]),
        ];
        for (program, args) in players {
            let started = Command::new(program)
                .args(*args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = started {
                let _ = child.wait();
                return;
            }
        }
    });
}

/// Whether the installer's Start-menu shortcut (which registers [`APP_ID`]) is
/// there. Without it, Windows drops toasts sent under that id, so the default
/// id is used instead.
#[cfg(windows)]
fn installed_shortcut() -> bool {
    let candidates = [
        ("APPDATA", r"Microsoft\Windows\Start Menu\Programs\StewardGet.lnk"),
        ("ProgramData", r"Microsoft\Windows\Start Menu\Programs\StewardGet.lnk"),
    ];
    candidates
        .iter()
        .any(|(var, rel)| std::env::var_os(var).is_some_and(|base| std::path::Path::new(&base).join(rel).is_file()))
}

/// Summary and body for downloads that just finished or failed, combined into
/// one notification when several end at once.
pub fn describe(finished: &[String], failed: &[String]) -> Option<(String, String)> {
    let list = |names: &[String]| {
        let mut text = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if names.len() > 3 {
            text.push_str(&format!(" and {} more", names.len() - 3));
        }
        text
    };
    match (finished.len(), failed.len()) {
        (0, 0) => None,
        (1, 0) => Some(("Download complete".into(), finished[0].clone())),
        (n, 0) => Some((format!("{n} downloads complete"), list(finished))),
        (0, 1) => Some(("Download failed".into(), failed[0].clone())),
        (0, n) => Some((format!("{n} downloads failed"), list(failed))),
        (a, b) => Some((
            format!("{a} complete, {b} failed"),
            format!("Complete: {}\nFailed: {}", list(finished), list(failed)),
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptions() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(super::describe(&[], &[]), None);
        assert_eq!(super::describe(&names(&["a.zip"]), &[]), Some(("Download complete".into(), "a.zip".into())));
        let (title, body) = super::describe(&names(&["a", "b", "c", "d", "e"]), &[]).unwrap();
        assert_eq!(title, "5 downloads complete");
        assert_eq!(body, "a, b, c and 2 more");
        assert_eq!(super::describe(&names(&["a"]), &names(&["b"])).unwrap().0, "1 complete, 1 failed");
    }
}
