//! The engine: owns the queue, schedules downloads, samples speeds and
//! persists state. Cheap to clone; every method is non-blocking.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use reqwest::Client;
use reqwest::header::{self, HeaderMap, HeaderValue};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::control::{ControlFile, meta_path, part_path};
use crate::limiter::RateLimiter;
use crate::model::{
    ApiState, Capture, CaptureAction, DownloadView, EngineSnapshot, Event, ExtensionInfo, LinkInfo, NewDownload,
    SegmentView,
};
use crate::net::{LaneView, Link, LinkPool, LinkView};
use crate::probe::Probe;
use crate::store::{self, DownloadList, Record, Store};
use crate::task::{self, Hooks, Job, Outcome, Shared};
use crate::{
    Category, Config, DownloadId, DownloadOptions, Error, ExistingFile, HISTORY_LEN, History, HistoryEntry, MAIN_QUEUE,
    QueueId, SAMPLE_INTERVAL_MS, Stats, Status,
};
use crate::{filename, fsio, util};

/// EWMA smoothing factor applied to each 500 ms speed sample.
const SPEED_ALPHA: f64 = 0.3;
/// The download list is saved at most every this many samples (2 s)...
const PERSIST_EVERY: u64 = 4;
/// ...and the statistics, which change constantly, every this many (10 s).
const PERSIST_STATS_EVERY: u64 = 20;

/// How to construct an [`Engine`].
#[derive(Debug, Clone, Default)]
pub struct EngineOptions {
    /// Directory for settings, the download list and stats. `None` keeps
    /// everything in memory.
    pub data_dir: Option<PathBuf>,
    /// Use this configuration instead of the stored one.
    pub config: Option<Config>,
}

impl EngineOptions {
    /// Persistent state in the platform's standard data directory.
    pub fn persistent() -> Self {
        Self { data_dir: Some(Self::default_data_dir()), config: None }
    }

    /// Nothing is written besides the downloaded files themselves.
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// `$STEWARDGET_DATA_DIR` if set (portable installs), otherwise
    /// `%APPDATA%\stewardget\StewardGet\data`, `~/Library/Application Support/dev.stewardget.StewardGet`,
    /// `$XDG_DATA_HOME/stewardget`…
    ///
    /// Before 0.7 the app was called Velox: `$VELOX_DATA_DIR` still counts, and
    /// its data folder is moved here the first time (see [`store::adopt_legacy_dir`]).
    pub fn default_data_dir() -> PathBuf {
        let env = ["STEWARDGET_DATA_DIR", "VELOX_DATA_DIR"]
            .iter()
            .find_map(|v| std::env::var_os(v).filter(|d| !d.is_empty()));
        if let Some(dir) = env {
            return PathBuf::from(dir);
        }
        let Some(dirs) = directories::ProjectDirs::from("dev", "stewardget", "StewardGet") else {
            return PathBuf::from(".stewardget");
        };
        match Self::legacy_data_dir() {
            Some(legacy) => store::adopt_legacy_dir(&legacy, dirs.data_dir()),
            None => dirs.data_dir().to_path_buf(),
        }
    }

    /// Where Velox, as the app was called before 0.7, kept its data.
    pub fn legacy_data_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "velox", "Velox").map(|d| d.data_dir().to_path_buf())
    }

    /// Reads settings without starting an engine or taking the lock.
    pub fn peek_config(dir: &Path) -> Option<Config> {
        store::peek_config(dir)
    }

    /// Reads stats without starting an engine or taking the lock.
    pub fn peek_stats(dir: &Path) -> Option<Stats> {
        store::peek_stats(dir)
    }

    /// Reads the download list without starting an engine or taking the lock.
    pub fn peek_downloads(dir: &Path) -> Vec<DownloadView> {
        store::peek_downloads(dir)
            .map(|l| l.downloads.into_iter().map(|rec| Item::new(rec).view()).collect())
            .unwrap_or_default()
    }
}

/// Handle to the download engine. Clone freely; all clones share state.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    runtime: Handle,
    store: Store,
    config: RwLock<Config>,
    client: RwLock<Client>,
    /// Ways out to the network; several when adapters are bonded.
    links: RwLock<Arc<LinkPool>>,
    /// Live numbers per link, by position in `links`.
    link_stats: Mutex<Vec<LinkStat>>,
    /// Video/audio groups being joined right now.
    muxing: Mutex<HashSet<String>>,
    /// The monthly data cap stopped the downloads.
    cap_hit: AtomicBool,
    global_limiter: Arc<RateLimiter>,
    /// Bandwidth shared by each queue's downloads ([`crate::Queue::speed_limit`]).
    queue_limiters: Mutex<HashMap<QueueId, Arc<RateLimiter>>>,
    /// The time-of-day speed limit is in force ([`crate::SpeedSchedule`]).
    scheduled_limit: AtomicBool,
    state: Mutex<State>,
    /// One save at a time: they share temporary file names.
    persist_lock: Mutex<()>,
    /// Settings changes are read-modify-write: one at a time.
    config_lock: Mutex<()>,
    events: broadcast::Sender<Event>,
    shutdown: CancellationToken,
    shutting_down: AtomicBool,
    api: Mutex<ApiRuntime>,
    /// No settings file existed when the engine started.
    first_run: bool,
    /// Last request from the browser extension: when, browser, version.
    extension: Mutex<Option<(Instant, String, String)>>,
    /// Brings the front-end's window forward (set by the desktop app).
    show_handler: Mutex<Option<ShowHandler>>,
    /// Asks the user about browser downloads before adding them (set by the desktop app).
    capture_handler: Mutex<Option<CaptureHandler>>,
    /// Closes the front-end (set by the desktop app).
    quit_handler: Mutex<Option<ShowHandler>>,
}

type ShowHandler = Arc<dyn Fn() + Send + Sync>;
type CaptureHandler = Arc<dyn Fn(Vec<NewDownload>) + Send + Sync>;

/// Smoothed speed and history of one link.
#[derive(Default)]
struct LinkStat {
    last_bytes: u64,
    speed: f64,
    history: VecDeque<f32>,
}

struct ApiRuntime {
    state: ApiState,
    stop: Option<CancellationToken>,
    /// `(enabled, port, token)` the server was started with.
    applied: Option<(bool, u16, Option<String>)>,
}

struct State {
    items: Vec<Item>,
    next_id: u64,
    dirty: bool,
    /// Only the statistics changed (bytes counted); saved less often.
    stats_dirty: bool,
    stats: Stats,
    /// Permanent log of finished downloads.
    log: History,
    log_dirty: bool,
    /// Bumped whenever `log` changes, so front-ends can cache what they show.
    log_gen: u64,
    speed: f64,
    history: VecDeque<f32>,
    /// Last local `(day, minute)` the scheduler looked at.
    schedule_minute: Option<(i64, u16)>,
}

struct Item {
    rec: Record,
    run: Option<Running>,
    speed: f64,
    history: VecDeque<f32>,
    last_transferred: u64,
    connections: usize,
    reconnecting: usize,
    segments: Vec<SegmentView>,
    /// Automatic retries used since the user last touched the download.
    auto_retries: u32,
    /// Don't start before this instant (automatic retry back-off).
    retry_at: Option<Instant>,
    /// Bytes received when the user paused, shown until the task has stopped.
    frozen: Option<u64>,
    /// Bytes each lane had carried at the last sample, and its smoothed speed.
    lane_prev: Vec<u64>,
    lane_speed: Vec<f64>,
}

struct Running {
    cancel: CancellationToken,
    shared: Arc<Shared>,
    on_stop: AfterStop,
}

/// What to do once a cancelled task has wound down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterStop {
    Pause,
    Requeue,
    Restart,
    Remove { delete_file: bool },
}

impl Item {
    fn new(rec: Record) -> Self {
        Self {
            rec,
            run: None,
            speed: 0.0,
            history: VecDeque::with_capacity(HISTORY_LEN),
            last_transferred: 0,
            connections: 0,
            reconnecting: 0,
            segments: Vec::new(),
            auto_retries: 0,
            retry_at: None,
            frozen: None,
            lane_prev: Vec::new(),
            lane_speed: Vec::new(),
        }
    }

    fn waiting_for_retry(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|t| t > now)
    }

    fn view(&self) -> DownloadView {
        self.view_with(true)
    }

    /// `history`: include the speed history even when the download isn't
    /// running. Snapshots leave it out: they are taken every frame, and
    /// copying 120 samples for each of thousands of finished downloads adds up.
    fn view_with(&self, history: bool) -> DownloadView {
        let rec = &self.rec;
        let (downloaded, total, segments) = match &self.run {
            // Paused: freeze the numbers while the connections close.
            Some(_) if rec.status == Status::Paused && self.frozen.is_some() => {
                (self.frozen.unwrap_or(rec.downloaded), rec.total, self.segments.clone())
            }
            Some(run) => {
                let t = run.shared.table.lock().unwrap();
                let total = run.shared.total().or(rec.total);
                if t.segments.is_empty() {
                    (rec.downloaded, total, Vec::new())
                } else {
                    (t.received(), total, t.views())
                }
            }
            None => (rec.downloaded, rec.total, self.segments.clone()),
        };
        let eta = match total {
            Some(t) if self.speed > 1.0 && rec.status == Status::Downloading => {
                Some((t.saturating_sub(downloaded) as f64 / self.speed).ceil() as u64)
            }
            _ => None,
        };
        DownloadView {
            id: rec.id,
            url: rec.url.clone(),
            file_name: rec.file_name.clone(),
            path: rec.path.clone(),
            category: rec.category,
            queue: rec.queue,
            status: rec.status,
            total,
            downloaded,
            // A paused download's task may still be closing its connections.
            speed: if matches!(rec.status, Status::Downloading | Status::Connecting) { self.speed } else { 0.0 },
            eta,
            connections: self.connections,
            reconnecting: self.reconnecting,
            active_secs: rec.active_secs.round() as u64,
            resumable: rec.resumable,
            segments,
            history: if history || self.run.is_some() { self.history.iter().copied().collect() } else { Vec::new() },
            error: rec.error.clone(),
            added_at: rec.added_at,
            finished_at: rec.finished_at,
            options: rec.options.clone(),
            checksum_verified: rec.checksum_verified,
            awaiting_link: rec.awaiting_link,
            retry_in: self
                .retry_at
                .filter(|_| rec.status == Status::Queued)
                .map(|t| t.saturating_duration_since(Instant::now()).as_secs_f64().ceil() as u64),
            retries: self.auto_retries,
            lanes: self.lane_views(),
            tuned_connections: self
                .run
                .as_ref()
                .map(|r| r.shared.target_connections.load(Ordering::Relaxed))
                .filter(|&n| n > 0),
        }
    }

    fn lane_views(&self) -> Vec<LaneView> {
        let Some(run) = &self.run else { return Vec::new() };
        let lanes = run.shared.lanes.lock().unwrap();
        lanes
            .iter()
            .enumerate()
            .map(|(i, l)| LaneView {
                link: l.link.clone(),
                host: l.host.clone(),
                mirror: l.mirror,
                bytes: l.bytes.load(Ordering::Relaxed),
                speed: self.lane_speed.get(i).copied().unwrap_or(0.0),
                workers: l.workers.load(Ordering::Relaxed),
                failed: !l.is_alive(),
                reason: l.reason.lock().unwrap().clone(),
            })
            .collect()
    }
}

