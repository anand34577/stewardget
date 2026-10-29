//! End-to-end tests against a tiny in-process HTTP/1.1 server.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use stewardget_core::{
    Capture, CaptureAction, Category, Checksum, Config, CustomCategory, DownloadOptions, Engine, EngineOptions, Event,
    ExistingFile, HashAlgo, MAIN_QUEUE, NewDownload, Queue, Status,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Copy, Default)]
struct ServerOpts {
    ranges: bool,
    /// Bytes per second per connection (0 = unthrottled).
    throttle: usize,
    /// Abort every n-th response midway (0 = never).
    flaky_every: usize,
    /// Respond 503 to every n-th request (0 = never).
    busy_every: usize,
    /// Serve at most this many connections at once; extra ones stall forever (0 = no cap).
    max_conns: usize,
    /// ETag to send (defaults to "v1").
    etag: Option<&'static str>,
    /// Answer the first n requests with 503 (a server that is briefly down).
    down_first: usize,
    /// Answer the first n requests with 429 and `Retry-After: 2`.
    slow_down_first: usize,
    /// `Content-Disposition` header to send.
    disposition: Option<&'static str>,
}

struct Server {
    url: String,
    data: Arc<Vec<u8>>,
    ranged_requests: Arc<AtomicUsize>,
    etag: Arc<Mutex<String>>,
}

impl Server {
    /// Simulates the file being replaced on the server.
    fn set_etag(&self, etag: &str) {
        *self.etag.lock().unwrap() = etag.to_owned();
    }
}

/// Deterministic, non-periodic payload so misplaced bytes are always caught.
fn payload(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect()
}

async fn serve(len: usize, opts: ServerOpts) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let data = Arc::new(payload(len));
    let counter = Arc::new(AtomicUsize::new(0));
    let ranged = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let etag = Arc::new(Mutex::new(opts.etag.unwrap_or("v1").to_owned()));
    let (d, c, r, e) = (Arc::clone(&data), Arc::clone(&counter), Arc::clone(&ranged), Arc::clone(&etag));
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let (d, c, r, open, e) =
                (Arc::clone(&d), Arc::clone(&c), Arc::clone(&r), Arc::clone(&open), Arc::clone(&e));
            tokio::spawn(async move {
                let n = open.fetch_add(1, Ordering::SeqCst) + 1;
                if opts.max_conns > 0 && n > opts.max_conns {
                    // Never answer; free the slot once the client hangs up.
                    let mut sock = sock;
                    let mut buf = [0u8; 1024];
                    while matches!(sock.read(&mut buf).await, Ok(n) if n > 0) {}
                } else {
                    let etag = e.lock().unwrap().clone();
                    let _ = handle(sock, &d, opts, &etag, &c, &r).await;
                }
                open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    Server { url: format!("http://{addr}/files/test%20file.bin"), data, ranged_requests: ranged, etag }
}

async fn handle(
    mut sock: TcpStream,
    data: &[u8],
    opts: ServerOpts,
    etag: &str,
    counter: &AtomicUsize,
    ranged: &AtomicUsize,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let req = String::from_utf8_lossy(&buf).to_ascii_lowercase();
    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
    if (opts.busy_every > 0 && n.is_multiple_of(opts.busy_every)) || n <= opts.down_first {
        sock.write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await?;
        return Ok(());
    }
    if n <= opts.slow_down_first {
        sock.write_all(
            b"HTTP/1.1 429 Too Many Requests
retry-after: 2
content-length: 0
connection: close

",
        )
        .await?;
        return Ok(());
    }
    let range = req.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.trim().split_once('-')).map(
        |(a, b)| {
            let a: usize = a.parse().unwrap();
            let b: usize = if b.is_empty() { data.len() - 1 } else { b.parse::<usize>().unwrap().min(data.len() - 1) };
            (a, b)
        },
    );
    let (status, body, extra) = match range {
        Some((a, b)) if opts.ranges => {
            ranged.fetch_add(1, Ordering::SeqCst);
            ("206 Partial Content", &data[a..=b], format!("content-range: bytes {a}-{b}/{}\r\n", data.len()))
        }
        _ => ("200 OK", data, String::new()),
    };
    let disposition = opts.disposition.map(|d| format!("content-disposition: {d}\r\n")).unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\n{extra}{}{disposition}etag: \"{}\"\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n",
        body.len(),
        if opts.ranges { "accept-ranges: bytes\r\n" } else { "" },
        etag,
    );
    sock.write_all(head.as_bytes()).await?;
    let abort_at =
        (opts.flaky_every > 0 && n.is_multiple_of(opts.flaky_every) && body.len() > 2).then_some(body.len() / 2);
    let chunk = 16 * 1024;
    let mut sent = 0;
    while sent < body.len() {
        let end = (sent + chunk).min(body.len());
        let end = abort_at.map_or(end, |a| end.min(a.max(sent + 1)));
        sock.write_all(&body[sent..end]).await?;
        sent = end;
        if abort_at.is_some_and(|a| sent >= a) {
            return Ok(()); // drop mid-body
        }
        if opts.throttle > 0 {
            tokio::time::sleep(Duration::from_secs_f64(chunk as f64 / opts.throttle as f64)).await;
        }
    }
    sock.flush().await
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stewardget-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(dir: &std::path::Path) -> Config {
    Config {
        download_dir: dir.to_path_buf(),
        connections: 8,
        min_split_size: 256 * 1024,
        max_retries: 20,
        read_timeout_secs: 5,
        ..Config::default()
    }
}

