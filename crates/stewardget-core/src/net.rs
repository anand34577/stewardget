//! Network links and download lanes.
//!
//! A **link** is a way out to the internet: the default route, or one network
//! adapter (Wi-Fi, Ethernet, a phone's hotspot) that connections are bound to.
//! A **lane** is one link talking to one source (the file's own address or one
//! of its mirrors). A download spreads its connections over all its lanes; the
//! work-stealing segment table then gives each lane work in proportion to the
//! speed it can sustain, so a fast link carries more and a failing one drops
//! out without stopping the download.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::limiter::RateLimiter;

/// A network adapter the user may route downloads through (saved in settings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkLink {
    /// Local IP address connections are bound to; identifies the adapter.
    pub address: String,
    /// Name shown in the interface, e.g. "Wi-Fi".
    pub name: String,
    pub enabled: bool,
    /// Cap for this link in bytes/second, shared by all downloads; `0` = none.
    pub limit: u64,
}

impl Default for NetworkLink {
    fn default() -> Self {
        Self { address: String::new(), name: String::new(), enabled: true, limit: 0 }
    }
}

/// An adapter found on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub address: IpAddr,
}

/// Network adapters with a usable IPv4 address (loopback and link-local
/// addresses are skipped; IPv6 is left out because most servers and proxies
/// are reached over IPv4 and binding to the wrong family fails).
pub fn adapters() -> Vec<Adapter> {
    let mut found: Vec<Adapter> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_link_local() && !v4.is_unspecified() => {
                Some(Adapter { name: i.name, address: IpAddr::V4(v4) })
            }
            _ => None,
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name).then(a.address.cmp(&b.address)));
    found.dedup_by(|a, b| a.address == b.address);
    found
}

/// One way out: shared by every download, so a per-link cap and the live
/// numbers cover them all.
pub(crate) struct Link {
    pub label: String,
    pub address: Option<IpAddr>,
    pub client: Client,
    pub limiter: RateLimiter,
    /// Bytes carried since start.
    pub bytes: AtomicU64,
}

impl Link {
    pub fn new(label: impl Into<String>, address: Option<IpAddr>, client: Client, limit: u64) -> Self {
        Self { label: label.into(), address, client, limiter: RateLimiter::new(limit), bytes: AtomicU64::new(0) }
    }
}

/// What the engine knows about its links; replaced when the settings change.
pub(crate) struct LinkPool {
    pub links: Vec<Arc<Link>>,
}

/// Live state of one lane, shared with the engine's sampler.
pub(crate) struct LaneStat {
    pub link: String,
    pub host: String,
    pub mirror: bool,
    pub bytes: AtomicU64,
    /// Dropped after repeated failures.
    pub failed: AtomicBool,
    /// Connections currently using the lane.
    pub workers: AtomicUsize,
    /// Why it was dropped, for the interface.
    pub reason: std::sync::Mutex<Option<String>>,
}

impl LaneStat {
    pub fn new(link: &str, host: &str, mirror: bool) -> Self {
        Self {
            link: link.to_owned(),
            host: host.to_owned(),
            mirror,
            bytes: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            workers: AtomicUsize::new(0),
            reason: std::sync::Mutex::new(None),
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.failed.load(Ordering::Relaxed)
    }
}

/// A lane as shown to front-ends.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneView {
    /// The link used ("Wi-Fi", or "Default connection").
    pub link: String,
    /// The server it talks to.
    pub host: String,
    /// A mirror rather than the file's own address.
    pub mirror: bool,
    pub bytes: u64,
    /// Bytes/second, smoothed.
    pub speed: f64,
    pub workers: usize,
    pub failed: bool,
    pub reason: Option<String>,
}

/// A link as shown to front-ends.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkView {
    pub label: String,
    pub address: Option<String>,
    pub speed: f64,
    pub bytes: u64,
    pub limit: u64,
    /// Recent speed samples, oldest first.
    pub history: Vec<f32>,
}

/// Mirrors that serve the same file may differ in what they call it, but not
/// in its size; this is the check for "is this really the same file".
pub(crate) fn same_content(primary_total: Option<u64>, mirror_total: Option<u64>) -> bool {
    matches!((primary_total, mirror_total), (Some(a), Some(b)) if a == b && a > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_must_match_in_size() {
        assert!(same_content(Some(10), Some(10)));
        assert!(!same_content(Some(10), Some(11)));
        assert!(!same_content(None, Some(10)));
        assert!(!same_content(Some(0), Some(0)));
    }

    #[test]
    fn adapter_listing_skips_loopback() {
        assert!(adapters().iter().all(|a| !a.address.is_loopback()));
    }
}