impl Engine {
    /// Starts the engine. Must be called from within a Tokio runtime.
    pub fn start(options: EngineOptions) -> Result<Self, Error> {
        let runtime = Handle::try_current()
            .map_err(|_| Error::Io("Engine::start must be called inside a Tokio runtime".into()))?;
        let store = match options.data_dir {
            Some(dir) => Store::open(dir)?,
            None => Store::memory(),
        };
        let stored = store.load_config();
        let had_control_token =
            stored.as_ref().and_then(|c| c.control_token.as_deref()).is_some_and(|t| t.trim().len() >= 16);
        let first_run = stored.is_none() && store.dir().is_some();
        let config = options.config.or(stored).unwrap_or_default().sanitized();
        if !had_control_token && store.dir().is_some() {
            // The command line and installer read it from the settings file.
            let _ = store.save_config(&config);
        }
        let mut list = store.load_downloads().unwrap_or_default();
        for rec in &mut list.downloads {
            rec.queue = valid_queue(&config, rec.queue);
            rec.category = config.categorize(&rec.file_name);
        }
        let stats = store.load_stats().unwrap_or_default();
        // Lists from before the history existed: start it from what they hold.
        let (log, log_dirty) = match store.load_history() {
            Some(log) => (log, false),
            None => {
                let mut log = History::default();
                let mut done: Vec<_> = list.downloads.iter().filter(|r| r.status == Status::Completed).collect();
                done.sort_by_key(|r| r.finished_at.unwrap_or(r.added_at));
                for rec in done {
                    log.record(history_entry(rec, config.connections));
                }
                let seeded = !log.entries.is_empty();
                (log, seeded)
            }
        };

        let items: Vec<Item> = list
            .downloads
            .into_iter()
            .map(|mut rec| {
                // Anything interrupted mid-flight is picked up again (or parked).
                if rec.status.is_active() || rec.status == Status::Queued {
                    rec.status = if config.resume_on_start { Status::Queued } else { Status::Paused };
                }
                Item::new(rec)
            })
            .collect();
        let next_id = list.next_id.max(items.iter().map(|i| i.rec.id + 1).max().unwrap_or(1));

        let inner = Arc::new(Inner {
            runtime,
            client: RwLock::new(build_client(&config)?),
            links: RwLock::new(Arc::new(build_pool(&config, &build_client(&config)?))),
            link_stats: Mutex::new(Vec::new()),
            muxing: Mutex::new(HashSet::new()),
            cap_hit: AtomicBool::new(false),
            global_limiter: Arc::new(RateLimiter::new(config.speed_limit)),
            queue_limiters: Mutex::new(HashMap::new()),
            scheduled_limit: AtomicBool::new(false),
            config: RwLock::new(config),
            store,
            persist_lock: Mutex::new(()),
            config_lock: Mutex::new(()),
            state: Mutex::new(State {
                items,
                next_id,
                dirty: false,
                stats_dirty: false,
                stats,
                log,
                log_dirty,
                log_gen: 1,
                speed: 0.0,
                history: VecDeque::with_capacity(HISTORY_LEN),
                schedule_minute: None,
            }),
            events: broadcast::channel(512).0,
            shutdown: CancellationToken::new(),
            shutting_down: AtomicBool::new(false),
            api: Mutex::new(ApiRuntime { state: ApiState::Disabled, stop: None, applied: None }),
            first_run,
            extension: Mutex::new(None),
            show_handler: Mutex::new(None),
            capture_handler: Mutex::new(None),
            quit_handler: Mutex::new(None),
        });
        inner.apply_speed_limits();
        inner.runtime.spawn(sampler(Arc::downgrade(&inner)));
        // Only the persistent (single-instance) engine serves the browser API.
        if inner.store.dir().is_some() {
            inner.apply_api_config();
        }
        inner.schedule();
        Ok(Self { inner })
    }

