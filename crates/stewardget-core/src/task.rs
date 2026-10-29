//! Runs a single download from probe to final rename.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::control::{self, ControlFile};
use crate::limiter::RateLimiter;
use crate::net::{LaneStat, Link, LinkPool};
use crate::probe::{self, Probe};
use crate::segment::{SegmentTable, UNBOUNDED};
use crate::{Config, DownloadOptions, Error, ExistingFile, Status, filename, fsio, ftp, util};

/// Bytes buffered per connection before a positional write.
const WRITE_BUFFER: usize = 512 * 1024;
/// How often the control file is checkpointed (each one flushes the file to disk).
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(3);

/// Callbacks into the engine.
pub(crate) trait Hooks: Send + Sync + 'static {
    /// The server has been probed; returns the destination path to use.
    fn on_probe(&self, probe: &Probe) -> PathBuf;
    fn on_status(&self, status: Status);
}

/// Live state of a running download, read by the engine's sampler.
pub(crate) struct Shared {
    pub table: Mutex<SegmentTable>,
    /// `u64::MAX` while unknown.
    pub total: AtomicU64,
    pub resumable: AtomicBool,
    /// Raw bytes pulled off the network (drives speed and statistics).
    pub transferred: AtomicU64,
    /// Connections waiting to retry after an error (shown as "Reconnecting").
    pub retrying: AtomicUsize,
    pub limiter: RateLimiter,
    /// Where connections go; see [`crate::net`].
    pub lanes: Mutex<Vec<Arc<LaneStat>>>,
    /// Connections the auto-tuner is currently allowing; `0` = not tuned.
    pub target_connections: AtomicUsize,
}

impl Shared {
    pub fn new(speed_limit: u64) -> Self {
        Self {
            table: Mutex::new(SegmentTable::default()),
            total: AtomicU64::new(u64::MAX),
            resumable: AtomicBool::new(false),
            transferred: AtomicU64::new(0),
            retrying: AtomicUsize::new(0),
            limiter: RateLimiter::new(speed_limit),
            lanes: Mutex::new(Vec::new()),
            target_connections: AtomicUsize::new(0),
        }
    }

    pub fn total(&self) -> Option<u64> {
        Some(self.total.load(Ordering::Relaxed)).filter(|&t| t != u64::MAX)
    }
}

pub(crate) struct Job {
    pub url: Url,
    pub options: DownloadOptions,
    pub config: Config,
    pub client: Client,
    /// The ways out to the network (one, unless several adapters are in use).
    pub links: Arc<LinkPool>,
    pub global_limiter: Arc<RateLimiter>,
    /// Shared by the downloads of this download's queue.
    pub queue_limiter: Arc<RateLimiter>,
    pub shared: Arc<Shared>,
    pub cancel: CancellationToken,
    pub hooks: Arc<dyn Hooks>,
    /// Resume when only the size matches (the user replaced the link).
    pub relaxed_resume: bool,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Completed { path: PathBuf, total: u64, verified: bool },
    Cancelled,
    Failed(Error),
}

pub(crate) async fn run(job: Job) -> Outcome {
    match run_inner(&job).await {
        Ok(outcome) => outcome,
        Err(_) if job.cancel.is_cancelled() => Outcome::Cancelled,
        Err(Error::Cancelled) => Outcome::Cancelled,
        Err(e) => Outcome::Failed(e),
    }
}

/// Request headers for a download: custom headers, `Referer` and `User-Agent`.
pub(crate) fn request_headers(options: &DownloadOptions) -> HeaderMap {
    let mut headers = build_headers(&options.headers);
    if let Some(referrer) = options.referrer.as_deref().and_then(|r| HeaderValue::from_str(r).ok()) {
        headers.insert(header::REFERER, referrer);
    }
    if let Some(ua) = options.user_agent.as_deref().and_then(|u| HeaderValue::from_str(u).ok()) {
        headers.insert(header::USER_AGENT, ua);
    }
    headers
}

