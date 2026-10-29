//! Public data types shared between the engine and its front-ends.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Category, Checksum, QueueId, Stats};

pub type DownloadId = u64;

/// Lifecycle of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting for a free slot.
    Queued,
    /// Probing the server for size, name and range support.
    Connecting,
    Downloading,
    /// Hashing the finished file.
    Verifying,
    Paused,
    Completed,
    Failed,
}

impl Status {
    /// Transferring or about to transfer.
    pub fn is_active(self) -> bool {
        matches!(self, Status::Connecting | Status::Downloading | Status::Verifying)
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Status::Completed | Status::Failed)
    }

    pub fn can_pause(self) -> bool {
        matches!(self, Status::Queued | Status::Connecting | Status::Downloading)
    }

    pub fn can_resume(self) -> bool {
        matches!(self, Status::Paused | Status::Failed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "Queued",
            Status::Connecting => "Connecting",
            Status::Downloading => "Downloading",
            Status::Verifying => "Verifying",
            Status::Paused => "Paused",
            Status::Completed => "Completed",
            Status::Failed => "Failed",
        }
    }
}

/// Per-download options; everything is optional and falls back to [`crate::Config`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadOptions {
    /// Override the detected file name.
    pub file_name: Option<String>,
    /// Override the destination directory.
    pub directory: Option<PathBuf>,
    /// Override the number of connections.
    pub connections: Option<usize>,
    /// Per-download cap in bytes/second; `0` = unlimited.
    pub speed_limit: u64,
    /// Extra request headers (cookies, auth, referer…).
    pub headers: Vec<(String, String)>,
    /// Verify the finished file against this digest.
    pub checksum: Option<Checksum>,
    /// Page the download was started from; sent as `Referer` and used to
    /// re-open the page when a link has to be refreshed.
    pub referrer: Option<String>,
    /// Override the `User-Agent` (the browser's, for captured downloads).
    pub user_agent: Option<String>,
    /// Where the browser's request finally landed. The download uses its
    /// stable link first (a signed redirect expires within minutes) and this
    /// one only when that link no longer works (a one-time link).
    pub fallback_url: Option<String>,
    /// Other addresses of the same file. Connections are spread over the
    /// original and the mirrors; one that fails or turns out to be a different
    /// file is dropped and the others finish the download.
    pub mirrors: Vec<String>,
    /// Set when this file is one half of a video (picture or sound) that is
    /// joined with its other half once both have arrived.
    pub mux: Option<crate::Mux>,
    /// What to do if the file name is taken in the folder; the setting's
    /// choice when `None`.
    pub existing: Option<crate::ExistingFile>,
}

/// A request to add a download.
#[derive(Debug, Clone, Default)]
pub struct NewDownload {
    pub url: String,
    pub options: DownloadOptions,
    /// Add in the paused state instead of queueing immediately.
    pub paused: bool,
    /// Queue to join; the main queue when `None`.
    pub queue: Option<QueueId>,
}

impl NewDownload {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), ..Default::default() }
    }
}

/// A download handed over by a browser (or any API client).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    pub url: String,
    /// File name suggested by the browser.
    pub file_name: Option<String>,
    /// Size reported by the browser, if known.
    pub size: Option<u64>,
    /// The address the browser's request ended at after redirects, when it
    /// differs from `url`; tried when `url` no longer works.
    pub fallback_url: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    /// `Cookie` header value for the URL.
    pub cookies: Option<String>,
    pub headers: Vec<(String, String)>,
    pub directory: Option<PathBuf>,
    pub paused: bool,
    /// Queue to join; the main queue when `None`.
    pub queue: Option<QueueId>,
    /// Add it straight away even when the desktop app would ask first
    /// (`stewardget add` from scripts).
    pub direct: bool,
}

/// What the engine did with a [`Capture`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureAction {
    /// A new download was queued.
    Added,
    /// The link of a download that was waiting for one was replaced; it resumes.
    Refreshed,
    /// Handed to the desktop app, which asks the user before adding it.
    Pending,
}

/// State of the local browser-integration API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiState {
    Disabled,
    Listening(u16),
    Failed(String),
}

