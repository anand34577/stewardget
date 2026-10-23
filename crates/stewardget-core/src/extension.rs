//! The browser extension, bundled into the binaries so every kind of install
//! (installer, package, archive or `cargo build`) can set it up the same way.

use std::path::{Path, PathBuf};

/// Name of the folder the Chromium build (Chrome, Edge, Brave, Opera, Vivaldi)
/// is unpacked into, inside the data directory.
pub const FOLDER: &str = "browser-extension";
/// Name of the folder the Firefox build is unpacked into, next to [`FOLDER`].
pub const FIREFOX_FOLDER: &str = "browser-extension-firefox";

// Chromium rejects `background.scripts` in Manifest V3 and Firefox does not run
// `background.service_worker`, so each gets its own manifest.
const CHROMIUM_MANIFEST: &[u8] = include_bytes!("../../../extension/manifest.json");
const FIREFOX_MANIFEST: &[u8] = include_bytes!("../../../extension/manifest.firefox.json");

/// Files shared by both builds.
const FILES: &[(&str, &[u8])] = &[
    ("background.js", include_bytes!("../../../extension/background.js")),
    ("content.js", include_bytes!("../../../extension/content.js")),
    ("popup.html", include_bytes!("../../../extension/popup.html")),
    ("popup.js", include_bytes!("../../../extension/popup.js")),
    ("README.md", include_bytes!("../../../extension/README.md")),
    ("icons/icon16.png", include_bytes!("../../../extension/icons/icon16.png")),
    ("icons/icon32.png", include_bytes!("../../../extension/icons/icon32.png")),
    ("icons/icon48.png", include_bytes!("../../../extension/icons/icon48.png")),
    ("icons/icon128.png", include_bytes!("../../../extension/icons/icon128.png")),
];

/// Writes the extension into `<data_dir>/browser-extension` (Chromium) and
/// `<data_dir>/browser-extension-firefox`, and returns the Chromium folder; see
/// [`firefox_folder`] for the other. Only files that differ are rewritten, so
/// calling this on every start-up is cheap and keeps an installed extension in
/// step with the app: the browser loads the new version the next time it starts.
pub fn unpack(data_dir: &Path) -> std::io::Result<PathBuf> {
    let root = data_dir.join(FOLDER);
    write_build(&root, CHROMIUM_MANIFEST)?;
    write_build(&data_dir.join(FIREFOX_FOLDER), FIREFOX_MANIFEST)?;
    Ok(root)
}

/// Velox, as the app was called before 0.7, kept the extension in its own data
/// folder, which has moved since. A browser that loaded it from there keeps it:
/// the Chromium build is kept in that folder too, up to date, while it exists.
pub fn refresh_legacy_copy() {
    if let Some(root) = crate::EngineOptions::legacy_data_dir().map(|d| d.join(FOLDER)).filter(|r| r.is_dir()) {
        keep_copy(&root);
    }
}

/// Writes the Chromium build into `root`, ignoring failures.
pub(crate) fn keep_copy(root: &Path) {
    let _ = write_build(root, CHROMIUM_MANIFEST);
}

/// The Firefox folder that belongs to the Chromium folder `root` returned by [`unpack`].
pub fn firefox_folder(root: &Path) -> PathBuf {
    root.with_file_name(FIREFOX_FOLDER)
}

fn write_build(root: &Path, manifest: &[u8]) -> std::io::Result<()> {
    for (name, bytes) in std::iter::once(&("manifest.json", manifest)).chain(FILES) {
        let path = root.join(name);
        if std::fs::read(&path).is_ok_and(|current| current == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    Ok(())
}

/// The folder the extension was unpacked into, if it is there.
pub fn location(data_dir: &Path) -> Option<PathBuf> {
    let root = data_dir.join(FOLDER);
    root.join("manifest.json").is_file().then_some(root)
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_the_app() {
        for bytes in [super::CHROMIUM_MANIFEST, super::FIREFOX_MANIFEST] {
            let manifest: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"), "bump extension/manifest*.json");
        }
    }

    #[test]
    fn each_browser_gets_a_manifest_it_accepts() {
        let chromium: serde_json::Value = serde_json::from_slice(super::CHROMIUM_MANIFEST).unwrap();
        let firefox: serde_json::Value = serde_json::from_slice(super::FIREFOX_MANIFEST).unwrap();
        assert_eq!(chromium["background"], serde_json::json!({ "service_worker": "background.js" }));
        assert!(chromium.get("browser_specific_settings").is_none());
        assert_eq!(firefox["background"], serde_json::json!({ "scripts": ["background.js"] }));
        assert!(firefox["browser_specific_settings"]["gecko"]["id"].is_string());
        // Apart from those keys the two must not drift apart.
        let strip = |mut v: serde_json::Value| {
            let map = v.as_object_mut().unwrap();
            map.remove("background");
            map.remove("browser_specific_settings");
            v
        };
        assert_eq!(strip(chromium), strip(firefox), "keep extension/manifest*.json in step");
    }

    #[test]
    fn unpack_writes_and_refreshes() {
        let dir = std::env::temp_dir().join(format!("stewardget-ext-{}", std::process::id()));
        let root = super::unpack(&dir).unwrap();
        assert!(root.join("icons/icon128.png").is_file());
        let firefox = super::firefox_folder(&root);
        assert_eq!(std::fs::read(firefox.join("manifest.json")).unwrap(), super::FIREFOX_MANIFEST);
        assert!(firefox.join("background.js").is_file());
        std::fs::write(root.join("popup.js"), "stale").unwrap();
        super::unpack(&dir).unwrap();
        assert_ne!(std::fs::read(root.join("popup.js")).unwrap(), b"stale");
        assert_eq!(super::location(&dir), Some(root));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