async fn run_inner(job: &Job) -> Result<Outcome, Error> {
    let headers = request_headers(&job.options);
    let probe = match probe_with_retry(job, &job.url, &headers).await {
        // The stable link stopped working (a one-time link the browser used
        // up): the address the browser was finally sent to may still work.
        Err(e @ Error::Http(400..=499)) => match fallback(job) {
            Some(url) => probe_with_retry(job, &url, &headers).await.map_err(|_| e)?,
            None => return Err(e),
        },
        other => other?,
    };
    if crate::hls::is_playlist(&job.url, &probe) {
        return crate::hls::run(job, &headers, probe).await;
    }
    let dest = job.hooks.on_probe(&probe);
    let part = control::part_path(&dest);
    let meta = control::meta_path(&dest);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let ranged = probe.ranges && probe.total.is_some_and(|t| t > 0);
    let min_split = job.config.min_split_size;
    let connections = job.options.connections.unwrap_or(job.config.connections).clamp(1, 64);

    // Partial data on disk is never thrown away silently: if it can't be
    // continued, stop and let the user decide (resume anyway, new link, restart).
    let existing = ControlFile::load(&meta).filter(|c| part.exists() && c.has_data());
    let resumed = match existing {
        None => None,
        Some(_) if !ranged => return Err(Error::ResumeUnsupported),
        Some(c) if c.total != probe.total => {
            let show = |t: Option<u64>| t.map_or("unknown".into(), util::format_bytes);
            return Err(Error::SizeChanged { old: show(c.total), new: show(probe.total) });
        }
        Some(c)
            if !job.relaxed_resume
                && !c.matches(probe.total, probe.etag.as_deref(), probe.last_modified.as_deref()) =>
        {
            return Err(Error::RemoteChanged);
        }
        Some(c) => {
            let table = SegmentTable::from_persisted(&c.segments, probe.total.unwrap_or(0))
                .ok_or_else(|| Error::Io("the progress file is damaged".into()))?;
            // A partial file cut short or replaced since would finish with holes.
            let needed = c.segments.iter().map(|&(start, _, written)| start + written).max().unwrap_or(0);
            if std::fs::metadata(&part).map_or(0, |m| m.len()) < needed {
                return Err(Error::Io(
                    "the partial file is shorter than the progress file says; restart the download".into(),
                ));
            }
            Some(table)
        }
    };
    let fresh = resumed.is_none();
    let table = match (resumed, probe.total) {
        (Some(t), _) => t,
        (None, Some(total)) if ranged => SegmentTable::split_evenly(total, connections, min_split),
        (None, total) => SegmentTable::single(total.unwrap_or(UNBOUNDED)),
    };

    // Fail early instead of filling the disk.
    if let Some(total) = probe.total {
        let needed = total.saturating_sub(table.written());
        let dir = dest.parent().unwrap_or(Path::new("."));
        // A sparse pre-allocated file doesn't reserve space, so count what's missing.
        if let Some(available) = fsio::free_space(dir)
            && available < needed.saturating_add(8 << 20)
        {
            return Err(Error::DiskFull {
                needed: util::format_bytes(needed),
                available: util::format_bytes(available),
            });
        }
    }

    let file = OpenOptions::new().read(true).write(true).create(true).truncate(fresh).open(&part)?;
    if fresh && ranged {
        fsio::preallocate(&file, probe.total.unwrap_or(0))?;
    }

    let remaining = probe.total.map_or(u64::MAX, |t| t.saturating_sub(table.written()));
    let shared = &job.shared;
    *shared.table.lock().unwrap() = table;
    shared.total.store(probe.total.unwrap_or(u64::MAX), Ordering::Relaxed);
    shared.resumable.store(ranged, Ordering::Relaxed);
    job.hooks.on_status(Status::Downloading);

    let mut ctl = ControlFile::new(job.url.to_string(), probe.total, probe.etag.clone(), probe.last_modified.clone());
    if ranged {
        ctl.segments = shared.table.lock().unwrap().to_persisted();
        let _ = ctl.save(&meta);
    }

    let mut max_workers = if ranged { (remaining / min_split).clamp(1, connections as u64) as usize } else { 1 };
    let lanes = build_lanes(job, &probe, &headers, ranged).await;
    // Every lane should get a connection, whatever the configured count.
    if ranged {
        max_workers = max_workers.max(lanes.len().min(64));
    }
    // Auto-tuning: begin modestly and grow while more connections still help.
    let start = TUNE_START.max(lanes.len());
    let tuned = ranged && job.config.auto_tune && job.options.connections.is_none() && max_workers > start;
    let mut spawned = if tuned { start } else { max_workers };
    job.shared.target_connections.store(if tuned { spawned } else { 0 }, Ordering::Relaxed);
    *job.shared.lanes.lock().unwrap() = lanes.iter().map(|l| Arc::clone(&l.stat)).collect();
    let file = Arc::new(file);
    let ctx = Arc::new(WorkerCtx {
        client: job.client.clone(),
        url: std::sync::RwLock::new(probe.final_url.clone()),
        origin: job.url.clone(),
        redirected: probe.final_url != job.url,
        total: probe.total,
        relinks: tokio::sync::Mutex::new(0),
        headers,
        ranged,
        etag: probe.etag.clone(),
        file: Arc::clone(&file),
        shared: Arc::clone(shared),
        global: Arc::clone(&job.global_limiter),
        queue: Arc::clone(&job.queue_limiter),
        lanes,
        stop: job.cancel.child_token(),
        min_split,
        max_retries: job.config.max_retries,
        read_timeout: Duration::from_secs(job.config.read_timeout_secs),
        header_timeout: Duration::from_secs(job.config.read_timeout_secs.min(15)),
    });

    let mut set = JoinSet::new();
    for n in 0..spawned {
        set.spawn(worker(Arc::clone(&ctx), n));
    }
    let mut tuner = Tuner::default();
    let mut tick = tokio::time::interval(CHECKPOINT_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut first_error: Option<Error> = None;
    let mut tune_tick = tokio::time::interval(TUNE_INTERVAL);
    tune_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            joined = set.join_next() => match joined {
                None => break,
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(e))) => {
                    if !e.is_retryable() {
                        ctx.stop.cancel(); // fatal for the whole download
                    }
                    first_error.get_or_insert(e);
                }
                Some(Err(join)) => {
                    ctx.stop.cancel();
                    first_error.get_or_insert(Error::Io(format!("connection task failed: {join}")));
                }
            },
            _ = tick.tick(), if ranged => checkpoint(&mut ctl, shared, &meta, &file).await,
            _ = tune_tick.tick(), if tuned && spawned < max_workers => {
                let moved = shared.transferred.load(Ordering::Relaxed);
                if !ctx.stop.is_cancelled() && !shared.table.lock().unwrap().is_complete() {
                    match tuner.step(moved, spawned, max_workers) {
                        Tune::Grow(add) => {
                            for n in spawned..spawned + add {
                                set.spawn(worker(Arc::clone(&ctx), n));
                            }
                            spawned += add;
                            shared.target_connections.store(spawned, Ordering::Relaxed);
                        }
                        // Growth stopped helping: this is the right count.
                        Tune::Settled => max_workers = spawned,
                        Tune::Wait => {}
                    }
                }
            }
        }
    }
    drop(ctx);

    if !shared.table.lock().unwrap().is_complete() {
        if ranged {
            // Keep every byte that made it to disk, whatever went wrong.
            checkpoint(&mut ctl, shared, &meta, &file).await;
        }
        if job.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        return Err(first_error.unwrap_or_else(|| Error::Network("connections closed before completion".into())));
    }

    finalize(job, &probe, file, &dest, &part, &meta).await
}

