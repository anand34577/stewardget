//! Signed download links that expire mid-download (GitHub release assets, S3):
//! the stable link redirects to a signed one, which stops working after a while.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use stewardget_core::{Config, Engine, EngineOptions, NewDownload, Status};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Requests a signed link answers before it "expires".
const USES_PER_LINK: usize = 3;

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 131 + i / 7) % 251) as u8).collect()
}

/// `/release/file.bin` redirects to `/signed/<id>.bin?token=N`, each token
/// valid for [`USES_PER_LINK`] requests. Returns the base URL and how many
/// redirects were handed out.
async fn serve(data: Arc<Vec<u8>>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let tokens = Arc::new(AtomicUsize::new(0));
    let uses: Arc<std::sync::Mutex<std::collections::HashMap<usize, usize>>> = Arc::default();
    let handed_out = Arc::clone(&tokens);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (data, tokens, uses) = (Arc::clone(&data), Arc::clone(&tokens), Arc::clone(&uses));
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let req = String::from_utf8_lossy(&buf).into_owned();
                let target = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
                if target == "/release/file.bin" {
                    let token = tokens.fetch_add(1, Ordering::SeqCst) + 1;
                    let head = format!(
                        "HTTP/1.1 302 Found\r\nlocation: /signed/5a26b454-bf1a.bin?token={token}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    return;
                }
                let token: usize = target.split("token=").nth(1).and_then(|t| t.parse().ok()).unwrap_or(0);
                let used = {
                    let mut uses = uses.lock().unwrap();
                    let n = uses.entry(token).or_default();
                    *n += 1;
                    *n
                };
                if token == 0 || used > USES_PER_LINK {
                    let _ = sock
                        .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                        .await;
                    return;
                }
                let lower = req.to_ascii_lowercase();
                let range = lower.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| {
                    let (a, b) = r.trim().split_once('-')?;
                    let a: usize = a.parse().ok()?;
                    let b: usize =
                        if b.is_empty() { data.len() - 1 } else { b.parse::<usize>().ok()?.min(data.len() - 1) };
                    Some((a, b))
                });
                let (a, b) = range.unwrap_or((0, data.len() - 1));
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: bytes {a}-{b}/{}\r\naccept-ranges: bytes\r\netag: \"same\"\r\ncontent-disposition: attachment; filename=\"tool-setup.exe\"\r\nconnection: close\r\n\r\n",
                    b - a + 1,
                    data.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                // Slowly, so connections keep asking for new ranges.
                for chunk in data[a..=b].chunks(32 * 1024) {
                    if sock.write_all(chunk).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
        }
    });
    (format!("http://{addr}"), handed_out)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_signed_link_is_replaced_and_the_download_finishes() {
    let data = Arc::new(payload(3 * 1024 * 1024));
    let (base, redirects) = serve(Arc::clone(&data)).await;
    let dir = std::env::temp_dir().join(format!("stewardget-relink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config {
        download_dir: dir.clone(),
        connections: 6,
        min_split_size: 128 * 1024,
        max_retries: 3,
        auto_retry: 0,
        read_timeout_secs: 5,
        ..Config::default()
    };
    let engine = Engine::start(EngineOptions::in_memory().with_config(config)).unwrap();
    let id = engine.add(NewDownload::new(format!("{base}/release/file.bin"))).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let view = loop {
        let d = engine.download(id).unwrap();
        if d.status.is_finished() || std::time::Instant::now() > deadline {
            break d;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(view.status, Status::Completed, "{:?}", view.error);
    assert_eq!(view.file_name, "tool-setup.exe", "the server's name, not the signed link's id");
    assert!(std::fs::read(&view.path).unwrap() == *data, "the file is intact");
    assert!(redirects.load(Ordering::SeqCst) > 1, "fresh links were fetched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_used_up_link_falls_back_to_where_the_browser_landed() {
    let data = Arc::new(payload(200 * 1024));
    let (base, _) = serve(Arc::clone(&data)).await;
    let dir = std::env::temp_dir().join(format!("stewardget-fallback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config { download_dir: dir.clone(), connections: 1, auto_retry: 0, ..Config::default() };
    let engine = Engine::start(EngineOptions::in_memory().with_config(config)).unwrap();
    // The clicked link answers 403 now (it was a one-time link); the address
    // the browser was sent to still works.
    let (id, _) = engine
        .capture(stewardget_core::Capture {
            url: format!("{base}/download?once=1"),
            fallback_url: Some(format!("{base}/signed/5a26b454-bf1a.bin?token=77")),
            ..Default::default()
        })
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let view = loop {
        let d = engine.download(id).unwrap();
        if d.status.is_finished() || std::time::Instant::now() > deadline {
            break d;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(view.status, Status::Completed, "{:?}", view.error);
    assert_eq!(view.file_name, "tool-setup.exe");
    assert!(std::fs::read(&view.path).unwrap() == *data);
    let _ = std::fs::remove_dir_all(&dir);
}