    pub(crate) fn from_inner(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    /// Handles a download handed over by the browser extension (or any API
    /// client). If a download is waiting for a fresh link and this capture
    /// looks like the same file (same size, or same name when a size is
    /// unknown), its link is replaced and it resumes where it stopped.
    /// Otherwise a new download is queued, or, when the desktop app asks first
    /// (see [`Engine::set_capture_handler`]), handed to it.
    pub fn capture(&self, capture: Capture) -> Result<(DownloadId, CaptureAction), Error> {
        Ok(self.capture_many(vec![capture])?.remove(0))
    }

    /// [`Engine::capture`] for several downloads at once, e.g. every link on a
    /// page. The desktop app gets them together so it can offer one list.
    /// Nothing is added when one of the links is invalid.
    pub fn capture_many(&self, captures: Vec<Capture>) -> Result<Vec<(DownloadId, CaptureAction)>, Error> {
        for c in &captures {
            parse_url(&c.url)?;
        }
        let ask = self.config().capture_dialog;
        let handler = self.inner.capture_handler.lock().unwrap().clone().filter(|_| ask);
        let mut results = Vec::with_capacity(captures.len());
        let mut pending = Vec::new();
        for capture in captures {
            if let Some(id) = self.refresh_waiting(&capture)? {
                results.push((id, CaptureAction::Refreshed));
                continue;
            }
            let ask = handler.is_some() && !capture.direct;
            let new = self.capture_request(capture)?;
            if ask {
                pending.push(new);
                results.push((0, CaptureAction::Pending));
            } else {
                results.push((self.add(new)?, CaptureAction::Added));
            }
        }
        if let Some(handler) = handler.filter(|_| !pending.is_empty()) {
            handler(pending);
        }
        Ok(results)
    }

    /// The download a capture would add: the browser's file name, cookies,
    /// referrer and user agent, ready for [`Engine::add`].
    pub fn capture_request(&self, capture: Capture) -> Result<NewDownload, Error> {
        let url = parse_url(&capture.url)?;
        let mut options = DownloadOptions {
            // Only a name the browser really chose is fixed here; otherwise the
            // server's name (found when the download starts) is used.
            file_name: browser_name(&capture, &url),
            directory: capture.directory,
            headers: capture.headers,
            referrer: capture.referrer,
            user_agent: capture.user_agent,
            fallback_url: capture
                .fallback_url
                .as_deref()
                .and_then(|f| parse_url(f).ok())
                .filter(|f| *f != url)
                .map(|f| f.to_string()),
            ..Default::default()
        };
        merge_cookie(&mut options.headers, capture.cookies.as_deref());
        Ok(NewDownload { url: url.to_string(), options, paused: capture.paused, queue: capture.queue })
    }

    /// Gives a capture to the download that waits for a fresh link of the same file.
    fn refresh_waiting(&self, capture: &Capture) -> Result<Option<DownloadId>, Error> {
        let url = parse_url(&capture.url)?;
        let hinted_name = hinted_name(capture, &url);
        let waiting = {
            let st = self.inner.lock();
            st.items
                .iter()
                .filter(|i| i.rec.awaiting_link)
                .find(|i| {
                    let same_size = capture.size.is_some() && capture.size == i.rec.total;
                    let same_name = hinted_name.as_deref().is_some_and(|n| same_file_name(n, &i.rec.file_name));
                    // A size alone could be any other file; resuming with it would corrupt the partial data.
                    (same_size && (same_name || hinted_name.is_none()))
                        || (same_name && (capture.size.is_none() || i.rec.total.is_none()))
                })
                .map(|i| i.rec.id)
        };
        if let Some(id) = waiting {
            self.inner.with_item(id, |item, _| {
                if capture.referrer.is_some() {
                    item.rec.options.referrer = capture.referrer.clone();
                }
                if capture.user_agent.is_some() {
                    item.rec.options.user_agent = capture.user_agent.clone();
                }
                merge_cookie(&mut item.rec.options.headers, capture.cookies.as_deref());
            })?;
            self.set_url(id, url.as_str())?;
            return Ok(Some(id));
        }
        Ok(None)
    }

    /// Replaces the link of a download and resumes it from its partial data,
    /// as long as the new link serves a file of the same size.
    pub fn set_url(&self, id: DownloadId, url: &str) -> Result<(), Error> {
        let url = parse_url(url)?;
        self.inner.with_item(id, |item, inner| {
            if item.rec.status == Status::Completed {
                return;
            }
            item.rec.url = url.to_string();
            item.rec.awaiting_link = false;
            item.rec.relaxed_resume = true;
            item.rec.error = None;
            match &mut item.run {
                Some(run) => {
                    run.on_stop = AfterStop::Requeue;
                    run.cancel.cancel();
                }
                None => inner.set_status(item, Status::Queued),
            }
        })?;
        self.inner.schedule();
        Ok(())
    }

    /// Marks a download as waiting for a fresh link (e.g. it expired). The
    /// next matching browser capture is attached to it and it resumes.
    /// Pass `false` to stop waiting.
    pub fn await_new_link(&self, id: DownloadId, waiting: bool) -> Result<(), Error> {
        self.inner.with_item(id, |item, inner| {
            item.rec.awaiting_link = waiting && item.rec.status != Status::Completed;
            if item.rec.awaiting_link
                && let Some(run) = &mut item.run
            {
                run.on_stop = AfterStop::Pause;
                item.frozen = Some(run.shared.table.lock().unwrap().received());
                run.cancel.cancel();
                inner.set_status(item, Status::Paused);
            }
        })
    }

    /// Asks the server about a link without downloading it: final file name,
    /// size and whether it can be resumed. Used by "Add download" dialogs.
    pub async fn check_link(&self, url: &str, options: &DownloadOptions) -> Result<LinkInfo, Error> {
        let url = parse_url(url)?;
        let client = self.inner.client.read().unwrap().clone();
        let headers = task::request_headers(options);
        let look = |url: Url| {
            let (client, headers) = (client.clone(), headers.clone());
            async move {
                tokio::time::timeout(Duration::from_secs(20), crate::probe::probe(&client, &url, &headers))
                    .await
                    .map_err(|_| Error::Timeout)?
            }
        };
        // Like the download itself: the browser's final address when the link is used up.
        let fallback = options.fallback_url.as_deref().and_then(|u| Url::parse(u).ok()).filter(|u| *u != url);
        let mut probe = match look(url.clone()).await {
            Err(e @ Error::Http(400..=499)) => match fallback {
                Some(f) => look(f).await.map_err(|_| e)?,
                None => return Err(e),
            },
            other => other?,
        };
        if crate::hls::is_playlist(&url, &probe) {
            let ext =
                tokio::time::timeout(Duration::from_secs(20), crate::hls::inspect(&client, &headers, &probe.final_url))
                    .await
                    .map_err(|_| Error::Timeout)??;
            probe.stream_ext = Some(ext);
            return Ok(LinkInfo {
                file_name: resolve_name(options, &probe, &url),
                size: None,
                // Saved piece by piece, so it can be paused and resumed.
                resumable: true,
                content_type: Some("HLS stream".into()),
                final_url: probe.final_url.to_string(),
            });
        }
        Ok(LinkInfo {
            file_name: resolve_name(options, &probe, &url),
            size: probe.total,
            resumable: probe.ranges && probe.total.is_some_and(|t| t > 0),
            content_type: probe.content_type.clone(),
            final_url: probe.final_url.to_string(),
        })
    }

    /// Continues a download whose server reports a different file version
    /// (validators changed) using the data already on disk. Only the size
    /// still has to match.
    pub fn resume_anyway(&self, id: DownloadId) -> Result<(), Error> {
        self.inner.with_item(id, |item, _| item.rec.relaxed_resume = true)?;
        self.resume(id)
    }

    /// Reads a web page (and, with `options.depth`, the pages it links to) and
    /// returns the files it links to. See [`crate::GrabOptions`].
    pub async fn grab_links(&self, url: &str, options: &crate::GrabOptions) -> Result<crate::GrabResult, Error> {
        let client = self.inner.client.read().unwrap().clone();
        crate::crawl::grab(&client, &self.config(), url, options).await
    }

    /// Replaces a download's options (headers, referrer, connections, limits,
    /// checksum...). A running download picks them up by reconnecting; its data
    /// is kept. Name and folder are changed with [`Engine::relocate`].
    pub fn update_options(&self, id: DownloadId, mut options: DownloadOptions) -> Result<(), Error> {
        self.inner.with_item(id, |item, _| {
            options.file_name = item.rec.options.file_name.clone();
            options.directory = item.rec.options.directory.clone();
            // Not shown in any dialog: kept as the browser gave it.
            options.fallback_url = item.rec.options.fallback_url.clone();
            let reconnect = options.headers != item.rec.options.headers
                || options.referrer != item.rec.options.referrer
                || options.user_agent != item.rec.options.user_agent
                || options.connections != item.rec.options.connections
                // The task verifies with the checksum it started with.
                || options.checksum != item.rec.options.checksum;
            if let Some(run) = &mut item.run {
                run.shared.limiter.set_rate(options.speed_limit);
                if reconnect {
                    run.on_stop = AfterStop::Requeue;
                    run.cancel.cancel();
                }
            }
            item.rec.options = options;
        })?;
        self.inner.schedule();
        Ok(())
    }

    /// Renames and/or moves a download that isn't running. Completed files
    /// and partial data are moved on disk (across drives if needed).
    pub fn relocate(&self, id: DownloadId, directory: PathBuf, file_name: &str) -> Result<(), Error> {
        let name = filename::sanitize(file_name.trim());
        if file_name.trim().is_empty() {
            return Err(Error::Io("the file name is empty".into()));
        }
        let category = self.config().categorize(&name);
        let mut st = self.inner.lock();
        let reserved: Vec<PathBuf> = st.items.iter().filter(|i| i.rec.id != id).map(|i| i.rec.path.clone()).collect();
        let item = st.items.iter_mut().find(|i| i.rec.id == id).ok_or(Error::NotFound)?;
        if item.run.is_some() {
            return Err(Error::Io("pause the download before moving or renaming it".into()));
        }
        let to = directory.join(&name);
        if to == item.rec.path {
            return Ok(());
        }
        // On case-insensitive file systems `a.zip` → `A.zip` finds itself "already there".
        let case_only = cfg!(any(windows, target_os = "macos"))
            && to.as_os_str().to_string_lossy().to_lowercase()
                == item.rec.path.as_os_str().to_string_lossy().to_lowercase();
        if !case_only
            && (to.exists() || part_path(&to).exists() || reserved.iter().any(|r| crate::fsio::same_path(r, &to)))
        {
            return Err(Error::Io(format!("\u{201c}{name}\u{201d} already exists in that folder")));
        }
        let from = item.rec.path.clone();
        if item.rec.status == Status::Completed {
            if from.exists() {
                fsio::move_file(&from, &to)?;
            }
        } else if item.rec.path_locked {
            if part_path(&from).exists() {
                fsio::move_file(&part_path(&from), &part_path(&to))?;
            }
            if meta_path(&from).exists() {
                fsio::move_file(&meta_path(&from), &meta_path(&to))?;
            }
        }
        item.rec.path = to;
        item.rec.file_name = name.clone();
        item.rec.category = category;
        item.rec.options.file_name = Some(name);
        item.rec.options.directory = Some(directory);
        st.dirty = true;
        Ok(())
    }

    /// Queues a new download. The URL may omit `https://`.
    pub fn add(&self, new: NewDownload) -> Result<DownloadId, Error> {
        let url = parse_url(&new.url)?;
        let config = self.config();
        let name = filename::sanitize(
            &new.options.file_name.clone().or_else(|| filename::from_url(&url)).unwrap_or_else(|| "download".into()),
        );
        let category = config.categorize(&name);
        let dir = target_dir(&config, &new.options, category);
        let queue = valid_queue(&config, new.queue.unwrap_or(MAIN_QUEUE));
        let id = {
            let mut st = self.inner.lock();
            let id = st.next_id;
            st.next_id += 1;
            st.items.push(Item::new(Record {
                id,
                url: url.to_string(),
                path: dir.join(&name),
                file_name: name,
                path_locked: false,
                category,
                queue,
                status: if new.paused { Status::Paused } else { Status::Queued },
                total: None,
                downloaded: 0,
                resumable: false,
                error: None,
                added_at: util::now_unix(),
                finished_at: None,
                options: new.options,
                checksum_verified: false,
                awaiting_link: false,
                relaxed_resume: false,
                active_secs: 0.0,
            }));
            st.dirty = true;
            id
        };
        self.inner.emit(Event::Added(id));
        self.inner.schedule();
        Ok(id)
    }

    pub fn pause(&self, id: DownloadId) -> Result<(), Error> {
        self.inner.with_item(id, |item, inner| {
            if !item.rec.status.can_pause() {
                return;
            }
            if let Some(run) = &mut item.run {
                run.on_stop = AfterStop::Pause;
                item.frozen = Some(run.shared.table.lock().unwrap().received());
                run.cancel.cancel();
            }
            item.retry_at = None;
            inner.set_status(item, Status::Paused);
        })
    }

    pub fn resume(&self, id: DownloadId) -> Result<(), Error> {
        self.inner.with_item(id, |item, inner| {
            if !item.rec.status.can_resume() {
                return;
            }
            if let Some(run) = &mut item.run {
                run.on_stop = AfterStop::Requeue;
            }
            item.rec.error = None;
            item.auto_retries = 0;
            item.retry_at = None;
            inner.set_status(item, Status::Queued);
        })?;
        self.inner.schedule();
        Ok(())
    }

    /// Discards partial data and downloads from scratch.
    pub fn restart(&self, id: DownloadId) -> Result<(), Error> {
        self.inner.with_item(id, |item, inner| match &mut item.run {
            Some(run) => {
                run.on_stop = AfterStop::Restart;
                run.cancel.cancel();
            }
            None => {
                reset_for_restart(item);
                inner.set_status(item, Status::Queued);
            }
        })?;
        self.inner.schedule();
        Ok(())
    }

    /// Removes a download from the list. With `delete_file` the completed
    /// file or the partial data is deleted from disk; otherwise it is kept
    /// (adding the same link again later resumes the partial data).
    pub fn remove(&self, id: DownloadId, delete_file: bool) -> Result<(), Error> {
        let removed = {
            let mut st = self.inner.lock();
            let pos = st.items.iter().position(|i| i.rec.id == id).ok_or(Error::NotFound)?;
            if let Some(run) = &mut st.items[pos].run {
                run.on_stop = AfterStop::Remove { delete_file };
                run.cancel.cancel();
                None
            } else {
                st.dirty = true;
                Some(st.items.remove(pos).rec)
            }
        };
        if let Some(rec) = removed {
            cleanup_files(&rec, delete_file);
            self.inner.emit(Event::Removed(id));
        }
        Ok(())
    }

    pub fn pause_all(&self) {
        let ids: Vec<_> =
            self.inner.lock().items.iter().filter(|i| i.rec.status.can_pause()).map(|i| i.rec.id).collect();
        for id in ids {
            let _ = self.pause(id);
        }
    }

    pub fn resume_all(&self) {
        let ids: Vec<_> =
            self.inner.lock().items.iter().filter(|i| i.rec.status.can_resume()).map(|i| i.rec.id).collect();
        for id in ids {
            let _ = self.resume(id);
        }
    }

    /// Forgets completed downloads (files are kept).
    pub fn clear_completed(&self) {
        let removed: Vec<_> = {
            let mut st = self.inner.lock();
            let (done, keep) =
                std::mem::take(&mut st.items).into_iter().partition(|i| i.rec.status == Status::Completed);
            st.items = keep;
            st.dirty = true;
            done.into_iter().map(|i: Item| i.rec.id).collect::<Vec<_>>()
        };
        for id in removed {
            self.inner.emit(Event::Removed(id));
        }
    }

    /// Moves a download up (`delta < 0`) or down its queue, past `delta`
    /// downloads of the same queue.
    pub fn reorder(&self, id: DownloadId, delta: isize) -> Result<(), Error> {
        let mut st = self.inner.lock();
        let pos = st.items.iter().position(|i| i.rec.id == id).ok_or(Error::NotFound)?;
        let queue = st.items[pos].rec.queue;
        // Positions of this queue's downloads, in order; move within those.
        let same: Vec<usize> = (0..st.items.len()).filter(|&i| st.items[i].rec.queue == queue).collect();
        let at = same.iter().position(|&i| i == pos).unwrap_or(0) as isize;
        let target = same[(at + delta).clamp(0, same.len() as isize - 1) as usize];
        let item = st.items.remove(pos);
        st.items.insert(target, item);
        st.dirty = true;
        Ok(())
    }

    /// Moves a download to the start (`top`) or the end of its queue.
    pub fn move_to_end(&self, id: DownloadId, top: bool) -> Result<(), Error> {
        self.reorder(id, if top { isize::MIN / 2 } else { isize::MAX / 2 })
    }

    /// Moves downloads to another queue (the main queue if it doesn't exist),
    /// at its end. Running downloads keep running.
    pub fn set_queue(&self, ids: &[DownloadId], queue: QueueId) -> Result<(), Error> {
        let queue = valid_queue(&self.config(), queue);
        {
            let mut st = self.inner.lock();
            for &id in ids {
                let pos = st.items.iter().position(|i| i.rec.id == id).ok_or(Error::NotFound)?;
                if st.items[pos].rec.queue == queue {
                    continue;
                }
                let mut item = st.items.remove(pos);
                item.rec.queue = queue;
                st.items.push(item);
            }
            st.dirty = true;
        }
        self.inner.schedule();
        Ok(())
    }

    /// Resumes every paused or failed download of a queue.
    pub fn start_queue(&self, queue: QueueId) {
        for id in self.ids_where(|i| i.rec.queue == queue && i.rec.status.can_resume() && !i.rec.awaiting_link) {
            let _ = self.resume(id);
        }
    }

    /// Pauses every download of a queue.
    pub fn stop_queue(&self, queue: QueueId) {
        for id in self.ids_where(|i| i.rec.queue == queue && i.rec.status.can_pause()) {
            let _ = self.pause(id);
        }
    }

    fn ids_where(&self, pred: impl Fn(&Item) -> bool) -> Vec<DownloadId> {
        self.inner.lock().items.iter().filter(|i| pred(i)).map(|i| i.rec.id).collect()
    }

    /// The folder a new download with this file name goes to, following the
    /// category folders and "sort into category folders".
    pub fn folder_for(&self, file_name: &str) -> PathBuf {
        let config = self.config();
        target_dir(&config, &DownloadOptions::default(), config.categorize(file_name))
    }

    /// Global speed cap, bytes/second (`0` = unlimited). Applies instantly.
    pub fn set_speed_limit(&self, bytes_per_sec: u64) {
        let _one_at_a_time = self.inner.config_lock.lock().unwrap();
        let mut config = self.config();
        config.speed_limit = bytes_per_sec;
        let _ = self.apply_config(config);
    }

    /// Per-download speed cap, bytes/second (`0` = unlimited). Applies instantly.
    pub fn set_download_speed_limit(&self, id: DownloadId, bytes_per_sec: u64) -> Result<(), Error> {
        self.inner.with_item(id, |item, _| {
            item.rec.options.speed_limit = bytes_per_sec;
            if let Some(run) = &item.run {
                run.shared.limiter.set_rate(bytes_per_sec);
            }
        })
    }

    pub fn config(&self) -> Config {
        self.inner.config.read().unwrap().clone()
    }

    /// Applies and persists new settings. Network settings affect downloads
    /// started afterwards; limits and concurrency apply immediately.
    pub fn update_config(&self, config: Config) -> Result<(), Error> {
        let _one_at_a_time = self.inner.config_lock.lock().unwrap();
        self.apply_config(config)
    }

    fn apply_config(&self, config: Config) -> Result<(), Error> {
        let config = config.sanitized();
        // Rebuilding drops the connection pool: only do it when the network settings changed.
        let rebuild = {
            let old = self.inner.config.read().unwrap();
            old.user_agent != config.user_agent
                || old.connect_timeout_secs != config.connect_timeout_secs
                || old.http1_only != config.http1_only
                || old.insecure_tls != config.insecure_tls
                || old.proxy != config.proxy
        };
        let relink = {
            let old = self.inner.config.read().unwrap();
            rebuild
                || old.multi_link != config.multi_link
                || old
                    .interfaces
                    .iter()
                    .map(|l| (&l.address, l.enabled))
                    .ne(config.interfaces.iter().map(|l| (&l.address, l.enabled)))
        };
        let client = if rebuild { Some(build_client(&config)?) } else { None };
        let pool = if relink {
            let base = client.clone().unwrap_or_else(|| self.inner.client.read().unwrap().clone());
            Some(build_pool(&config, &base))
        } else {
            None
        };
        self.inner.store.save_config(&config)?;
        if let Some(client) = client {
            *self.inner.client.write().unwrap() = client;
        }
        match pool {
            Some(pool) => {
                *self.inner.links.write().unwrap() = Arc::new(pool);
                self.inner.link_stats.lock().unwrap().clear();
            }
            None => {
                // Only the caps changed: apply them without dropping connections.
                for link in &self.inner.links.read().unwrap().links {
                    if let Some(l) = config
                        .interfaces
                        .iter()
                        .find(|l| Some(l.address.as_str()) == link.address.map(|a| a.to_string()).as_deref())
                    {
                        link.limiter.set_rate(l.limit);
                    }
                }
            }
        }
        {
            // Downloads of a deleted queue join the main queue, and every
            // download follows the current category rules.
            let mut st = self.inner.lock();
            let mut changed = false;
            for item in &mut st.items {
                let queue = valid_queue(&config, item.rec.queue);
                let category = config.categorize(&item.rec.file_name);
                changed |= queue != item.rec.queue || category != item.rec.category;
                item.rec.queue = queue;
                item.rec.category = category;
            }
            st.dirty |= changed;
        }
        *self.inner.config.write().unwrap() = config;
        self.inner.apply_speed_limits();
        if self.inner.store.dir().is_some() {
            self.inner.apply_api_config();
        }
        self.inner.schedule();
        Ok(())
    }

    /// The network links in use with their live speed. One entry (the default
    /// connection) unless several adapters are bonded.
    pub fn links(&self) -> Vec<LinkView> {
        let pool = Arc::clone(&self.inner.links.read().unwrap());
        let stats = self.inner.link_stats.lock().unwrap();
        pool.links
            .iter()
            .enumerate()
            .map(|(i, link)| LinkView {
                label: link.label.clone(),
                address: link.address.map(|a| a.to_string()),
                speed: stats.get(i).map_or(0.0, |s| s.speed),
                bytes: link.bytes.load(Ordering::Relaxed),
                limit: link.limiter.rate(),
                history: stats.get(i).map(|s| s.history.iter().copied().collect()).unwrap_or_default(),
            })
            .collect()
    }

    /// Data downloaded this month, the cap (`0` = none) and whether it stopped the downloads.
    pub fn month_usage(&self) -> (u64, u64, bool) {
        let used = self.inner.lock().stats.month_total(util::now_unix());
        (used, self.config().monthly_cap, self.inner.cap_hit.load(Ordering::Relaxed))
    }

    /// Partial-download files that no download in the list owns, in the
    /// download folders (they pile up after removing unfinished downloads).
    pub fn leftovers(&self) -> Vec<crate::Leftover> {
        let config = self.config();
        let st = self.inner.lock();
        let mut dirs: Vec<PathBuf> = vec![config.download_dir.clone()];
        dirs.extend(config.category_dirs.values().cloned());
        let mut owned = HashSet::new();
        for item in &st.items {
            if let Some(dir) = item.rec.path.parent() {
                dirs.push(dir.to_path_buf());
            }
            if item.rec.status != Status::Completed {
                owned.insert(part_path(&item.rec.path));
                owned.insert(meta_path(&item.rec.path));
            }
        }
        drop(st);
        dirs.sort();
        dirs.dedup();
        // A folder inside another is read with it.
        let roots: Vec<PathBuf> =
            dirs.iter().filter(|d| !dirs.iter().any(|o| o != *d && d.starts_with(o))).cloned().collect();
        crate::tools::find_leftovers(&roots, &owned)
    }

    /// Folders worth scanning for duplicates.
    pub fn download_folders(&self) -> Vec<PathBuf> {
        let config = self.config();
        let mut dirs = vec![config.download_dir.clone()];
        dirs.extend(config.category_dirs.values().cloned());
        dirs.sort();
        dirs.dedup();
        dirs.iter().filter(|d| !dirs.iter().any(|o| o != *d && d.starts_with(o))).cloned().collect()
    }

    /// Writes a backup of settings (without access tokens), history, statistics and the list.
    pub fn export_backup(&self, dest: &Path) -> Result<(), Error> {
        let dir = self.data_dir().ok_or_else(|| Error::Unsupported("this engine keeps nothing on disk".into()))?;
        // Make sure the files are current.
        self.inner.persist_parts(true, true);
        crate::tools::export_backup(&dir, &self.config(), dest)
    }

    /// Applies the settings of a backup; this computer's access tokens and
    /// the folders' existence are left as they are.
    pub fn import_settings(&self, src: &Path) -> Result<(), Error> {
        let mut config = crate::tools::read_backup_settings(src)?;
        let mine = self.config();
        config.api_token = mine.api_token;
        config.mcp_token = mine.mcp_token;
        config.control_token = mine.control_token;
        config.remote_notify.keep_secrets_of(&mine.remote_notify);
        self.update_config(config)
    }

    /// A report for bug reports: versions, settings that matter, adapters,
    /// tools and free space. No links, names or tokens.
    pub fn diagnostics(&self) -> String {
        let c = self.config();
        let (ytdlp, ffmpeg) = self.media_tools();
        let tool = |name: &str, p: Option<PathBuf>| match p {
            Some(p) => format!("{name}: {} ({})", p.display(), crate::tool_version(&p, name).unwrap_or_else(|e| e)),
            None => format!("{name}: not found"),
        };
        let adapters =
            crate::adapters().iter().map(|a| format!("{} {}", a.name, a.address)).collect::<Vec<_>>().join(", ");
        let (used, cap, hit) = self.month_usage();
        let snap = self.snapshot();
        let free = util::free_space(&c.download_dir).map_or("unknown".into(), util::format_bytes);
        [
            format!("StewardGet {} on {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH),
            format!("Downloads: {} in the list, {} active", snap.downloads.len(), snap.count(Status::is_active)),
            format!("Free space in the download folder: {free}"),
            format!(
                "Connections {} (auto-tune {}), simultaneous {}, retries {}, HTTP/1.1 only {}",
                c.connections, c.auto_tune, c.max_concurrent, c.max_retries, c.http1_only
            ),
            format!("Proxy: {}", if c.proxy.is_some() { "set" } else { "none" }),
            format!(
                "Several networks: {} ({} chosen)",
                c.multi_link,
                c.interfaces.iter().filter(|l| l.enabled).count()
            ),
            format!("Adapters: {}", if adapters.is_empty() { "none found".into() } else { adapters }),
            format!("Links in use: {}", self.links().iter().map(|l| l.label.clone()).collect::<Vec<_>>().join(", ")),
            tool("yt-dlp", ytdlp),
            tool("ffmpeg", ffmpeg),
            format!("Steps after download: {}", c.post_actions.iter().filter(|r| r.enabled).count()),
            format!(
                "Monthly cap: {} used of {}{}",
                util::format_bytes(used),
                if cap == 0 { "no cap".into() } else { util::format_bytes(cap) },
                if hit { " (reached)" } else { "" }
            ),
            format!("Browser API: {} on port {}", if c.api_enabled { "on" } else { "off" }, c.api_port),
        ]
        .join("\n")
    }

    /// Asks yt-dlp (or reads a DASH manifest) for what a page offers.
    pub async fn inspect_media(&self, url: &str) -> Result<crate::MediaInfo, Error> {
        let config = self.config();
        let client = self.inner.client.read().unwrap().clone();
        let ytdlp = crate::find_tool(config.ytdlp_path.as_deref(), "yt-dlp");
        crate::media::inspect(&client, ytdlp.as_deref(), url.trim()).await
    }

    /// Where yt-dlp and ffmpeg were found, if anywhere.
    pub fn media_tools(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        let config = self.config();
        (
            crate::find_tool(config.ytdlp_path.as_deref(), "yt-dlp"),
            crate::find_tool(config.ffmpeg_path.as_deref(), "ffmpeg"),
        )
    }

    /// Downloads one of the choices from [`Engine::inspect_media`]. A choice
    /// with separate picture and sound becomes two downloads that are joined
    /// when both are done.
    pub fn add_media(
        &self,
        info: &crate::MediaInfo,
        index: usize,
        directory: Option<PathBuf>,
        queue: Option<QueueId>,
    ) -> Result<Vec<DownloadId>, Error> {
        let option = info.options.get(index).ok_or(Error::NotFound)?;
        if option.needs_merge() && self.media_tools().1.is_none() {
            return Err(Error::Unsupported(
                "joining video and audio needs ffmpeg, which isn't installed (for example `winget install ffmpeg`). Pick a choice with picture and sound in one file, or audio only".into(),
            ));
        }
        let title = filename::sanitize(&info.title);
        let group = util::random_token();
        let out_ext = match option.parts.as_slice() {
            [v, a] if v.ext == "mp4" && matches!(a.ext.as_str(), "m4a" | "mp4") => "mp4",
            _ => "mkv",
        };
        let mut ids = Vec::new();
        for part in &option.parts {
            let (file_name, mux) = match part.stream {
                Some(stream) => {
                    let tag = if stream == crate::MuxStream::Video { "video" } else { "audio" };
                    (
                        format!("{title}.{tag}.{}", part.ext),
                        Some(crate::Mux { group: group.clone(), stream, output: format!("{title}.{out_ext}") }),
                    )
                }
                None => (format!("{title}.{}", part.ext), None),
            };
            let headers = part
                .headers
                .iter()
                .filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "accept-encoding" | "range" | "host"))
                .cloned()
                .collect();
            let mut new = NewDownload::new(part.url.clone());
            new.queue = queue;
            new.options.file_name = Some(file_name);
            new.options.directory = directory.clone();
            new.options.headers = headers;
            new.options.mux = mux;
            ids.push(self.add(new)?);
        }
        Ok(ids)
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let config = self.config();
        let st = self.inner.lock();
        EngineSnapshot {
            downloads: st.items.iter().map(|i| i.view_with(false)).collect(),
            speed: st.speed,
            history: st.history.iter().copied().collect(),
            speed_limit: config.speed_limit,
            scheduled_limit: self.inner.scheduled_limit.load(Ordering::Relaxed).then_some(config.speed_schedule.limit),
            max_concurrent: config.max_concurrent,
            stats: st.stats.clone(),
            api: self.inner.api.lock().unwrap().state.clone(),
            extension: self.inner.extension.lock().unwrap().as_ref().map(|(at, browser, version)| ExtensionInfo {
                browser: browser.clone(),
                version: version.clone(),
                seen_secs: at.elapsed().as_secs(),
            }),
        }
    }