pub(crate) async fn finalize(
    job: &Job,
    probe: &Probe,
    file: Arc<std::fs::File>,
    dest: &Path,
    part: &Path,
    meta: &Path,
) -> Result<Outcome, Error> {
    let written = job.shared.table.lock().unwrap().written();
    if let Some(expected) = probe.total
        && expected != written
    {
        return Err(Error::SizeMismatch { expected, actual: written });
    }
    job.shared.total.store(written, Ordering::Relaxed);

    // Without a known size, a retry starts over at byte 0; an attempt that got
    // further before it broke must not leave its tail behind. Then flush and
    // close the handle (Windows can't rename an open file).
    let size_unknown = probe.total.is_none();
    tokio::task::spawn_blocking(move || {
        if size_unknown {
            file.set_len(written)?;
        }
        file.sync_all()
    })
    .await
    .map_err(std::io::Error::other)??;

    let verified = match job.options.checksum.clone() {
        Some(checksum) => {
            job.hooks.on_status(Status::Verifying);
            let path = part.to_path_buf();
            let res =
                tokio::task::spawn_blocking(move || checksum.verify(&path)).await.map_err(std::io::Error::other)?;
            if let Err(e) = res {
                let _ = std::fs::remove_file(meta);
                return Err(e);
            }
            true
        }
        None => false,
    };

    // Unless told to overwrite, never clobber a file that appeared while we were downloading.
    let final_path = if job.config.existing_file == ExistingFile::Overwrite {
        dest.to_path_buf()
    } else {
        let dir = dest.parent().unwrap_or(Path::new("."));
        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        filename::unique_path(dir, &name, |p| p.exists())
    };
    let (from, to) = (part.to_path_buf(), final_path.clone());
    tokio::task::spawn_blocking(move || fsio::rename(&from, &to)).await.map_err(std::io::Error::other)??;
    let _ = std::fs::remove_file(meta);
    if let Some(lm) = &probe.last_modified {
        fsio::set_mtime_from_http(&final_path, lm);
    }
    Ok(Outcome::Completed { path: final_path, total: written, verified })
}

