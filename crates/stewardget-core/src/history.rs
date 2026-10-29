//! A permanent log of finished downloads.
//!
//! The download list is a working set: users remove entries to keep it short.
//! The history outlives that. Every download that completes or fails for good
//! is recorded here, so "that PDF from last month" can still be found, and
//! the statistics pages can chart what was downloaded and when.
//! Stored locally only, like everything else.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Category, Status};

/// Entries kept; the oldest are dropped beyond this.
pub const MAX_ENTRIES: usize = 5000;

/// One finished download.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Id of the download when it ran. Ids are never reused, so this also
    /// keeps a re-recorded download from appearing twice.
    pub id: u64,
    pub url: String,
    /// Host part of `url`, lower case; empty when the link has none.
    pub host: String,
    pub file_name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub category: Category,
    /// Size in bytes, if it was known.
    pub size: Option<u64>,
    /// [`Status::Completed`] or [`Status::Failed`].
    pub status: Status,
    pub error: Option<String>,
    pub added_at: u64,
    pub finished_at: u64,
    /// Seconds spent transferring.
    #[serde(default)]
    pub active_secs: u64,
    /// Connections used, as configured when it started.
    #[serde(default)]
    pub connections: usize,
    #[serde(default)]
    pub checksum_verified: bool,
    /// Free-form labels the user (or a rule) attached.
    #[serde(default)]
    pub tags: Vec<String>,
    /// What the post-download actions did, one line each.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl HistoryEntry {
    /// Mean speed while transferring, bytes/second.
    pub fn average_speed(&self) -> f64 {
        match (self.size, self.active_secs) {
            (Some(size), secs) if secs > 0 => size as f64 / secs as f64,
            _ => 0.0,
        }
    }

    /// True when every whitespace-separated word of `query` appears in the
    /// name, link, host, folder or a tag (case-insensitive).
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let hay = format!(
            "{}\n{}\n{}\n{}\n{}",
            self.file_name,
            self.url,
            self.host,
            self.path.display(),
            self.tags.join(" ")
        )
        .to_lowercase();
        query.split_whitespace().all(|word| hay.contains(word))
    }
}

/// Newest-last list of [`HistoryEntry`], bounded by [`MAX_ENTRIES`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    /// Adds an entry, replacing an earlier one for the same download (a failed
    /// download that later completed, or one restarted).
    pub fn record(&mut self, entry: HistoryEntry) {
        self.entries.retain(|e| e.id != entry.id);
        self.entries.push(entry);
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        if excess > 0 {
            self.entries.drain(..excess);
        }
    }

    pub fn remove(&mut self, ids: &[u64]) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| !ids.contains(&e.id));
        before - self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Replaces the tags of an entry.
    pub fn set_tags(&mut self, id: u64, tags: Vec<String>) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.tags = tags;
                true
            }
            None => false,
        }
    }

    /// Completed entries per hour of the (local) day, from `utc_offset_secs`.
    pub fn by_hour(&self, utc_offset_secs: i64) -> [u32; 24] {
        let mut hours = [0u32; 24];
        for e in self.entries.iter().filter(|e| e.status == Status::Completed) {
            let local = e.finished_at as i64 + utc_offset_secs;
            hours[(local.rem_euclid(86_400) / 3600) as usize] += 1;
        }
        hours
    }

    /// Hosts ranked by completed bytes: `(host, downloads, bytes, mean speed)`.
    pub fn top_hosts(&self, limit: usize) -> Vec<HostSummary> {
        let mut map: std::collections::HashMap<&str, HostSummary> = std::collections::HashMap::new();
        for e in self.entries.iter().filter(|e| e.status == Status::Completed && !e.host.is_empty()) {
            let s = map
                .entry(e.host.as_str())
                .or_insert_with(|| HostSummary { host: e.host.clone(), ..Default::default() });
            s.downloads += 1;
            s.bytes += e.size.unwrap_or(0);
            s.active_secs += e.active_secs;
        }
        let mut rows: Vec<_> = map.into_values().collect();
        rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.host.cmp(&b.host)));
        rows.truncate(limit);
        rows
    }
}

/// What the history knows about one host.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostSummary {
    pub host: String,
    pub downloads: usize,
    pub bytes: u64,
    pub active_secs: u64,
}

impl HostSummary {
    /// Mean speed over the host's downloads, bytes/second.
    pub fn average_speed(&self) -> f64 {
        if self.active_secs > 0 { self.bytes as f64 / self.active_secs as f64 } else { 0.0 }
    }
}

/// Lower-cased host of a link, without port or credentials.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, name: &str, host: &str, size: u64, secs: u64) -> HistoryEntry {
        HistoryEntry {
            id,
            url: format!("https://{host}/{name}"),
            host: host.into(),
            file_name: name.into(),
            path: PathBuf::from("/dl").join(name),
            category: Category::Other,
            size: Some(size),
            status: Status::Completed,
            error: None,
            added_at: 0,
            finished_at: 3600 * 5 + 10,
            active_secs: secs,
            connections: 8,
            checksum_verified: false,
            tags: vec![],
            notes: vec![],
        }
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("https://User:pw@Example.COM:8080/a/b?c#d"), "example.com");
        assert_eq!(host_of("http://[::1]:80/x"), "::1");
        assert_eq!(host_of("example.org/path"), "example.org");
    }

    #[test]
    fn record_replaces_same_download_and_is_bounded() {
        let mut h = History::default();
        h.record(entry(1, "a.zip", "x.com", 10, 1));
        let mut again = entry(1, "a.zip", "x.com", 20, 1);
        again.status = Status::Failed;
        h.record(again);
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].status, Status::Failed);

        for id in 2..(MAX_ENTRIES as u64 + 10) {
            h.record(entry(id, "f", "x.com", 1, 1));
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert!(h.entries.iter().all(|e| e.id != 1), "the oldest entries are dropped first");
    }

    #[test]
    fn search_matches_every_word_anywhere() {
        let mut e = entry(1, "Quarterly-Report.pdf", "files.corp.com", 1, 1);
        e.tags = vec!["work".into()];
        assert!(e.matches("report corp"));
        assert!(e.matches("WORK pdf"));
        assert!(!e.matches("report holiday"));
        assert!(e.matches("   "));
    }

    #[test]
    fn hour_histogram_and_host_ranking() {
        let mut h = History::default();
        h.record(entry(1, "a", "big.com", 1000, 10));
        h.record(entry(2, "b", "small.com", 10, 10));
        h.record(entry(3, "c", "big.com", 1000, 10));
        assert_eq!(h.by_hour(0)[5], 3);
        assert_eq!(h.by_hour(3600)[6], 3);
        let top = h.top_hosts(5);
        assert_eq!(top[0].host, "big.com");
        assert_eq!(top[0].downloads, 2);
        assert!((top[0].average_speed() - 100.0).abs() < 1e-9);
    }
}
