//! # stewardget-core
//!
//! The download engine that powers every StewardGet front-end (GUI, TUI and CLI).
//!
//! Highlights:
//!
//! * **Dynamic segmentation** – a file is split across several HTTP/1.1
//!   connections. When a connection finishes its range it *steals* half of the
//!   largest remaining range from a busy connection, so every connection stays
//!   busy until the very last byte.
//! * **Crash-safe resume** – progress is persisted in a small control file next
//!   to the partial download and is validated against `ETag`/`Last-Modified`
//!   before resuming.
//! * **Token-bucket rate limiting** – global and per-download, adjustable live.
//! * **Robust retries** – exponential back-off with jitter, per connection.
//! * **Integrity checks** – optional MD5/SHA-1/SHA-256/SHA-512 verification.
//! * **Browser integration** – a local, loopback-only API the browser
//!   extension hands downloads to, including expired-link *recapture*.
//! * **No telemetry** – the engine only ever talks to the servers you ask it to.
//!
//! ```no_run
//! # async fn demo() -> Result<(), stewardget_core::Error> {
//! use stewardget_core::{Engine, EngineOptions, NewDownload};
//!
//! let engine = Engine::start(EngineOptions::in_memory())?;
//! let id = engine.add(NewDownload::new("https://example.com/big.iso"))?;
//! let mut events = engine.subscribe();
//! while let Ok(event) = events.recv().await {
//!     if event.id() == id && event.is_terminal() {
//!         break;
//!     }
//! }
//! # Ok(()) }
//! ```

mod api;
mod category;
mod checksum;
mod config;
mod control;
mod crawl;
mod engine;
mod error;
pub mod extension;
mod filename;
mod fsio;
mod hls;
mod limiter;
pub mod mcp;
mod model;
mod probe;
pub mod remote;
mod segment;
mod stats;
mod store;
mod task;
pub mod util;

pub use category::{Category, CustomCategory};
pub use checksum::{Checksum, HashAlgo, hash_file};
pub use config::{
    Accent, CloseAction, Config, DEFAULT_API_PORT, ExistingFile, MAIN_QUEUE, Queue, QueueId, Schedule, SpeedSchedule,
    Theme,
};
pub use crawl::{GrabOptions, GrabResult};
pub use engine::{Engine, EngineOptions};
pub use error::Error;
pub use model::{
    ApiState, Capture, CaptureAction, DownloadId, DownloadOptions, DownloadView, EngineSnapshot, Event, ExtensionInfo,
    LinkInfo, NewDownload, SegmentView, Status,
};
pub use stats::{DayTotal, Stats};

/// Number of speed samples kept per download and globally (one every 500 ms).
pub const HISTORY_LEN: usize = 120;

/// Interval between speed samples.
pub const SAMPLE_INTERVAL_MS: u64 = 500;