/// Progress of one segment, for visualisation. Offsets are absolute bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentView {
    pub start: u64,
    pub end: u64,
    pub done: u64,
    pub active: bool,
}

/// A point-in-time view of a download, cheap to clone and render.
#[derive(Debug, Clone)]
pub struct DownloadView {
    pub id: DownloadId,
    pub url: String,
    pub file_name: String,
    pub path: PathBuf,
    pub category: Category,
    /// The queue it belongs to (always one that exists).
    pub queue: QueueId,
    pub status: Status,
    pub total: Option<u64>,
    pub downloaded: u64,
    /// Smoothed speed in bytes/second.
    pub speed: f64,
    /// Seconds remaining at the current speed.
    pub eta: Option<u64>,
    pub connections: usize,
    /// Connections waiting to retry after a network error.
    pub reconnecting: usize,
    /// Seconds spent transferring (not queued or paused), for "finished in".
    pub active_secs: u64,
    pub resumable: bool,
    pub segments: Vec<SegmentView>,
    /// Recent speed samples, oldest first. In [`crate::Engine::snapshot`] only
    /// running downloads carry them; [`crate::Engine::download`] always does.
    pub history: Vec<f32>,
    pub error: Option<String>,
    pub added_at: u64,
    pub finished_at: Option<u64>,
    pub options: DownloadOptions,
    pub checksum_verified: bool,
    /// Waiting for the browser to capture a fresh link (see [`crate::Engine::await_new_link`]).
    pub awaiting_link: bool,
    /// Seconds until the next automatic retry of a failed download.
    pub retry_in: Option<u64>,
    /// Automatic retries used so far.
    pub retries: u32,
    /// Where connections go: one entry per (network link, source) pair.
    pub lanes: Vec<crate::net::LaneView>,
    /// Connections the auto-tuner is aiming for while it ramps up; equals
    /// the configured count once it settled.
    pub tuned_connections: Option<usize>,
}

/// What a server says about a link, before downloading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    /// The file name StewardGet would use.
    pub file_name: String,
    pub size: Option<u64>,
    /// Supports byte ranges: segmented, pausable and resumable.
    pub resumable: bool,
    pub content_type: Option<String>,
    /// The URL after redirects.
    pub final_url: String,
}

impl DownloadView {
    /// Completion in `0.0..=1.0`, if the size is known.
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(0) => Some(1.0),
            Some(t) => Some((self.downloaded as f64 / t as f64).clamp(0.0, 1.0) as f32),
            None if self.status == Status::Completed => Some(1.0),
            None => None,
        }
    }
}

/// Everything a dashboard needs, gathered under one lock.
#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    pub downloads: Vec<DownloadView>,
    /// Aggregate speed, bytes/second.
    pub speed: f64,
    /// Aggregate speed samples, oldest first.
    pub history: Vec<f32>,
    pub speed_limit: u64,
    /// The time-of-day limit ([`crate::SpeedSchedule`]) in force instead of `speed_limit`.
    pub scheduled_limit: Option<u64>,
    pub max_concurrent: usize,
    pub stats: Stats,
    pub api: ApiState,
    /// The browser extension that last talked to the API, if any did since start-up.
    pub extension: Option<ExtensionInfo>,
}

/// A browser extension seen on the local API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInfo {
    /// Browser name as reported by the extension, e.g. `Chrome` or `Firefox`.
    pub browser: String,
    /// Version of the extension.
    pub version: String,
    /// Seconds since its last request.
    pub seen_secs: u64,
}

impl EngineSnapshot {
    pub fn count(&self, pred: impl Fn(Status) -> bool) -> usize {
        self.downloads.iter().filter(|d| pred(d.status)).count()
    }
}

/// Notifications broadcast by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Added(DownloadId),
    StatusChanged(DownloadId, Status),
    Removed(DownloadId),
}

impl Event {
    pub fn id(&self) -> DownloadId {
        match *self {
            Event::Added(id) | Event::StatusChanged(id, _) | Event::Removed(id) => id,
        }
    }

    /// The download reached a state it won't leave on its own.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Event::StatusChanged(_, Status::Completed | Status::Failed | Status::Paused) | Event::Removed(_))
    }
}