/// Records the finished ranges. They are read first and the data is flushed
/// afterwards, so the control file never claims bytes a crash or power cut
/// could still lose. The flush runs on the blocking pool.
async fn checkpoint(ctl: &mut ControlFile, shared: &Shared, meta: &Path, file: &Arc<std::fs::File>) {
    let segments = shared.table.lock().unwrap().to_persisted();
    let flushed = Arc::clone(file);
    if !matches!(tokio::task::spawn_blocking(move || flushed.sync_data()).await, Ok(Ok(()))) {
        return; // keep the older, safe checkpoint
    }
    ctl.segments = segments;
    let (ctl, meta) = (ctl.clone(), meta.to_path_buf());
    // A failed checkpoint only costs resume precision; the next tick retries.
    let _ = tokio::task::spawn_blocking(move || ctl.save(&meta)).await;
}

/// Same scheme, host and port.
pub(crate) fn same_origin(a: &Url, b: &Url) -> bool {
    a.origin() == b.origin()
}

/// The request headers minus the ones that carry credentials.
pub(crate) fn without_credentials(mut headers: HeaderMap) -> HeaderMap {
    headers.remove(header::COOKIE);
    headers.remove(header::AUTHORIZATION);
    headers.remove(header::PROXY_AUTHORIZATION);
    headers
}

fn build_headers(pairs: &[(String, String)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.trim().as_bytes()), HeaderValue::from_str(v.trim())) {
            map.append(k, v);
        }
    }
    map
}

/// The browser's final address for a captured download, if there is one.
fn fallback(job: &Job) -> Option<Url> {
    job.options.fallback_url.as_deref().and_then(|u| Url::parse(u).ok()).filter(|u| *u != job.url)
}

