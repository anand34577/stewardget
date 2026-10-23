//! The `.vxmeta` control file that makes partial downloads resumable, even
//! across crashes and from a different front-end.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsio;

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ControlFile {
    pub version: u32,
    pub url: String,
    pub total: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// `(start, end, written)` per segment.
    pub segments: Vec<(u64, u64, u64)>,
}

impl ControlFile {
    pub fn new(url: String, total: Option<u64>, etag: Option<String>, last_modified: Option<String>) -> Self {
        Self { version: VERSION, url, total, etag, last_modified, segments: Vec::new() }
    }

    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice::<Self>(&bytes).ok().filter(|c| c.version == VERSION)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        fsio::write_atomic(path, &serde_json::to_vec(self).expect("control file serialises"))
    }

    /// Any downloaded bytes recorded?
    pub fn has_data(&self) -> bool {
        self.segments.iter().any(|&(_, _, written)| written > 0)
    }

    /// Whether the remote file is the one this control file describes. One
    /// agreeing validator is enough: CDNs often vary the `ETag` between edge
    /// servers while `Last-Modified` stays put, and vice versa.
    pub fn matches(&self, total: Option<u64>, etag: Option<&str>, last_modified: Option<&str>) -> bool {
        if self.total != total || total.is_none() {
            return false;
        }
        let strip = |e: &str| e.trim_start_matches("W/").to_owned();
        let etag_eq = match (self.etag.as_deref(), etag) {
            (Some(a), Some(b)) => Some(strip(a) == strip(b)),
            _ => None,
        };
        let lm_eq = match (self.last_modified.as_deref(), last_modified) {
            (Some(a), Some(b)) => Some(a == b),
            _ => None,
        };
        match (etag_eq, lm_eq) {
            (Some(true), _) | (_, Some(true)) => true,
            (Some(false), _) | (_, Some(false)) => false,
            // Nothing comparable: only safe if neither side has validators.
            (None, None) => {
                self.etag.is_none() && etag.is_none() && self.last_modified.is_none() && last_modified.is_none()
            }
        }
    }
}

/// `<dest>.vxpart` – the partial data file.
pub(crate) fn part_path(dest: &Path) -> PathBuf {
    with_suffix(dest, ".vxpart")
}

/// `<dest>.vxmeta` – the control file.
pub(crate) fn meta_path(dest: &Path) -> PathBuf {
    with_suffix(dest, ".vxmeta")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validators() {
        let c = ControlFile::new("u".into(), Some(10), Some("\"abc\"".into()), Some("date".into()));
        assert!(c.matches(Some(10), Some("\"abc\""), Some("other-date")));
        assert!(c.matches(Some(10), Some("\"xyz\""), Some("date")), "same Last-Modified, edge-specific ETag");
        assert!(!c.matches(Some(10), Some("\"xyz\""), Some("other-date")));
        assert!(c.matches(Some(10), Some("W/\"abc\""), None));
        assert!(!c.matches(Some(11), Some("\"abc\""), Some("date")));
        let c = ControlFile::new("u".into(), Some(10), None, Some("date".into()));
        assert!(c.matches(Some(10), None, Some("date")));
        assert!(!c.matches(Some(10), None, None));
        assert!(!ControlFile::new("u".into(), None, None, None).matches(None, None, None));
    }
}