    /// The permanent log of finished downloads, oldest first.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.inner.lock().log.entries.clone()
    }

    /// Changes whenever [`Engine::history`] would return something different.
    pub fn history_generation(&self) -> u64 {
        self.inner.lock().log_gen
    }

    /// Forgets history entries (files stay). Returns how many were removed.
    pub fn remove_history(&self, ids: &[DownloadId]) -> usize {
        let mut st = self.inner.lock();
        let n = st.log.remove(ids);
        if n > 0 {
            st.log_dirty = true;
            st.log_gen += 1;
        }
        n
    }

    pub fn clear_history(&self) {
        let mut st = self.inner.lock();
        st.log.clear();
        st.log_dirty = true;
        st.log_gen += 1;
    }

    /// Replaces the tags of a history entry.
    pub fn tag_history(&self, id: DownloadId, tags: Vec<String>) -> bool {
        let mut st = self.inner.lock();
        let ok = st.log.set_tags(id, tags);
        if ok {
            st.log_dirty = true;
            st.log_gen += 1;
        }
        ok
    }

    /// Reads the history without starting an engine or taking the lock.
    pub fn peek_history(dir: &Path) -> Vec<HistoryEntry> {
        store::peek_history(dir).map(|h| h.entries).unwrap_or_default()
    }

    /// True when StewardGet runs for the first time with this data directory (no
    /// settings were saved yet).
    pub fn first_run(&self) -> bool {
        self.inner.first_run
    }

    /// Called when another launch of StewardGet asks this one to show its window
    /// (`POST /api/v1/show`). The handler runs on an engine thread.
    pub fn set_show_handler(&self, handler: impl Fn() + Send + Sync + 'static) {
        *self.inner.show_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    /// Lets the desktop app ask the user about downloads from the browser
    /// before they are added, while [`Config::capture_dialog`] is on. The
    /// handler gets them ready to add and runs on an engine thread.
    pub fn set_capture_handler(&self, handler: impl Fn(Vec<NewDownload>) + Send + Sync + 'static) {
        *self.inner.capture_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    /// Called when another program asks StewardGet to quit (`POST /api/v1/quit`),
    /// e.g. the installer before it replaces the files.
    pub fn set_quit_handler(&self, handler: impl Fn() + Send + Sync + 'static) {
        *self.inner.quit_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    /// Runs the quit handler. `false` when no front-end registered one.
    pub(crate) fn request_quit(&self) -> bool {
        let handler = self.inner.quit_handler.lock().unwrap().clone();
        handler.map(|h| h()).is_some()
    }

    /// Runs the show handler. `false` when no front-end with a window registered one.
    pub(crate) fn request_show(&self) -> bool {
        let handler = self.inner.show_handler.lock().unwrap().clone();
        handler.map(|h| h()).is_some()
    }

    /// Notes a request from the browser extension.
    pub(crate) fn extension_seen(&self, browser: &str, version: &str) {
        *self.inner.extension.lock().unwrap() = Some((Instant::now(), browser.to_owned(), version.to_owned()));
    }

    pub fn download(&self, id: DownloadId) -> Option<DownloadView> {
        self.inner.lock().items.iter().find(|i| i.rec.id == id).map(Item::view)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// The data directory, if persistent.
    pub fn data_dir(&self) -> Option<PathBuf> {
        self.inner.store.dir().map(Path::to_path_buf)
    }

    /// Stops all transfers (they resume on next start), checkpoints and saves.
    pub async fn shutdown(&self) {
        if let Some(stop) = self.inner.api.lock().unwrap().stop.take() {
            stop.cancel();
        }
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        self.inner.shutdown.cancel();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && self.inner.lock().items.iter().any(|i| i.run.is_some()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.inner.persist(true);
    }
}

impl Inner {
    /// Starts, restarts or stops the browser API to match the configuration.
    fn apply_api_config(self: &Arc<Self>) {
        let config = self.config();
        // One listener serves both the browser API and MCP.
        let wanted = (config.api_enabled || config.mcp_enabled, config.api_port, None);
        let mut api = self.api.lock().unwrap();
        if api.applied.as_ref() == Some(&wanted) {
            return;
        }
        if let Some(stop) = api.stop.take() {
            stop.cancel();
        }
        api.applied = Some(wanted);
        if !(config.api_enabled || config.mcp_enabled) {
            api.state = ApiState::Disabled;
            return;
        }
        let port = config.api_port;
        // Bind right here so the UI immediately knows if the port is taken.
        let bound = std::net::TcpListener::bind(("127.0.0.1", port)).and_then(|l| {
            l.set_nonblocking(true)?;
            let _guard = self.runtime.enter();
            tokio::net::TcpListener::from_std(l)
        });
        match bound {
            Ok(listener) => {
                let stop = self.shutdown.child_token();
                api.state = ApiState::Listening(port);
                api.stop = Some(stop.clone());
                self.runtime.spawn(crate::api::serve(listener, Arc::downgrade(self), stop));
            }
            Err(e) => api.state = ApiState::Failed(format!("port {port}: {e}")),
        }
    }

    /// Sets the global limit (the scheduled one during its hours) and each
    /// queue's limit. Rates are only touched when they change, since setting
    /// one empties its bucket.
    fn apply_speed_limits(&self) {
        use chrono::{Datelike, Timelike};
        let config = self.config();
        let now = chrono::Local::now();
        let weekday = now.weekday().num_days_from_monday() as usize;
        let minute = (now.hour() * 60 + now.minute()) as u16;
        self.scheduled_limit.store(config.speed_schedule.is_active(weekday, minute), Ordering::Relaxed);
        let global = config.effective_speed_limit(weekday, minute);
        if self.global_limiter.rate() != global {
            self.global_limiter.set_rate(global);
        }
        let mut limiters = self.queue_limiters.lock().unwrap();
        limiters.retain(|id, _| config.queues.iter().any(|q| q.id == *id));
        for q in &config.queues {
            let limiter = limiters.entry(q.id).or_insert_with(|| Arc::new(RateLimiter::new(q.speed_limit)));
            if limiter.rate() != q.speed_limit {
                limiter.set_rate(q.speed_limit);
            }
        }
    }

    fn queue_limiter(&self, queue: QueueId) -> Arc<RateLimiter> {
        let mut limiters = self.queue_limiters.lock().unwrap();
        Arc::clone(limiters.entry(queue).or_insert_with(|| Arc::new(RateLimiter::new(0))))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    fn with_item(&self, id: DownloadId, f: impl FnOnce(&mut Item, &Inner)) -> Result<(), Error> {
        let mut st = self.lock();
        let item = st.items.iter_mut().find(|i| i.rec.id == id).ok_or(Error::NotFound)?;
        f(item, self);
        st.dirty = true;
        Ok(())
    }

    fn set_status(&self, item: &mut Item, status: Status) {
        if item.rec.status != status {
            item.rec.status = status;
            self.emit(Event::StatusChanged(item.rec.id, status));
        }
    }

    /// Starts queued downloads while slots are free.
    fn schedule(self: &Arc<Self>) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        if self.cap_hit.load(Ordering::Relaxed) {
            return; // the monthly data cap is used up
        }
        let (max, caps): (usize, HashMap<QueueId, usize>) = {
            let config = self.config.read().unwrap();
            (config.max_concurrent, config.queues.iter().map(|q| (q.id, q.max_concurrent)).collect())
        };
        let mut st = self.lock();
        let mut running = 0;
        let mut per_queue: HashMap<QueueId, usize> = HashMap::new();
        for item in st.items.iter().filter(|i| i.run.is_some()) {
            running += 1;
            *per_queue.entry(item.rec.queue).or_default() += 1;
        }
        let now = Instant::now();
        for i in 0..st.items.len() {
            if running >= max {
                break;
            }
            let item = &st.items[i];
            let queue = item.rec.queue;
            let cap = caps.get(&queue).copied().unwrap_or(0);
            if cap > 0 && per_queue.get(&queue).copied().unwrap_or(0) >= cap {
                continue;
            }
            if item.rec.status == Status::Queued && item.run.is_none() && !item.waiting_for_retry(now) {
                self.start(&mut st.items[i]);
                st.dirty = true;
                running += 1;
                *per_queue.entry(queue).or_default() += 1;
            }
        }
    }

    fn start(self: &Arc<Self>, item: &mut Item) {
        let url = match Url::parse(&item.rec.url) {
            Ok(u) => u,
            Err(e) => {
                item.rec.error = Some(Error::InvalidUrl(e.to_string()).to_string());
                self.set_status(item, Status::Failed);
                return;
            }
        };
        let id = item.rec.id;
        let shared = Arc::new(Shared::new(item.rec.options.speed_limit));
        let cancel = self.shutdown.child_token();
        let job = Job {
            url,
            options: item.rec.options.clone(),
            config: self.config(),
            client: self.client.read().unwrap().clone(),
            links: Arc::clone(&self.links.read().unwrap()),
            global_limiter: Arc::clone(&self.global_limiter),
            queue_limiter: self.queue_limiter(item.rec.queue),
            shared: Arc::clone(&shared),
            cancel: cancel.clone(),
            hooks: Arc::new(EngineHooks { inner: Arc::clone(self), id }),
            relaxed_resume: item.rec.relaxed_resume,
        };
        item.run = Some(Running { cancel, shared, on_stop: AfterStop::Pause });
        item.retry_at = None;
        item.last_transferred = 0;
        item.rec.error = None;
        self.set_status(item, Status::Connecting);

        let inner = Arc::clone(self);
        self.runtime.spawn(async move {
            let outcome = task::run(job).await;
            inner.finish(id, outcome);
        });
    }

    fn finish(self: &Arc<Self>, id: DownloadId, outcome: Outcome) {
        let config = self.config();
        let mut mux_group = None;
        {
            let mut st = self.lock();
            let Some(pos) = st.items.iter().position(|i| i.rec.id == id) else { return };
            let now = util::now_unix();
            let shutting_down = self.shutting_down.load(Ordering::SeqCst);
            let item = &mut st.items[pos];
            let Some(run) = item.run.take() else { return };

            // Final accounting for bytes that arrived since the last sample.
            let late = run.shared.transferred.load(Ordering::Relaxed).saturating_sub(item.last_transferred);
            {
                let t = run.shared.table.lock().unwrap();
                if !t.segments.is_empty() {
                    item.rec.downloaded = t.written();
                    item.segments = t.views();
                }
            }
            item.rec.total = run.shared.total().or(item.rec.total);
            item.speed = 0.0;
            item.connections = 0;
            item.reconnecting = 0;

            let mut completed = false;
            let mut failed = false;
            let mut remove = None;
            let mut after_command = None;
            match (outcome, run.on_stop) {
                (_, AfterStop::Remove { delete_file }) => remove = Some(delete_file),
                (Outcome::Completed { path, total, verified }, _) => {
                    item.rec.file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    item.rec.path = path;
                    item.rec.total = Some(total);
                    item.rec.downloaded = total;
                    item.rec.finished_at = Some(now);
                    item.rec.checksum_verified = verified;
                    item.rec.error = None;
                    item.rec.awaiting_link = false;
                    item.rec.relaxed_resume = false;
                    // Browser session cookies aren't kept once they've done their job.
                    item.rec.options.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("cookie"));
                    item.segments.clear();
                    self.set_status(item, Status::Completed);
                    completed = true;
                    mux_group = item.rec.options.mux.as_ref().map(|m| m.group.clone());
                    if config.on_complete_command.is_some() || config.post_actions.iter().any(|r| r.enabled) {
                        after_command = Some((
                            config.on_complete_command.clone(),
                            config.post_actions.clone(),
                            item.rec.path.clone(),
                            item.rec.url.clone(),
                            item.rec.category,
                            total,
                        ));
                    }
                }
                // Interrupted by quitting: picked up again at the next start, unless
                // the user had paused it just before.
                (Outcome::Cancelled, _) if shutting_down && item.rec.status != Status::Paused => {
                    item.rec.status = Status::Queued;
                }
                (Outcome::Cancelled, AfterStop::Restart) => {
                    reset_for_restart(item);
                    self.set_status(item, Status::Queued);
                }
                (Outcome::Cancelled, AfterStop::Requeue) => self.set_status(item, Status::Queued),
                (Outcome::Cancelled, AfterStop::Pause) => self.set_status(item, Status::Paused),
                // The task failed just as the user (or a fresh link) stopped it:
                // what they asked for wins over an automatic retry or "Failed".
                (Outcome::Failed(_), _) if shutting_down && item.rec.status != Status::Paused => {
                    item.rec.status = Status::Queued;
                }
                (Outcome::Failed(_), AfterStop::Restart) => {
                    reset_for_restart(item);
                    self.set_status(item, Status::Queued);
                }
                (Outcome::Failed(_), AfterStop::Requeue) => self.set_status(item, Status::Queued),
                (Outcome::Failed(_), AfterStop::Pause) if item.rec.status == Status::Paused => {}
                (Outcome::Failed(e), _)
                    if e.is_retryable() && item.auto_retries < config.auto_retry && !item.rec.awaiting_link =>
                {
                    // Network trouble: try again later, with growing pauses.
                    let delay = config.auto_retry_delay_secs.saturating_mul(1 << item.auto_retries.min(6));
                    item.auto_retries += 1;
                    item.retry_at = Some(Instant::now() + Duration::from_secs(delay));
                    item.rec.error = Some(e.to_string());
                    self.set_status(item, Status::Queued);
                }
                (Outcome::Failed(e), _) => {
                    item.rec.error = Some(e.to_string());
                    self.set_status(item, Status::Failed);
                    failed = true;
                }
            }

            if (completed || failed) && !config.remote_notify.targets.is_empty() {
                let events = notify_events(&st.items, pos, failed, &config);
                let plan = config.remote_notify.plan(&events);
                if !plan.is_empty() {
                    // Best effort, in the background: a slow mail server mustn't hold anything up.
                    self.runtime.spawn(async move {
                        let _ = crate::notifiers::send_planned(plan).await;
                    });
                }
            }
            if completed || failed {
                let entry = history_entry(&st.items[pos].rec, config.connections);
                st.log.record(entry);
                st.log_dirty = true;
                st.log_gen += 1;
            }
            st.stats.record_bytes(late, now);
            st.stats.completed += u64::from(completed);
            st.stats.failed += u64::from(failed);
            st.dirty = true;
            st.stats_dirty = true;
            if let Some(delete_file) = remove {
                let rec = st.items.remove(pos).rec;
                drop(st);
                cleanup_files(&rec, delete_file);
                self.emit(Event::Removed(id));
            }
            if let Some((command, rules, path, url, category, size)) = after_command {
                // Best effort, in the background: a slow scanner mustn't hold up the queue.
                let inner = Arc::clone(self);
                self.runtime.spawn_blocking(move || {
                    if let Some(command) = command {
                        let _ = util::run_after_download(&command, &path, &url);
                    }
                    let report = crate::run_rules(&rules, &path, &url, category, size);
                    if !report.outcomes.is_empty() || !report.tags.is_empty() {
                        inner.apply_report(id, report);
                    }
                });
            }
        }
        self.schedule();
        self.persist(false);
        // After the state lock is released: joining looks at every download.
        if let Some(group) = mux_group {
            self.try_mux(group);
        }
    }

    /// Once both halves of a video have arrived, joins them with ffmpeg.
    fn try_mux(self: &Arc<Self>, group: String) {
        let (video, audio, output) = {
            let st = self.lock();
            let parts: Vec<&Item> =
                st.items.iter().filter(|i| i.rec.options.mux.as_ref().is_some_and(|m| m.group == group)).collect();
            if parts.len() != 2 || parts.iter().any(|i| i.rec.status != Status::Completed) {
                return;
            }
            let pick =
                |s: crate::MuxStream| parts.iter().find(|i| i.rec.options.mux.as_ref().is_some_and(|m| m.stream == s));
            let (Some(v), Some(a)) = (pick(crate::MuxStream::Video), pick(crate::MuxStream::Audio)) else { return };
            let output = v.rec.options.mux.as_ref().map(|m| m.output.clone()).unwrap_or_default();
            ((v.rec.id, v.rec.path.clone()), (a.rec.id, a.rec.path.clone()), output)
        };
        if !self.muxing.lock().unwrap().insert(group.clone()) {
            return;
        }
        let ffmpeg = crate::find_tool(self.config().ffmpeg_path.as_deref(), "ffmpeg");
        let inner = Arc::clone(self);
        self.runtime.spawn_blocking(move || {
            let dir = video.1.parent().map(Path::to_path_buf).unwrap_or_default();
            let target = filename::unique_path(&dir, &filename::sanitize(&output), |p| p.exists());
            let result = match &ffmpeg {
                Some(ff) => crate::merge_media(ff, &video.1, &audio.1, &target).map_err(|e| e.to_string()),
                None => Err("ffmpeg was not found; install it to join video and audio".to_owned()),
            };
            inner.muxing.lock().unwrap().remove(&group);
            inner.finish_mux(video.0, audio.0, &video.1, &audio.1, &target, result);
        });
    }

    fn finish_mux(
        self: &Arc<Self>,
        video_id: DownloadId,
        audio_id: DownloadId,
        video: &Path,
        audio: &Path,
        target: &Path,
        result: Result<(), String>,
    ) {
        {
            let mut st = self.lock();
            match &result {
                Ok(()) => {
                    let _ = std::fs::remove_file(video);
                    let _ = std::fs::remove_file(audio);
                    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let category = self.config().categorize(&name);
                    if let Some(item) = st.items.iter_mut().find(|i| i.rec.id == video_id) {
                        item.rec.path = target.to_path_buf();
                        item.rec.file_name = name.clone();
                        item.rec.category = category;
                        item.rec.options.mux = None;
                        item.rec.total = std::fs::metadata(target).ok().map(|m| m.len());
                    }
                    st.items.retain(|i| i.rec.id != audio_id);
                    st.log.entries.retain(|e| e.id != audio_id);
                    if let Some(e) = st.log.entries.iter_mut().find(|e| e.id == video_id) {
                        e.path = target.to_path_buf();
                        e.file_name = name;
                        e.category = category;
                        e.size = std::fs::metadata(target).ok().map(|m| m.len());
                        e.notes = vec!["✓ Joined video and audio with ffmpeg".into()];
                    }
                }
                Err(why) => {
                    for id in [video_id, audio_id] {
                        if let Some(e) = st.log.entries.iter_mut().find(|e| e.id == id) {
                            e.notes =
                                vec![format!("✗ Could not join video and audio: {why}. The two files were kept.")];
                        }
                    }
                }
            }
            st.dirty = true;
            st.log_dirty = true;
            st.log_gen += 1;
        }
        if result.is_ok() {
            self.emit(Event::Removed(audio_id));
        }
        self.persist(false);
    }

    /// Records what the post-download rules did: the new place of a moved file,
    /// tags and a line per step in the history.
    fn apply_report(self: &Arc<Self>, id: DownloadId, report: crate::ActionReport) {
        {
            let mut st = self.lock();
            let moved = report.new_path.clone();
            if let (Some(path), Some(item)) = (&moved, st.items.iter_mut().find(|i| i.rec.id == id)) {
                item.rec.path = path.clone();
                item.rec.file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                st.dirty = true;
            }
            if let Some(entry) = st.log.entries.iter_mut().find(|e| e.id == id) {
                if let Some(path) = &moved {
                    entry.path = path.clone();
                    entry.file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                }
                for tag in report.tags {
                    if !entry.tags.contains(&tag) {
                        entry.tags.push(tag);
                    }
                }
                entry.notes = report
                    .outcomes
                    .iter()
                    .map(|o| format!("{} {}: {}", if o.ok { "✓" } else { "✗" }, o.rule, o.message))
                    .collect();
                st.log_dirty = true;
                st.log_gen += 1;
            }
        }
        self.persist(false);
    }

    /// Pauses everything once the month's data cap is used up, and lets
    /// downloads start again when the cap is raised or a new month begins.
    fn enforce_cap(self: &Arc<Self>) {
        let cap = self.config.read().unwrap().monthly_cap;
        let used = self.lock().stats.month_total(util::now_unix());
        if cap > 0 && used >= cap {
            if !self.cap_hit.swap(true, Ordering::Relaxed) {
                Engine::from_inner(Arc::clone(self)).pause_all();
            }
        } else if self.cap_hit.swap(false, Ordering::Relaxed) {
            self.schedule();
        }
    }

    /// Updates speeds, histories and statistics. Called every sample interval.
    fn sample(&self, dt: f64) {
        let now = util::now_unix();
        let mut st = self.lock();
        let mut delta_total = 0;
        let mut speed_total = 0.0;
        let mut transferring = false;
        for item in &mut st.items {
            let Some(run) = &item.run else { continue };
            let transferred = run.shared.transferred.load(Ordering::Relaxed);
            let delta = transferred.saturating_sub(item.last_transferred);
            item.last_transferred = transferred;
            let instant = delta as f64 / dt;
            item.speed += SPEED_ALPHA * (instant - item.speed);
            if item.speed < 1.0 {
                item.speed = 0.0;
            }
            push_bounded(&mut item.history, item.speed as f32);
            {
                let t = run.shared.table.lock().unwrap();
                item.connections = t.active();
                item.reconnecting = run.shared.retrying.load(Ordering::Relaxed).min(item.connections);
                if !t.segments.is_empty() {
                    item.rec.downloaded = t.received();
                }
            }
            {
                let lanes = run.shared.lanes.lock().unwrap();
                item.lane_prev.resize(lanes.len(), 0);
                item.lane_speed.resize(lanes.len(), 0.0);
                for (i, lane) in lanes.iter().enumerate() {
                    let bytes = lane.bytes.load(Ordering::Relaxed);
                    let now_speed = bytes.saturating_sub(item.lane_prev[i]) as f64 / dt;
                    item.lane_prev[i] = bytes;
                    item.lane_speed[i] += SPEED_ALPHA * (now_speed - item.lane_speed[i]);
                    if item.lane_speed[i] < 1.0 {
                        item.lane_speed[i] = 0.0;
                    }
                }
            }
            item.rec.total = run.shared.total().or(item.rec.total);
            item.rec.resumable = run.shared.resumable.load(Ordering::Relaxed);
            if item.rec.status == Status::Downloading {
                item.rec.active_secs += dt;
            }
            delta_total += delta;
            speed_total += item.speed;
            transferring |= item.rec.status == Status::Downloading;
        }
        st.speed = speed_total;
        push_bounded(&mut st.history, speed_total as f32);
        {
            let pool = Arc::clone(&self.links.read().unwrap());
            let mut stats = self.link_stats.lock().unwrap();
            stats.resize_with(pool.links.len(), LinkStat::default);
            for (link, stat) in pool.links.iter().zip(stats.iter_mut()) {
                let bytes = link.bytes.load(Ordering::Relaxed);
                let instant = bytes.saturating_sub(stat.last_bytes) as f64 / dt;
                stat.last_bytes = bytes;
                stat.speed += SPEED_ALPHA * (instant - stat.speed);
                if stat.speed < 1.0 {
                    stat.speed = 0.0;
                }
                push_bounded(&mut stat.history, stat.speed as f32);
            }
        }
        if delta_total > 0 || transferring {
            st.stats.record_bytes(delta_total, now);
            if transferring {
                st.stats.active_secs += dt;
            }
            st.stats.peak_speed = st.stats.peak_speed.max(speed_total);
            st.stats_dirty = true;
        }
    }

    /// Starts or pauses each queue at its configured times.
    fn run_scheduler(self: &Arc<Self>) {
        use chrono::{Datelike, Timelike};
        let queues = self.config.read().unwrap().queues.clone();
        let now = chrono::Local::now();
        let key = (i64::from(now.num_days_from_ce()), (now.hour() * 60 + now.minute()) as u16);
        let previous = {
            let mut st = self.lock();
            if st.schedule_minute == Some(key) {
                return;
            }
            let previous = st.schedule_minute.replace(key);
            // Don't fire for the minute the engine happened to start in.
            let Some(previous) = previous else { return };
            previous
        };
        // The time-of-day speed limit may have started or ended.
        self.apply_speed_limits();
        let engine = Engine::from_inner(Arc::clone(self));
        // Every minute since the last look, so a start or stop time passed while
        // the computer slept still counts; the latest event of each queue wins.
        let absolute = |(day, minute): (i64, u16)| day * 1440 + i64::from(minute);
        let (from, to) = (absolute(previous), absolute(key));
        let from = from.max(to - 7 * 1440);
        for queue in queues.iter().filter(|q| q.schedule.enabled) {
            let mut last: Option<bool> = None; // Some(true) = start
            for m in from + 1..=to {
                let (day, minute) = (m.div_euclid(1440), m.rem_euclid(1440) as u16);
                // `num_days_from_ce` is 1 for Monday, 0001-01-01.
                let weekday = (day - 1).rem_euclid(7) as usize;
                if minute == queue.schedule.start_minute && queue.schedule.days[weekday] {
                    last = Some(true);
                } else if queue.schedule.stop_minute == Some(minute) && queue.schedule.stops_on(weekday) {
                    last = Some(false);
                }
            }
            match last {
                // Paused only: a download that failed for good needs a look first.
                Some(true) => {
                    for id in engine.ids_where(|i| {
                        i.rec.queue == queue.id && i.rec.status == Status::Paused && !i.rec.awaiting_link
                    }) {
                        let _ = engine.resume(id);
                    }
                }
                Some(false) => engine.stop_queue(queue.id),
                None => {}
            }
        }
    }

    fn persist(&self, force: bool) {
        self.persist_parts(force, true);
    }

    /// Saves the download list when it changed (or `force`) and, with `stats`,
    /// the statistics. Does file I/O: call it off the async threads when busy.
    fn persist_parts(&self, force: bool, stats: bool) {
        let _one_at_a_time = self.persist_lock.lock().unwrap();
        let (list, stats, log) = {
            let mut st = self.lock();
            let list = (st.dirty || force).then(|| {
                st.dirty = false;
                DownloadList { next_id: st.next_id, downloads: st.items.iter().map(|i| i.rec.clone()).collect() }
            });
            let stats = (stats && (st.stats_dirty || force)).then(|| {
                st.stats_dirty = false;
                st.stats.clone()
            });
            let log = st.log_dirty.then(|| {
                st.log_dirty = false;
                st.log.clone()
            });
            (list, stats, log)
        };
        // Persisting is best-effort; the next cycle retries.
        if let Some(list) = list {
            let _ = self.store.save_downloads(&list);
        }
        if let Some(stats) = stats {
            let _ = self.store.save_stats(&stats);
        }
        if let Some(log) = log {
            let _ = self.store.save_history(&log);
        }
    }
}

struct EngineHooks {
    inner: Arc<Inner>,
    id: DownloadId,
}

impl Hooks for EngineHooks {
    fn on_probe(&self, probe: &Probe) -> PathBuf {
        let config = self.inner.config();
        let mut st = self.inner.lock();
        st.dirty = true;
        // A download removed with its files kept, whose task is still winding
        // down, no longer owns its path: re-adding the link right away must
        // resume that data, not start a renamed copy. (One whose files are
        // being deleted keeps it, or the deletion would hit the new download.)
        let leaving = |i: &Item| i.run.as_ref().is_some_and(|r| r.on_stop == AfterStop::Remove { delete_file: false });
        let reserved: Vec<PathBuf> = st
            .items
            .iter()
            .filter(|i| i.rec.id != self.id && i.rec.path_locked && i.rec.status != Status::Completed && !leaving(i))
            .map(|i| i.rec.path.clone())
            .collect();
        let Some(item) = st.items.iter_mut().find(|i| i.rec.id == self.id) else {
            return config.download_dir.join("download");
        };
        let rec = &mut item.rec;
        rec.total = probe.total;
        rec.resumable = probe.ranges;
        if !rec.path_locked {
            let url = Url::parse(&rec.url).unwrap_or_else(|_| probe.final_url.clone());
            let name = resolve_name(&rec.options, probe, &url);
            rec.category = config.categorize(&name);
            let dir = target_dir(&config, &rec.options, rec.category);
            let overwrite = rec.options.existing.unwrap_or(config.existing_file) == ExistingFile::Overwrite;
            // Re-adding a URL whose partial download is still on disk resumes it.
            let candidate = dir.join(&name);
            let resumable_here = !reserved.iter().any(|r| crate::fsio::same_path(r, &candidate))
                && part_path(&candidate).exists()
                && ControlFile::load(&meta_path(&candidate)).is_some_and(|c| {
                    c.url == rec.url || c.matches(probe.total, probe.etag.as_deref(), probe.last_modified.as_deref())
                });
            rec.path = if resumable_here {
                candidate
            } else {
                filename::unique_path(&dir, &name, |p| {
                    (p.exists() && !overwrite)
                        || part_path(p).exists()
                        || reserved.iter().any(|r| crate::fsio::same_path(r, p))
                })
            };
            rec.file_name = rec.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
            rec.path_locked = true;
        }
        rec.path.clone()
    }

    fn on_status(&self, status: Status) {
        let mut st = self.inner.lock();
        if let Some(item) = st.items.iter_mut().find(|i| i.rec.id == self.id)
            && item.run.is_some()
            && item.rec.status.is_active()
        {
            self.inner.set_status(item, status);
        }
    }
}

async fn sampler(inner: std::sync::Weak<Inner>) {
    let period = Duration::from_millis(SAMPLE_INTERVAL_MS);
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = Instant::now();
    let mut n = 0u64;
    loop {
        tick.tick().await;
        let Some(inner) = inner.upgrade() else { return };
        if inner.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let now = Instant::now();
        inner.sample(now.duration_since(last).as_secs_f64().max(0.001));
        last = now;
        inner.enforce_cap();
        inner.run_scheduler();
        // Picks up downloads whose automatic-retry pause is over.
        inner.schedule();
        n += 1;
        if n.is_multiple_of(PERSIST_EVERY) {
            // Disk writes must not hold up a runtime thread.
            let with_stats = n.is_multiple_of(PERSIST_STATS_EVERY);
            let saver = Arc::clone(&inner);
            tokio::task::spawn_blocking(move || saver.persist_parts(false, with_stats));
        }
    }
}

fn push_bounded(buf: &mut VecDeque<f32>, value: f32) {
    if buf.len() == HISTORY_LEN {
        buf.pop_front();
    }
    buf.push_back(value);
}

fn reset_for_restart(item: &mut Item) {
    if item.rec.status != Status::Completed {
        cleanup_files(&item.rec, true);
    }
    item.auto_retries = 0;
    item.retry_at = None;
    item.rec.relaxed_resume = false;
    let rec = &mut item.rec;
    rec.path_locked = false;
    rec.downloaded = 0;
    rec.total = None;
    rec.error = None;
    rec.finished_at = None;
    rec.checksum_verified = false;
    rec.active_secs = 0.0;
    item.segments.clear();
    item.history.clear();
}

/// Deletes a download's files: the partial data of an unfinished download,
/// or the finished file.
fn cleanup_files(rec: &Record, delete_file: bool) {
    if !delete_file {
        return;
    }
    if rec.status == Status::Completed {
        let _ = std::fs::remove_file(&rec.path);
    } else if rec.path_locked {
        let _ = std::fs::remove_file(part_path(&rec.path));
        let _ = std::fs::remove_file(meta_path(&rec.path));
    }
}

/// The file name a download will get: explicit name, server-suggested name,
/// or the last URL segment, plus an extension from the MIME type if missing.
fn resolve_name(options: &DownloadOptions, probe: &Probe, url: &Url) -> String {
    let name = options
        .file_name
        .clone()
        .or_else(|| probe.file_name.clone())
        .or_else(|| filename::from_url(&probe.final_url))
        .or_else(|| filename::from_url(url))
        .unwrap_or_else(|| "download".into());
    let mut name = filename::sanitize(&name);
    if let Some(ext) = probe.stream_ext {
        // A stream is saved as one video file, not as its playlist.
        let lower = name.to_ascii_lowercase();
        if let Some(stem) =
            [".m3u8", ".m3u"].iter().find_map(|e| lower.ends_with(e).then(|| &name[..name.len() - e.len()]))
        {
            name = stem.to_owned();
        }
        if !name.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
            name = format!("{name}.{ext}");
        }
        return name;
    }
    if !name.contains('.')
        && let Some(ext) = probe.content_type.as_deref().and_then(filename::extension_for_mime)
    {
        name = format!("{name}.{ext}");
    }
    name
}

/// The queue a download may be in: `queue` if it exists, else the main queue.
fn valid_queue(config: &Config, queue: QueueId) -> QueueId {
    config.queue(queue).id
}

/// File name the browser suggested, else the last URL segment. Only for
/// matching a capture against a download that waits for a fresh link.
fn hinted_name(capture: &Capture, url: &Url) -> Option<String> {
    browser_name(capture, url).or_else(|| filename::from_url(url).map(|n| filename::sanitize(&n)))
}

/// The name the browser chose for a capture, when it says more than the
/// link does. A name that is just the URL's last segment (often an id such as
/// `5a26b454-bf1a-…` on signed storage links) is not used as the file name:
/// it would hide the real name the server sends in `Content-Disposition`.
fn browser_name(capture: &Capture, url: &Url) -> Option<String> {
    let name = capture
        .file_name
        .as_deref()
        .map(|n| n.rsplit(['/', '\\']).next().unwrap_or(n).trim().to_owned())
        .filter(|n| !n.is_empty())?;
    let from_link = filename::from_url(url).map(|n| filename::sanitize(&n));
    let name = filename::sanitize(&name);
    (from_link.as_deref() != Some(name.as_str())).then_some(name)
}

/// Same file name, ignoring case and a ` (n)` de-duplication suffix.
fn same_file_name(a: &str, b: &str) -> bool {
    fn base(name: &str) -> String {
        let lower = name.to_lowercase();
        let (stem, ext) = match lower.rfind('.') {
            Some(i) if i > 0 => lower.split_at(i),
            _ => (lower.as_str(), ""),
        };
        let stem = match stem.rfind(" (") {
            Some(i) if stem.ends_with(')') && stem[i + 2..stem.len() - 1].bytes().all(|b| b.is_ascii_digit()) => {
                &stem[..i]
            }
            _ => stem,
        };
        format!("{stem}{ext}")
    }
    base(a) == base(b)
}

fn merge_cookie(headers: &mut Vec<(String, String)>, cookies: Option<&str>) {
    if let Some(c) = cookies.filter(|c| !c.trim().is_empty()) {
        headers.retain(|(k, _)| !k.eq_ignore_ascii_case("cookie"));
        headers.push(("Cookie".into(), c.trim().to_owned()));
    }
}

fn target_dir(config: &Config, options: &DownloadOptions, category: Category) -> PathBuf {
    if let Some(dir) = config.category_dirs.get(&category).filter(|_| options.directory.is_none()) {
        return dir.clone();
    }
    match &options.directory {
        Some(dir) => dir.clone(),
        None => match config.category_folder(category) {
            Some(sub) if config.auto_categorize => config.download_dir.join(sub),
            _ => config.download_dir.clone(),
        },
    }
}

fn parse_url(input: &str) -> Result<Url, Error> {
    let input = input.trim();
    let url = Url::parse(input).or_else(|e| match e {
        url::ParseError::RelativeUrlWithoutBase => Url::parse(&format!("https://{input}")),
        e => Err(e),
    });
    let url = url.map_err(|e| Error::InvalidUrl(format!("{input}: {e}")))?;
    match url.scheme() {
        "http" | "https" | "ftp" if url.host().is_some() => Ok(url),
        "http" | "https" | "ftp" => Err(Error::InvalidUrl(format!("{input}: missing host"))),
        other => Err(Error::InvalidUrl(format!("unsupported scheme `{other}`"))),
    }
}

/// The links downloads use: the enabled adapters that are really present when
/// bonding is on, otherwise the default connection.
fn build_pool(config: &Config, base: &Client) -> LinkPool {
    let present = crate::net::adapters();
    let mut links: Vec<Arc<Link>> = Vec::new();
    if config.multi_link {
        for l in config.interfaces.iter().filter(|l| l.enabled) {
            let Ok(addr) = l.address.parse::<std::net::IpAddr>() else { continue };
            if !present.iter().any(|a| a.address == addr) {
                continue; // unplugged or switched off
            }
            if let Ok(client) = build_client_for(config, Some(addr)) {
                let label = if l.name.trim().is_empty() { addr.to_string() } else { l.name.clone() };
                links.push(Arc::new(Link::new(label, Some(addr), client, l.limit)));
            }
        }
    }
    if links.is_empty() {
        links.push(Arc::new(Link::new("Default connection", None, base.clone(), 0)));
    }
    LinkPool { links }
}

fn build_client(config: &Config) -> Result<Client, Error> {
    build_client_for(config, None)
}

fn build_client_for(config: &Config, local: Option<std::net::IpAddr>) -> Result<Client, Error> {
    let mut headers = HeaderMap::new();
    // We want the bytes exactly as stored; compression would break ranges.
    headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    let mut builder = Client::builder()
        .user_agent(config.user_agent.clone())
        .default_headers(headers)
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(30))
        .pool_max_idle_per_host(64)
        .redirect(reqwest::redirect::Policy::limited(16))
        .tls_danger_accept_invalid_certs(config.insecure_tls);
    if let Some(addr) = local {
        builder = builder.local_address(addr);
    }
    if config.http1_only {
        builder = builder.http1_only();
    }
    if let Some(proxy) = &config.proxy {
        let proxy = reqwest::Proxy::all(proxy.as_str()).map_err(|e| Error::InvalidUrl(format!("proxy: {e}")))?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| Error::Network(e.to_string()))
}

fn history_entry(rec: &Record, default_connections: usize) -> HistoryEntry {
    HistoryEntry {
        id: rec.id,
        url: rec.url.clone(),
        host: crate::host_of(&rec.url),
        file_name: rec.file_name.clone(),
        path: rec.path.clone(),
        category: rec.category,
        size: rec.total.or((rec.downloaded > 0).then_some(rec.downloaded)),
        status: rec.status,
        error: rec.error.clone(),
        added_at: rec.added_at,
        finished_at: rec.finished_at.unwrap_or_else(util::now_unix),
        active_secs: rec.active_secs as u64,
        connections: rec.options.connections.unwrap_or(default_connections),
        checksum_verified: rec.checksum_verified,
        tags: Vec::new(),
        notes: Vec::new(),
    }
}

/// What other devices may hear about now that `items[pos]` finished (or failed
/// for good): the download itself, its queue running dry, everything done.
fn notify_events(
    items: &[Item],
    pos: usize,
    failed: bool,
    config: &Config,
) -> Vec<(crate::notifiers::Event, crate::notifiers::Message)> {
    use crate::notifiers::{Event, Message};
    let rec = &items[pos].rec;
    let queue = rec.queue;
    let waiting = |i: &Item| i.rec.status == Status::Queued || i.rec.status.is_active();
    let mut events = Vec::new();

    let host = crate::history::host_of(&rec.url);
    let from = if host.is_empty() { String::new() } else { format!(" from {host}") };
    if failed {
        let error = rec.error.clone().unwrap_or_else(|| "unknown error".to_owned());
        events.push((
            Event::Failed { queue },
            Message {
                title: format!("Download failed: {}", rec.file_name),
                body: format!("{}{from} stopped: {error}.", rec.file_name),
                problem: true,
            },
        ));
    } else {
        let size = rec.total.map_or(String::new(), |t| format!(" ({})", util::format_bytes(t)));
        let folder = rec.path.parent().map(|d| d.display().to_string()).unwrap_or_default();
        events.push((
            Event::Finished { queue },
            Message {
                title: format!("Downloaded {}", rec.file_name),
                body: format!("{}{size}{from} is ready in {folder}.", rec.file_name),
                problem: false,
            },
        ));
    }

    // How a set of downloads ended, e.g. "5 downloaded, 1 failed, 2 paused".
    let tally = |of: &mut dyn Iterator<Item = &Item>| {
        let (mut done, mut bad, mut paused) = (0, 0, 0);
        for i in of {
            match i.rec.status {
                Status::Completed => done += 1,
                Status::Failed => bad += 1,
                Status::Paused => paused += 1,
                _ => {}
            }
        }
        let mut parts = vec![format!("{done} downloaded")];
        if bad > 0 {
            parts.push(format!("{bad} failed"));
        }
        if paused > 0 {
            parts.push(format!("{paused} paused"));
        }
        (parts.join(", "), bad > 0)
    };
    if !items.iter().any(|i| i.rec.queue == queue && waiting(i)) {
        let name = &config.queue(queue).name;
        let (summary, problem) = tally(&mut items.iter().filter(|i| i.rec.queue == queue));
        events.push((
            Event::QueueFinished { queue },
            Message {
                title: format!("Queue finished: {name}"),
                body: format!("Nothing left to download in {name}: {summary}."),
                problem,
            },
        ));
    }
    if !items.iter().any(waiting) {
        let (summary, problem) = tally(&mut items.iter());
        events.push((
            Event::AllFinished,
            Message {
                title: "All downloads finished".to_owned(),
                body: format!("Nothing left to download: {summary}."),
                problem,
            },
        ));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_matching() {
        assert!(same_file_name("Setup (2).exe", "setup.exe"));
        assert!(same_file_name("a.tar.gz", "A.tar.gz"));
        assert!(!same_file_name("a (x).zip", "a.zip"));
        assert!(!same_file_name("b.zip", "a.zip"));
    }

    #[test]
    fn urls() {
        assert_eq!(parse_url("example.com/a.zip").unwrap().as_str(), "https://example.com/a.zip");
        assert!(parse_url("ftp://example.com/a").is_ok());
        assert!(parse_url("gopher://example.com/a").is_err());
        assert!(parse_url("http://").is_err());
    }
}