async fn probe_with_retry(job: &Job, url: &Url, headers: &HeaderMap) -> Result<Probe, Error> {
    let mut attempt = 0;
    loop {
        let result = tokio::select! {
            biased;
            _ = job.cancel.cancelled() => return Err(Error::Cancelled),
            // A server that accepts the connection and never answers must not
            // leave the download "Connecting" forever.
            r = tokio::time::timeout(Duration::from_secs(job.config.read_timeout_secs), probe::probe(&job.client, url, headers)) => r.unwrap_or(Err(Error::Timeout)),
        };
        match result {
            Ok(p) => return Ok(p),
            Err(e) if e.is_retryable() && attempt < job.config.max_retries => {
                attempt += 1;
                backoff(attempt, e.retry_after(), &job.cancel).await?;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Exponential back-off (0.5 s · 2ⁿ, capped at 30 s) with up to 50 % jitter.
pub(crate) async fn backoff(attempt: u32, server_asked: Option<u64>, cancel: &CancellationToken) -> Result<(), Error> {
    let base = (500u64 << attempt.min(6)).min(30_000);
    let mut delay = Duration::from_millis(base + util::jitter_ms(base / 2));
    // A server that says how long to wait is obeyed.
    if let Some(secs) = server_asked {
        delay = delay.max(Duration::from_secs(secs));
    }
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

struct WorkerCtx {
    client: Client,
    /// Where segments are fetched: the link after redirects. A signed link
    /// that expires (GitHub, S3) is replaced by a fresh one, see [`relink`].
    url: std::sync::RwLock<Url>,
    /// The link as added, which hands out fresh redirects.
    origin: Url,
    redirected: bool,
    total: Option<u64>,
    /// Fresh redirects fetched so far.
    relinks: tokio::sync::Mutex<u32>,
    /// All request headers; credentials are dropped for other hosts.
    headers: HeaderMap,
    ranged: bool,
    etag: Option<String>,
    file: Arc<std::fs::File>,
    shared: Arc<Shared>,
    global: Arc<RateLimiter>,
    queue: Arc<RateLimiter>,
    lanes: Vec<Lane>,
    stop: CancellationToken,
    min_split: u64,
    max_retries: u32,
    read_timeout: Duration,
    /// Time allowed until response headers arrive; stalled servers are detected sooner.
    header_timeout: Duration,
}

/// Most fresh redirects fetched for one run of a download.
const MAX_RELINKS: u32 = 50;

impl WorkerCtx {
    fn table(&self) -> std::sync::MutexGuard<'_, SegmentTable> {
        self.shared.table.lock().unwrap()
    }

    fn current_url(&self) -> Url {
        self.url.read().unwrap().clone()
    }

    /// Where `lane` fetches from.
    fn lane_url(&self, lane: &Lane) -> Url {
        lane.mirror_url.clone().unwrap_or_else(|| self.current_url())
    }

    fn alive_lanes(&self) -> usize {
        self.lanes.iter().filter(|l| l.stat.is_alive()).count()
    }

    /// The first lane at or after `from` (wrapping) that is still alive.
    fn alive_lane(&self, from: usize) -> usize {
        let n = self.lanes.len();
        (0..n).map(|i| (from + i) % n).find(|&i| self.lanes[i].stat.is_alive()).unwrap_or(from % n)
    }

    /// Request headers for `url`: cookies and passwords belong to the host
    /// they were meant for, not to wherever a redirect ends up.
    fn headers_for(&self, url: &Url) -> HeaderMap {
        if same_origin(&self.origin, url) { self.headers.clone() } else { without_credentials(self.headers.clone()) }
    }

    /// Whether `e` may mean that the redirected link expired. Signed links
    /// (GitHub release assets, S3, CDNs) stop working after a few minutes;
    /// open connections keep going, but every new range request is refused.
    fn link_expired(&self, e: &Error) -> bool {
        self.redirected && matches!(e, Error::Http(401 | 403 | 404 | 410))
    }

    /// Asks the original link for a fresh redirect, after `failed` stopped
    /// working, and checks that it still serves the same file. Several
    /// connections may fail at once: only the first one asks.
    async fn relink(&self, failed: &Url) -> Result<(), Error> {
        let mut count = self.relinks.lock().await;
        if self.current_url() != *failed {
            return Ok(()); // another connection already got a fresh one
        }
        if *count >= MAX_RELINKS {
            return Err(Error::Http(403));
        }
        *count += 1;
        let probe = tokio::select! {
            biased;
            _ = self.stop.cancelled() => return Err(Error::Cancelled),
            r = tokio::time::timeout(self.read_timeout, probe::probe(&self.client, &self.origin, &self.headers)) => {
                r.map_err(|_| Error::Timeout)??
            }
        };
        if probe.total != self.total {
            let show = |t: Option<u64>| t.map_or("unknown".into(), util::format_bytes);
            return Err(Error::SizeChanged { old: show(self.total), new: show(probe.total) });
        }
        if let (Some(a), Some(b)) = (&self.etag, &probe.etag)
            && a != b
        {
            return Err(Error::RemoteChanged);
        }
        *self.url.write().unwrap() = probe.final_url;
        Ok(())
    }
}

/// One connection: claims segments (stealing work when idle) until none are left.
/// `first_lane` spreads connections over the lanes; a connection moves to
/// another lane when its own keeps failing.
async fn worker(ctx: Arc<WorkerCtx>, first_lane: usize) -> Result<(), Error> {
    let mut lane = ctx.alive_lane(first_lane);
    ctx.lanes[lane].stat.workers.fetch_add(1, Ordering::Relaxed);
    let result = work(&ctx, &mut lane).await;
    ctx.lanes[lane].stat.workers.fetch_sub(1, Ordering::Relaxed);
    result
}

async fn work(ctx: &Arc<WorkerCtx>, lane: &mut usize) -> Result<(), Error> {
    loop {
        if ctx.stop.is_cancelled() {
            return Ok(());
        }
        let Some(idx) = ctx.table().claim(ctx.min_split) else { return Ok(()) };
        let mut attempt = 0;
        loop {
            if !ctx.lanes[*lane].stat.is_alive() {
                let next = ctx.alive_lane(*lane);
                ctx.lanes[*lane].stat.workers.fetch_sub(1, Ordering::Relaxed);
                ctx.lanes[next].stat.workers.fetch_add(1, Ordering::Relaxed);
                *lane = next;
            }
            let this = &ctx.lanes[*lane];
            let before = ctx.table().segments[idx].written;
            let url = ctx.lane_url(this);
            match fetch_segment(ctx, idx, this, &url).await {
                Ok(()) => {
                    ctx.table().segments[idx].active = false;
                    break;
                }
                Err(Error::Cancelled) => {
                    ctx.table().release(idx);
                    return Ok(());
                }
                Err(e) if this.mirror_url.is_none() && ctx.link_expired(&e) => match ctx.relink(&url).await {
                    Ok(()) => {} // try again with the fresh link
                    Err(Error::Cancelled) => {
                        ctx.table().release(idx);
                        return Ok(());
                    }
                    Err(fresh) => {
                        ctx.table().release(idx);
                        // The original error explains more than a failed second look.
                        return Err(if fresh.is_retryable() { e } else { fresh });
                    }
                },
                Err(e) => {
                    let progressed = ctx.table().segments[idx].written > before;
                    if progressed {
                        attempt = 0; // made progress: the connection is flaky, not dead
                    }
                    // Another lane can carry on where this one gave up: a mirror
                    // that serves something else, a network link that went down.
                    if ctx.alive_lanes() > 1 && (!e.is_retryable() || (!progressed && attempt >= 2)) {
                        this.stat.failed.store(true, Ordering::Relaxed);
                        *this.stat.reason.lock().unwrap() = Some(e.to_string());
                        attempt = 0;
                        continue; // the top of the loop moves to a live lane
                    }
                    if !e.is_retryable() || attempt >= ctx.max_retries {
                        ctx.table().release(idx);
                        return Err(e);
                    }
                    // Servers that cap connections per client stall or reject the
                    // extra ones. If this connection keeps failing while siblings
                    // are healthy, bow out; they will steal its range.
                    if !progressed && attempt >= 2 {
                        let mut t = ctx.table();
                        if t.active() > 1 {
                            t.release(idx);
                            return Ok(());
                        }
                    }
                    attempt += 1;
                    ctx.shared.retrying.fetch_add(1, Ordering::Relaxed);
                    let waited = backoff(attempt, e.retry_after(), &ctx.stop).await;
                    ctx.shared.retrying.fetch_sub(1, Ordering::Relaxed);
                    if waited.is_err() {
                        ctx.table().release(idx);
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Streams one segment into the file. Returns `Ok` once the segment's
/// (possibly shrunk) end is reached.
async fn fetch_segment(ctx: &WorkerCtx, idx: usize, lane: &Lane, url: &Url) -> Result<(), Error> {
    let (offset, end) = {
        let mut t = ctx.table();
        let s = &mut t.segments[idx];
        if !ctx.ranged {
            s.written = 0; // can't resume without range support
        }
        s.received = s.written;
        (s.cursor(), s.end)
    };
    if offset >= end {
        return Ok(());
    }

    let mut stream: ftp::ByteStream = if url.scheme() == "ftp" {
        tokio::select! {
            biased;
            _ = ctx.stop.cancelled() => return Err(Error::Cancelled),
            r = tokio::time::timeout(ctx.header_timeout, ftp::open(url, offset, lane.link.address)) => {
                r.map_err(|_| Error::Timeout)??
            }
        }
    } else {
        open_http(ctx, lane, url, offset, end).await?
    };
    let mut buf = Vec::with_capacity(WRITE_BUFFER);
    let mut buf_offset = offset;
    let result = loop {
        let next = tokio::select! {
            biased;
            _ = ctx.stop.cancelled() => break Err(Error::Cancelled),
            n = tokio::time::timeout(ctx.read_timeout, stream.next()) => n,
        };
        let chunk = match next {
            Err(_) => break Err(Error::Timeout),
            Ok(None) => break Ok(false),
            Ok(Some(Err(e))) => break Err(e),
            Ok(Some(Ok(chunk))) => chunk,
        };
        // Speed limits may make this wait a while; pausing must not.
        tokio::select! {
            biased;
            _ = ctx.stop.cancelled() => break Err(Error::Cancelled),
            _ = async {
                ctx.shared.limiter.acquire(chunk.len()).await;
                lane.link.limiter.acquire(chunk.len()).await;
                ctx.queue.acquire(chunk.len()).await;
                ctx.global.acquire(chunk.len()).await;
            } => {}
        }

        let (take, reached_end) = {
            let mut t = ctx.table();
            let s = &mut t.segments[idx];
            // `end` may have shrunk because another connection stole our tail.
            let take = s.remaining().min(chunk.len() as u64) as usize;
            s.received += take as u64;
            (take, s.cursor() >= s.end)
        };
        ctx.shared.transferred.fetch_add(take as u64, Ordering::Relaxed);
        lane.stat.bytes.fetch_add(take as u64, Ordering::Relaxed);
        lane.link.bytes.fetch_add(take as u64, Ordering::Relaxed);
        buf.extend_from_slice(&chunk[..take]);

        if buf.len() >= WRITE_BUFFER || reached_end {
            let len = buf.len() as u64;
            match flush(ctx, idx, buf_offset, std::mem::take(&mut buf)).await {
                Ok(b) => {
                    buf = b;
                    buf_offset += len;
                }
                Err(e) => break Err(e),
            }
        }
        if reached_end {
            break Ok(true);
        }
    };

    // Whatever arrived before an error is valid data; keep it.
    if !buf.is_empty() {
        flush(ctx, idx, buf_offset, buf).await?;
    }
    match result {
        Ok(true) => Ok(()),
        Ok(false) if end == UNBOUNDED => {
            let mut t = ctx.table();
            let s = &mut t.segments[idx];
            s.end = s.start + s.written;
            Ok(())
        }
        Ok(false) => Err(Error::Network("connection closed early".into())),
        Err(e) => Err(e),
    }
}

/// Sends the ranged request for a segment and checks the answer.
async fn open_http(ctx: &WorkerCtx, lane: &Lane, url: &Url, offset: u64, end: u64) -> Result<ftp::ByteStream, Error> {
    let mut req = lane.link.client.get(url.clone()).headers(ctx.headers_for(url));
    if ctx.ranged {
        req = req.header(header::RANGE, format!("bytes={offset}-{}", end - 1));
        if let Some(etag) = &ctx.etag {
            req = req.header(header::IF_RANGE, etag);
        }
    }
    let resp = tokio::select! {
        biased;
        _ = ctx.stop.cancelled() => return Err(Error::Cancelled),
        r = tokio::time::timeout(ctx.header_timeout, req.send()) => r.map_err(|_| Error::Timeout)??,
    };
    let status = resp.status();
    if ctx.ranged {
        match status {
            StatusCode::PARTIAL_CONTENT => {
                let start = resp
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(probe::parse_content_range_start);
                if start != Some(offset) {
                    return Err(Error::RangeIgnored);
                }
            }
            StatusCode::OK if ctx.etag.is_some() => return Err(Error::RemoteChanged),
            StatusCode::OK => return Err(Error::RangeIgnored),
            _ => return Err(probe::http_error(&resp)),
        }
    } else if !status.is_success() {
        return Err(probe::http_error(&resp));
    }

    Ok(Box::pin(resp.bytes_stream().map(|r| r.map_err(Error::from))))
}

async fn flush(ctx: &WorkerCtx, idx: usize, offset: u64, buf: Vec<u8>) -> Result<Vec<u8>, Error> {
    let len = buf.len() as u64;
    let mut buf = fsio::write_at(&ctx.file, offset, buf).await?;
    ctx.table().segments[idx].written += len;
    buf.clear();
    Ok(buf)
}

/// Connections a download starts with when auto-tuning.
const TUNE_START: usize = 4;
/// How often the tuner looks at the speed.
const TUNE_INTERVAL: Duration = Duration::from_millis(1500);
/// A step must gain at least this much to be worth another one.
const TUNE_GAIN: f64 = 1.10;
/// How long a mirror gets to answer the check before being skipped.
const MIRROR_CHECK: Duration = Duration::from_secs(10);

/// One network link talking to one source.
pub(crate) struct Lane {
    pub link: Arc<Link>,
    /// `None` for the file's own address (which may be refreshed), otherwise a mirror.
    pub mirror_url: Option<Url>,
    pub stat: Arc<LaneStat>,
}

/// Lanes of a download: every network link times every usable source. A
/// mirror is only used once it answered a range request with the same size.
async fn build_lanes(job: &Job, probe: &Probe, headers: &HeaderMap, ranged: bool) -> Vec<Lane> {
    let mut mirrors: Vec<Url> = Vec::new();
    if ranged && !job.options.mirrors.is_empty() {
        let checker = job.links.links.first().map_or_else(|| job.client.clone(), |l| l.client.clone());
        let mut seen = vec![job.url.clone(), probe.final_url.clone()];
        let candidates: Vec<Url> = job
            .options
            .mirrors
            .iter()
            .filter_map(|m| Url::parse(m.trim()).ok())
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
            .filter(|u| {
                let fresh = !seen.contains(u);
                seen.push(u.clone());
                fresh
            })
            .take(8)
            .collect();
        let checks = candidates.iter().map(|url| {
            let (client, url) = (checker.clone(), url.clone());
            let headers =
                if same_origin(&job.url, &url) { headers.clone() } else { without_credentials(headers.clone()) };
            async move {
                let result = tokio::time::timeout(MIRROR_CHECK, probe::probe(&client, &url, &headers)).await;
                (url, result)
            }
        });
        for (_url, result) in futures_util::future::join_all(checks).await {
            if let Ok(Ok(p)) = result
                && p.ranges
                && crate::net::same_content(probe.total, p.total)
            {
                mirrors.push(p.final_url);
            }
        }
    }
    let sources: Vec<Option<Url>> = std::iter::once(None).chain(mirrors.into_iter().map(Some)).collect();
    let mut lanes = Vec::new();
    for source in &sources {
        for link in &job.links.links {
            let host = source.as_ref().unwrap_or(&probe.final_url).host_str().unwrap_or("").to_lowercase();
            lanes.push(Lane {
                stat: Arc::new(LaneStat::new(&link.label, &host, source.is_some())),
                link: Arc::clone(link),
                mirror_url: source.clone(),
            });
        }
    }
    lanes
}

/// What the auto-tuner decided.
#[derive(Debug, PartialEq, Eq)]
enum Tune {
    /// Start this many more connections.
    Grow(usize),
    /// More connections no longer help.
    Settled,
    Wait,
}

/// Grows the connection count while the speed keeps improving. Servers and
/// links differ: some give a single stream the whole line, others hand out a
/// slice per connection. Measuring beats guessing.
struct Tuner {
    last_moved: u64,
    last_at: Instant,
    /// Speed seen before the latest step.
    baseline: Option<f64>,
    flat: u32,
}

impl Default for Tuner {
    fn default() -> Self {
        Self { last_moved: 0, last_at: Instant::now(), baseline: None, flat: 0 }
    }
}

impl Tuner {
    fn step(&mut self, moved: u64, spawned: usize, max: usize) -> Tune {
        let now = Instant::now();
        let secs = now.duration_since(self.last_at).as_secs_f64().max(0.05);
        let speed = moved.saturating_sub(self.last_moved) as f64 / secs;
        self.last_moved = moved;
        self.last_at = now;
        if speed < 1.0 {
            return Tune::Wait; // nothing flowing yet
        }
        let better = self.baseline.is_none_or(|b| speed >= b * TUNE_GAIN);
        if better {
            self.baseline = Some(speed);
            self.flat = 0;
            return Tune::Grow((spawned / 2).max(2).min(max - spawned));
        }
        self.flat += 1;
        if self.flat >= 2 { Tune::Settled } else { Tune::Wait }
    }
}

#[cfg(test)]
mod tune_tests {
    use super::*;

    /// One reading, as if a second had passed since the last one.
    fn second(t: &mut Tuner, moved: u64, spawned: usize, max: usize) -> Tune {
        t.last_at = Instant::now() - Duration::from_secs(1);
        t.step(moved, spawned, max)
    }

    #[test]
    fn grows_until_the_speed_stops_improving() {
        let mut t = Tuner::default();
        assert_eq!(second(&mut t, 0, 4, 16), Tune::Wait);
        assert_eq!(second(&mut t, 1_000_000, 4, 16), Tune::Grow(2)); // 1 MB/s, first reading
        assert_eq!(second(&mut t, 3_000_000, 6, 16), Tune::Grow(3)); // 2 MB/s: better
        assert_eq!(second(&mut t, 5_050_000, 9, 16), Tune::Wait); // 2.05 MB/s: flat once
        assert_eq!(second(&mut t, 7_100_000, 9, 16), Tune::Settled);
    }

    #[test]
    fn never_exceeds_the_maximum() {
        let mut t = Tuner::default();
        assert_eq!(second(&mut t, 1_000_000, 15, 16), Tune::Grow(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_stay_on_their_origin() {
        let a = Url::parse("https://files.example.com/a.zip").unwrap();
        assert!(same_origin(&a, &Url::parse("https://files.example.com/b/c.zip?x=1").unwrap()));
        assert!(!same_origin(&a, &Url::parse("https://cdn.example.net/a.zip").unwrap()));
        assert!(!same_origin(&a, &Url::parse("http://files.example.com/a.zip").unwrap()));
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("session=1"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        h.insert(header::REFERER, HeaderValue::from_static("https://files.example.com/"));
        let h = without_credentials(h);
        assert!(h.get(header::COOKIE).is_none() && h.get(header::AUTHORIZATION).is_none());
        assert!(h.get(header::REFERER).is_some(), "the referrer is not a credential");
    }
}