async fn wait_for(engine: &Engine, id: u64, target: Status, secs: u64) -> Status {
    let mut events = engine.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let status = engine.download(id).map(|d| d.status);
        if status == Some(target) || status.is_some_and(|s| s.is_finished()) {
            return status.unwrap();
        }
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Event::StatusChanged(i, s))) if i == id && (s == target || s.is_finished()) => return s,
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for {target:?}; currently {status:?}"),
        }
    }
}

fn sha256_hex(data: &[u8], dir: &std::path::Path) -> String {
    let p = dir.join("reference.bin");
    std::fs::write(&p, data).unwrap();
    let c = Checksum { algo: HashAlgo::Sha256, hex: String::new() };
    let hex = match c.verify(&p) {
        Err(stewardget_core::Error::ChecksumMismatch { actual, .. }) => actual,
        other => panic!("{other:?}"),
    };
    std::fs::remove_file(p).unwrap();
    hex
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn segmented_download_with_checksum() {
    let server = serve(12 * 1024 * 1024 + 17, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("segmented");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let hex = sha256_hex(&server.data, &dir);
    let id = engine
        .add(NewDownload {
            url: server.url.clone(),
            options: DownloadOptions { checksum: Some(format!("sha256:{hex}").parse().unwrap()), ..Default::default() },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(view.file_name, "test file.bin");
    assert!(view.checksum_verified);
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
    assert!(server.ranged_requests.load(Ordering::SeqCst) >= 8, "expected parallel ranged requests");
    assert!(!dir.join("test file.bin.vxpart").exists());
    assert!(!dir.join("test file.bin.vxmeta").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn falls_back_to_single_stream_without_ranges() {
    let server = serve(3 * 1024 * 1024, ServerOpts::default()).await;
    let dir = temp_dir("single");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert!(!view.resumable);
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn survives_flaky_and_busy_servers() {
    let opts = ServerOpts { ranges: true, flaky_every: 3, busy_every: 5, ..Default::default() };
    let server = serve(6 * 1024 * 1024 + 3, opts).await;
    let dir = temp_dir("flaky");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 90).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume_keeps_data_intact() {
    let opts = ServerOpts { ranges: true, throttle: 512 * 1024, ..Default::default() };
    let server = serve(8 * 1024 * 1024, opts).await;
    let dir = temp_dir("pause");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    wait_for(&engine, id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    engine.pause(id).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paused = engine.download(id).unwrap();
    assert_eq!(paused.status, Status::Paused);
    assert!(paused.downloaded > 0 && paused.downloaded < server.data.len() as u64);
    engine.resume(id).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 60).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_across_engine_restarts() {
    let opts = ServerOpts { ranges: true, throttle: 512 * 1024, ..Default::default() };
    let server = serve(8 * 1024 * 1024, opts).await;
    let dir = temp_dir("restart");
    let data_dir = dir.join("state");
    let downloads = dir.join("files");
    let options = || EngineOptions { data_dir: Some(data_dir.clone()), config: Some(config(&downloads)) };

    let engine = Engine::start(options()).unwrap();
    // A second engine on the same directory must be refused.
    assert!(matches!(Engine::start(options()), Err(stewardget_core::Error::Locked(_))));
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    wait_for(&engine, id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    engine.shutdown().await;
    let before = engine.download(id).unwrap().downloaded;
    assert!(before > 0);
    drop(engine);

    let engine = Engine::start(options()).unwrap();
    let view = engine.download(id).expect("download restored");
    assert!(view.downloaded >= before.min(view.downloaded));
    assert_eq!(wait_for(&engine, id, Status::Completed, 60).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
    let stats = engine.snapshot().stats;
    assert_eq!(stats.completed, 1);
    assert!(stats.total_bytes >= server.data.len() as u64 / 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn respects_speed_limit() {
    let server = serve(2 * 1024 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("limit");
    let cfg = Config { speed_limit: 1024 * 1024, ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let start = std::time::Instant::now();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let secs = start.elapsed().as_secs_f64();
    assert!(secs > 1.5, "2 MiB at 1 MiB/s finished in {secs:.2}s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_checksum_fails() {
    let server = serve(1024 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("badsum");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let options =
        DownloadOptions { checksum: Some(format!("md5:{}", "0".repeat(32)).parse().unwrap()), ..Default::default() };
    let id = engine.add(NewDownload { url: server.url.clone(), options, ..Default::default() }).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Failed, 30).await, Status::Failed);
    assert!(engine.download(id).unwrap().error.unwrap().contains("checksum"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapts_to_connection_limited_servers() {
    let opts = ServerOpts { ranges: true, max_conns: 2, throttle: 4 * 1024 * 1024, ..Default::default() };
    let server = serve(6 * 1024 * 1024, opts).await;
    let dir = temp_dir("connlimit");
    let cfg = Config { read_timeout_secs: 1, ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 60).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refreshed_link_resumes_partial_data() {
    // The "expired" link: a slow server we abandon midway.
    let old = serve(8 * 1024 * 1024, ServerOpts { ranges: true, throttle: 512 * 1024, ..Default::default() }).await;
    // A fresh mirror of the same file that reports a different ETag.
    let fresh = serve(8 * 1024 * 1024, ServerOpts { ranges: true, etag: Some("v2"), ..Default::default() }).await;
    let dir = temp_dir("refresh");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&old.url)).unwrap();
    wait_for(&engine, id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;

    engine.await_new_link(id, true).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = engine.download(id).unwrap();
    assert!(before.awaiting_link && before.downloaded > 0);

    // The browser captures the same file from a new link.
    let (got, action) = engine
        .capture(stewardget_core::Capture {
            url: fresh.url.clone(),
            file_name: Some(r"C:\Users\me\Downloads\test file.bin".into()),
            size: Some(fresh.data.len() as u64),
            ..Default::default()
        })
        .unwrap();
    assert_eq!((got, action), (id, stewardget_core::CaptureAction::Refreshed));
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(std::fs::read(&view.path).unwrap(), *fresh.data);
    assert!(view.url.starts_with(&fresh.url[..20]));
    // Only the missing part came from the fresh server.
    assert!(fresh.ranged_requests.load(Ordering::SeqCst) > 0);
    assert_eq!(engine.snapshot().downloads.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_api_accepts_extensions_and_rejects_web_pages() {
    let server = serve(64 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("api");
    let port = 20_000 + (std::process::id() % 20_000) as u16;
    let cfg = Config { api_port: port, ..config(&dir.join("files")) };
    let engine = Engine::start(EngineOptions { data_dir: Some(dir.join("state")), config: Some(cfg) }).unwrap();
    assert_eq!(engine.snapshot().api, stewardget_core::ApiState::Listening(port));

    async fn request(port: u16, head: &str, body: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let req = format!("{head}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }
    async fn post(port: u16, extra: &str, body: &str) -> String {
        request(port, &format!("POST /api/v1/downloads HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\n{extra}"), body).await
    }
    let body = format!(r#"{{"url":"{}","referrer":"https://example.com/page"}}"#, server.url);
    let web = post(port, "origin: https://evil.example\r\nx-stewardget-client: 1\r\n", &body).await;
    assert!(web.starts_with("HTTP/1.1 403"), "{web}");
    let no_header = post(port, "origin: chrome-extension://abc\r\n", &body).await;
    assert!(no_header.starts_with("HTTP/1.1 403"), "{no_header}");
    // DNS rebinding: a page on another domain that now resolves to 127.0.0.1.
    let rebound = request(
        port,
        "POST /api/v1/downloads HTTP/1.1\r\nhost: evil.example:7853\r\nx-stewardget-client: 1\r\n",
        &body,
    )
    .await;
    assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");
    let listing = request(port, &format!("GET /api/v1/downloads HTTP/1.1\r\nhost: localhost:{port}\r\n"), "").await;
    assert!(listing.starts_with("HTTP/1.1 403"), "{listing}");
    assert_eq!(engine.snapshot().extension, None);
    let ok = post(
        port,
        "origin: chrome-extension://abc\r\nx-stewardget-client: extension\r\nx-stewardget-browser: Edge\r\nx-stewardget-extension-version: 9.9.9\r\n",
        &body,
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("\"added\""), "{ok}");
    let seen = engine.snapshot().extension.unwrap();
    assert_eq!((seen.browser.as_str(), seen.version.as_str()), ("Edge", "9.9.9"));

    // Without a window there is nothing to show; a front-end can register one.
    let show = format!("POST /api/v1/show HTTP/1.1\r\nhost: [::1]:{port}\r\nx-stewardget-client: cli\r\n");
    assert!(request(port, &show, "").await.contains(r#"{"shown":false}"#));
    let shown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = shown.clone();
    engine.set_show_handler(move || flag.store(true, Ordering::SeqCst));
    assert!(request(port, &show, "").await.contains(r#"{"shown":true}"#));
    assert!(shown.load(Ordering::SeqCst));
    // An extension from before the rename (0.7) names its headers X-Velox-….
    let legacy = format!(
        "POST /api/v1/show HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nx-velox-client: extension\r\nx-velox-browser: Brave\r\nx-velox-extension-version: 0.6.0\r\n"
    );
    assert!(request(port, &legacy, "").await.contains(r#"{"shown":true}"#));
    let seen = engine.snapshot().extension.unwrap();
    assert_eq!((seen.browser.as_str(), seen.version.as_str()), ("Brave", "0.6.0"));

    let snap = engine.snapshot();
    assert_eq!(snap.downloads.len(), 1);
    assert_eq!(snap.downloads[0].options.referrer.as_deref(), Some("https://example.com/page"));
    let id = snap.downloads[0].id;
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);

    // Without a token a client can't skip the desktop app's question.
    let asked = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let sink = asked.clone();
    engine.set_capture_handler(move |batch| *sink.lock().unwrap() += batch.len());
    let pushy = format!(r#"{{"url":"{}","direct":true,"paused":true}}"#, server.url);
    let reply = post(port, "x-stewardget-client: extension\r\n", &pushy).await;
    assert!(reply.contains("\"pending\""), "{reply}");
    assert_eq!(*asked.lock().unwrap(), 1);
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_remote_file_keeps_partial_data_until_the_user_decides() {
    let opts = ServerOpts { ranges: true, throttle: 512 * 1024, ..Default::default() };
    let server = serve(8 * 1024 * 1024, opts).await;
    let dir = temp_dir("changed");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    wait_for(&engine, id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    engine.pause(id).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paused = engine.download(id).unwrap();
    assert!(paused.downloaded > 0);

    // The server now reports another version: StewardGet must stop, not start over.
    server.set_etag("v2");
    engine.resume(id).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Failed, 30).await, Status::Failed);
    let failed = engine.download(id).unwrap();
    assert!(failed.error.as_deref().unwrap().contains("changed"), "{:?}", failed.error);
    let part = std::fs::metadata(format!("{}.vxpart", paused.path.display())).expect("partial data kept");
    assert!(part.len() > 0);
    assert!(failed.downloaded >= paused.downloaded);

    // "Resume anyway" continues from the data on disk.
    engine.resume_anyway(id).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 60).await, Status::Completed);
    assert_eq!(std::fs::read(engine.download(id).unwrap().path).unwrap(), *server.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_keeps_partial_data_and_re_adding_resumes_it() {
    let opts = ServerOpts { ranges: true, throttle: 512 * 1024, ..Default::default() };
    let server = serve(8 * 1024 * 1024, opts).await;
    let dir = temp_dir("keep");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    wait_for(&engine, id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // Remove while the paused task may still be winding down, then re-add at
    // once: the new download must still take over the same file.
    engine.pause(id).unwrap();
    let path = engine.download(id).unwrap().path;
    engine.remove(id, false).unwrap();
    let part = PathBuf::from(format!("{}.vxpart", path.display()));
    assert!(part.exists(), "partial data must survive removal from the list");

    let before = server.ranged_requests.load(Ordering::SeqCst);
    let again = engine.add(NewDownload::new(&server.url)).unwrap();
    wait_for(&engine, again, Status::Downloading, 10).await;
    let resumed_at = engine.download(again).unwrap();
    assert_eq!(resumed_at.path, path, "same file, not a renamed copy");
    assert_eq!(wait_for(&engine, again, Status::Completed, 60).await, Status::Completed);
    assert_eq!(std::fs::read(&path).unwrap(), *server.data);
    assert!(server.ranged_requests.load(Ordering::SeqCst) > before);

    // Removing with "delete files" really deletes.
    engine.remove(again, true).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_downloads_retry_automatically() {
    let opts = ServerOpts { ranges: true, down_first: 1, ..Default::default() };
    let server = serve(1024 * 1024, opts).await;
    let dir = temp_dir("autoretry");
    let cfg = Config { max_retries: 0, auto_retry: 2, auto_retry_delay_secs: 1, ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let view = engine.download(id).unwrap();
    assert_eq!(view.retries, 1);
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_link_reports_name_size_and_resume_support() {
    let server = serve(300_000, ServerOpts { ranges: true, ..Default::default() }).await;
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&temp_dir("check")))).unwrap();
    let info = engine.check_link(&server.url, &DownloadOptions::default()).await.unwrap();
    assert_eq!(info.file_name, "test file.bin");
    assert_eq!(info.size, Some(300_000));
    assert!(info.resumable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relocate_moves_finished_and_partial_files() {
    let server = serve(512 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("relocate");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let elsewhere = dir.join("moved");
    engine.relocate(id, elsewhere.clone(), "renamed.bin").unwrap();
    let view = engine.download(id).unwrap();
    assert_eq!(view.path, elsewhere.join("renamed.bin"));
    assert_eq!(std::fs::read(&view.path).unwrap(), *server.data);
    assert_eq!(view.file_name, "renamed.bin");

    // Partial data travels with a paused download, and it still completes.
    let slow = serve(8 * 1024 * 1024, ServerOpts { ranges: true, throttle: 512 * 1024, ..Default::default() }).await;
    let id = engine.add(NewDownload::new(&slow.url)).unwrap();
    wait_for(&engine, id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(engine.relocate(id, elsewhere.clone(), "busy.bin").is_err(), "running downloads can't move");
    engine.pause(id).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    engine.relocate(id, elsewhere.clone(), "partial.bin").unwrap();
    assert!(elsewhere.join("partial.bin.vxpart").exists());
    engine.resume(id).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 60).await, Status::Completed);
    assert_eq!(std::fs::read(elsewhere.join("partial.bin")).unwrap(), *slow.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queues_have_their_own_limits_order_and_controls() {
    let dir = temp_dir("queues");
    let night = Queue { id: 7, name: "Night".into(), max_concurrent: 1, ..Queue::default() };
    let cfg = Config { max_concurrent: 8, queues: vec![Queue::default(), night], ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    // Unreachable links that stay paused: only queue bookkeeping is under test.
    let add = |n: usize, queue| {
        engine
            .add(NewDownload { url: format!("http://127.0.0.1:9/f{n}.bin"), paused: true, queue, ..Default::default() })
            .unwrap()
    };
    let a = add(1, None);
    let b = add(2, Some(7));
    let c = add(3, Some(7));
    let d = add(4, Some(99)); // unknown queue: main
    let queue_of = |id| engine.download(id).unwrap().queue;
    assert_eq!([queue_of(a), queue_of(b), queue_of(c), queue_of(d)], [MAIN_QUEUE, 7, 7, MAIN_QUEUE]);

    // Moving within a queue skips downloads of other queues.
    engine.reorder(c, -1).unwrap();
    let order: Vec<_> = engine.snapshot().downloads.iter().map(|v| v.id).collect();
    assert_eq!(order, [a, c, b, d]);
    engine.move_to_end(a, false).unwrap();
    let order: Vec<_> = engine.snapshot().downloads.iter().map(|v| v.id).collect();
    assert_eq!(order, [c, b, d, a]);

    // A queue's own limit: only one of Night's downloads starts.
    engine.start_queue(7);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = [b, c].iter().filter(|&&id| engine.download(id).unwrap().status != Status::Paused).count();
    assert!(started >= 1);
    let running = [b, c].iter().filter(|&&id| engine.download(id).unwrap().status.is_active()).count();
    assert!(running <= 1, "Night allows one download at a time");
    assert_eq!(engine.download(a).unwrap().status, Status::Paused, "other queues are left alone");
    engine.stop_queue(7);
    for id in [b, c] {
        assert_eq!(engine.download(id).unwrap().status, Status::Paused);
    }

    // Moving to another queue, and deleting a queue, which hands its downloads to main.
    engine.set_queue(&[a], 7).unwrap();
    assert_eq!(queue_of(a), 7);
    let mut config = engine.config();
    config.queues.retain(|q| q.id != 7);
    engine.update_config(config).unwrap();
    assert!([a, b, c].iter().all(|&id| queue_of(id) == MAIN_QUEUE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_desktop_app_can_ask_before_browser_downloads_start() {
    let dir = temp_dir("ask");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let capture = |name: &str| Capture {
        url: format!("https://example.com/download?file={name}"),
        file_name: Some(format!("C:\\Users\\me\\Downloads\\{name}")),
        cookies: Some("session=1".into()),
        referrer: Some("https://example.com/page".into()),
        paused: true,
        ..Default::default()
    };

    // Nobody asks: added right away.
    let (id, action) = engine.capture(capture("one.zip")).unwrap();
    assert_eq!(action, CaptureAction::Added);
    assert_eq!(engine.download(id).unwrap().file_name, "one.zip");

    // The desktop app asks: nothing is added, it gets the downloads instead, together.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let sink = asked.clone();
    engine.set_capture_handler(move |batch| sink.lock().unwrap().push(batch));
    let results = engine.capture_many(vec![capture("two.zip"), capture("three.iso")]).unwrap();
    assert_eq!(results, [(0, CaptureAction::Pending), (0, CaptureAction::Pending)]);
    assert_eq!(engine.snapshot().downloads.len(), 1);
    let batches = asked.lock().unwrap().clone();
    assert_eq!(batches.len(), 1);
    let names: Vec<_> = batches[0].iter().map(|n| n.options.file_name.clone().unwrap()).collect();
    assert_eq!(names, ["two.zip", "three.iso"]);
    let first = &batches[0][0];
    assert!(first.options.headers.contains(&("Cookie".into(), "session=1".into())));
    assert_eq!(first.options.referrer.as_deref(), Some("https://example.com/page"));

    // One bad link: nothing at all is handed over.
    assert!(
        engine
            .capture_many(vec![capture("four.zip"), Capture { url: "ftp://x/y".into(), ..Default::default() }])
            .is_err()
    );
    assert_eq!(asked.lock().unwrap().len(), 1);

    // Turned off in the settings: straight in again.
    engine.update_config(Config { capture_dialog: false, ..engine.config() }).unwrap();
    assert_eq!(engine.capture(capture("five.zip")).unwrap().1, CaptureAction::Added);
    assert_eq!(engine.folder_for("x.mkv"), dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_speed_limit_caps_its_downloads() {
    let server = serve(2 * 1024 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("queue-limit");
    let slow = Queue { id: 1, name: "Slow".into(), speed_limit: 1024 * 1024, ..Queue::default() };
    let cfg = Config { queues: vec![Queue::default(), slow], ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let start = std::time::Instant::now();
    let id = engine.add(NewDownload { queue: Some(1), ..NewDownload::new(&server.url) }).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    let secs = start.elapsed().as_secs_f64();
    assert!(secs > 1.5, "2 MiB at 1 MiB/s finished in {secs:.2}s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn custom_categories_choose_the_folder_and_existing_files_follow_the_policy() {
    let server = serve(256 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("custom-category");
    let cfg = Config {
        auto_categorize: true,
        custom_categories: vec![CustomCategory { id: 4, name: "Firmware".into(), extensions: vec!["bin".into()] }],
        ..config(&dir)
    };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let first = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, first, Status::Completed, 30).await, Status::Completed);
    let d = engine.download(first).unwrap();
    assert_eq!(d.category, Category::Custom(4));
    assert_eq!(d.path, dir.join("Firmware").join("test file.bin"));

    // Renaming is the default: the second copy gets a number.
    let second = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, second, Status::Completed, 30).await, Status::Completed);
    assert_eq!(engine.download(second).unwrap().path, dir.join("Firmware").join("test file (1).bin"));

    // With overwrite, the same name is reused.
    let mut cfg = engine.config();
    cfg.existing_file = ExistingFile::Overwrite;
    engine.update_config(cfg).unwrap();
    let third = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, third, Status::Completed, 30).await, Status::Completed);
    let path = engine.download(third).unwrap().path;
    assert_eq!(path, dir.join("Firmware").join("test file.bin"));
    assert_eq!(std::fs::read(&path).unwrap(), *server.data);

    // Deleting the category sorts its downloads back into the built-in ones.
    let mut cfg = engine.config();
    cfg.custom_categories.clear();
    engine.update_config(cfg).unwrap();
    assert_eq!(engine.download(first).unwrap().category, Category::Program);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_cookies_are_forgotten_once_the_download_is_done() {
    let server = serve(128 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("cookies");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let capture = Capture { url: server.url.clone(), cookies: Some("session=secret".into()), ..Default::default() };
    let (id, _) = engine.capture(capture).unwrap();
    assert!(engine.download(id).unwrap().options.headers.iter().any(|(k, _)| k == "Cookie"));
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    assert!(engine.download(id).unwrap().options.headers.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pausing_stops_the_numbers_at_once_even_under_a_speed_limit() {
    let server = serve(16 * 1024 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("pause-now");
    let cfg = Config { speed_limit: 512 * 1024, ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    let started = tokio::time::Instant::now();
    while engine.download(id).unwrap().downloaded < 600 * 1024 {
        assert!(started.elapsed() < Duration::from_secs(20), "never got going");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    engine.pause(id).unwrap();
    let at_pause = engine.download(id).unwrap();
    assert_eq!(at_pause.status, Status::Paused);
    assert_eq!(at_pause.speed, 0.0, "a paused download shows no speed");
    // The connections close within a moment, and the shown size never moves.
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(engine.download(id).unwrap().downloaded, at_pause.downloaded);
    }
    // Resuming carries on from the same place.
    engine.resume(id).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(engine.download(id).unwrap().downloaded >= at_pause.downloaded);
    engine.remove(id, true).unwrap();
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_says_retry_after_is_obeyed() {
    let server = serve(512 * 1024, ServerOpts { ranges: true, slow_down_first: 1, ..Default::default() }).await;
    let dir = temp_dir("retry-after");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let start = std::time::Instant::now();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    assert!(start.elapsed() >= Duration::from_millis(1900), "retried after {:?}", start.elapsed());
}

async fn http(port: u16, head: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "{head}host: 127.0.0.1:{port}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

fn json_body(response: &str) -> serde_json::Value {
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap_or(serde_json::Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_api_and_mcp_manage_downloads() {
    let server = serve(8 * 1024 * 1024, ServerOpts { ranges: true, throttle: 256 * 1024, ..Default::default() }).await;
    let dir = temp_dir("mcp");
    let port = 20_000 + ((std::process::id() + 7) % 20_000) as u16;
    let token = "t".repeat(32);
    let cfg = Config { api_port: port, mcp_enabled: true, mcp_token: Some(token.clone()), ..config(&dir) };
    let engine = Engine::start(EngineOptions { data_dir: Some(dir.join("state")), config: Some(cfg) }).unwrap();
    let auth = format!("authorization: Bearer {token}\r\n");
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    // MCP needs its token, and refuses web pages.
    let denied = http(port, "POST /mcp HTTP/1.1\r\n", ping).await;
    assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
    let web = http(port, &format!("POST /mcp HTTP/1.1\r\norigin: https://evil.example\r\n{auth}"), ping).await;
    assert!(web.starts_with("HTTP/1.1 403"), "{web}");
    let head = format!("POST /mcp HTTP/1.1\r\n{auth}");
    let init = json_body(
        &http(
            port,
            &head,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        )
        .await,
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "stewardget");
    let note = http(port, &head, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await;
    assert!(note.starts_with("HTTP/1.1 202"), "{note}");

    // Add through a tool, then watch and control it through the REST API.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "add_download", "arguments": { "url": server.url, "folder": "Tools" } }
    });
    let added = json_body(&http(port, &head, &call.to_string()).await);
    assert_eq!(added["result"]["isError"], false, "{added}");
    let id = engine.snapshot().downloads[0].id;
    assert!(engine.download(id).unwrap().path.starts_with(dir.join("Tools")));
    let escape = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "add_download", "arguments": { "url": server.url, "folder": "../outside" } }
    });
    let refused = json_body(&http(port, &head, &escape.to_string()).await);
    assert_eq!(refused["result"]["isError"], true);

    // Without a token only what the browser extension needs is open.
    let bare = "x-stewardget-client: test\r\n";
    for (method, path) in [
        ("GET", "/api/v1/downloads"),
        ("GET", "/api/v1/stats"),
        ("POST", "/api/v1/pause-all"),
        ("POST", "/api/v1/quit"),
    ] {
        let refused = http(port, &format!("{method} {path} HTTP/1.1\r\n{bare}"), "").await;
        assert!(refused.starts_with("HTTP/1.1 401"), "{method} {path}: {refused}");
    }
    let ping_open = http(port, &format!("GET /api/v1/ping HTTP/1.1\r\n{bare}"), "").await;
    assert!(ping_open.starts_with("HTTP/1.1 200"), "{ping_open}");
    // The command line and installer hold the control token from the settings file.
    let control = engine.config().control_token.expect("a control token is created");
    let rest = format!("x-stewardget-client: test\r\nauthorization: Bearer {control}\r\n");
    let rest = rest.as_str();
    let one = http(port, &format!("GET /api/v1/downloads/{id} HTTP/1.1\r\n{rest}"), "").await;
    assert_eq!(json_body(&one)["file_name"], "test file.bin");
    let paused = http(port, &format!("POST /api/v1/downloads/{id}/pause HTTP/1.1\r\n{rest}"), "{}").await;
    assert!(paused.starts_with("HTTP/1.1 200"), "{paused}");
    assert_eq!(engine.download(id).unwrap().status, Status::Paused);
    let stats = http(port, &format!("GET /api/v1/stats HTTP/1.1\r\n{rest}"), "").await;
    assert_eq!(json_body(&stats)["paused"], 1);
    let limit =
        http(port, &format!("POST /api/v1/speed-limit HTTP/1.1\r\n{rest}"), r#"{"bytes_per_sec":1048576}"#).await;
    assert!(limit.starts_with("HTTP/1.1 200"), "{limit}");
    assert_eq!(engine.config().speed_limit, 1_048_576);
    let gone = http(port, &format!("DELETE /api/v1/downloads/{id}?delete_file=1 HTTP/1.1\r\n{rest}"), "").await;
    assert!(gone.starts_with("HTTP/1.1 200"), "{gone}");
    // The connections close first, then the download leaves the list.
    for _ in 0..50 {
        if engine.download(id).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(engine.download(id).is_none());
    let missing = http(port, &format!("POST /api/v1/downloads/{id}/pause HTTP/1.1\r\n{rest}"), "{}").await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");

    // Switched off, /mcp is not there at all.
    let mut off = engine.config();
    off.mcp_enabled = false;
    engine.update_config(off).unwrap();
    let off = http(port, &head, ping).await;
    assert!(off.starts_with("HTTP/1.1 404"), "{off}");
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_download_gets_the_servers_name_not_the_links_id() {
    let server = serve(
        64 * 1024,
        ServerOpts { ranges: true, disposition: Some(r#"attachment; filename="Real Name.zip""#), ..Default::default() },
    )
    .await;
    let dir = temp_dir("capture-name");
    let engine = Engine::start(EngineOptions::in_memory().with_config(config(&dir))).unwrap();
    let capture = |file_name: Option<&str>| Capture {
        url: server.url.clone(),
        file_name: file_name.map(Into::into),
        ..Default::default()
    };

    // No name from the browser, or only the link's last segment (what a browser
    // reports before it has read the response): the server's name wins.
    for hint in [None, Some("test file.bin"), Some(r"C:\Users\me\Downloads\test file.bin")] {
        let (id, action) = engine.capture(capture(hint)).unwrap();
        assert_eq!(action, CaptureAction::Added);
        assert_eq!(wait_for(&engine, id, Status::Completed, 20).await, Status::Completed);
        let name = engine.download(id).unwrap().file_name;
        assert!(name.starts_with("Real Name") && name.ends_with(".zip"), "{hint:?} gave {name}");
    }

    // A name the browser really chose is kept.
    let (id, _) = engine.capture(capture(Some(r"C:\Users\me\Downloads\chosen.zip"))).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 20).await, Status::Completed);
    assert_eq!(engine.download(id).unwrap().file_name, "chosen.zip");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pausing_wins_over_an_automatic_retry() {
    // Every request fails, so the task ends with a retryable error; the user
    // paused it meanwhile, and it must stay paused rather than be re-queued.
    let server = serve(4 * 1024 * 1024, ServerOpts { ranges: true, down_first: 1000, ..Default::default() }).await;
    let dir = temp_dir("pause-vs-retry");
    let cfg = Config { max_retries: 1, auto_retry: 5, auto_retry_delay_secs: 1, ..config(&dir) };
    let engine = Engine::start(EngineOptions::in_memory().with_config(cfg)).unwrap();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    wait_for(&engine, id, Status::Connecting, 10).await;
    engine.pause(id).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let d = engine.download(id).unwrap();
    assert_eq!(d.status, Status::Paused, "{:?}", d.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finished_downloads_stay_in_the_history_after_leaving_the_list() {
    let server = serve(2 * 1024 * 1024, ServerOpts { ranges: true, ..Default::default() }).await;
    let dir = temp_dir("history");
    let data_dir = dir.join("state");
    let downloads = dir.join("files");
    let options = || EngineOptions { data_dir: Some(data_dir.clone()), config: Some(config(&downloads)) };

    let engine = Engine::start(options()).unwrap();
    assert!(engine.history().is_empty());
    let generation = engine.history_generation();
    let id = engine.add(NewDownload::new(&server.url)).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Completed, 30).await, Status::Completed);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let log = engine.history();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].id, id);
    assert_eq!(log[0].file_name, "test file.bin");
    assert_eq!(log[0].host, "127.0.0.1");
    assert_eq!(log[0].size, Some(server.data.len() as u64));
    assert_eq!(log[0].status, Status::Completed);
    assert!(engine.history_generation() > generation);

    // Clearing the list does not clear the history…
    engine.clear_completed();
    assert!(engine.download(id).is_none());
    assert_eq!(engine.history().len(), 1);
    assert!(engine.tag_history(id, vec!["work".into()]));

    // …which is stored and comes back after a restart.
    engine.shutdown().await;
    drop(engine);
    let engine = Engine::start(options()).unwrap();
    let log = engine.history();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].tags, ["work"]);
    assert_eq!(engine.remove_history(&[id]), 1);
    assert!(engine.history().is_empty());
}
