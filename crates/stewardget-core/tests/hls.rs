//! HLS streams end to end: master playlist, best variant, AES-128 pieces,
//! pause and resume, against a tiny in-process server.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use stewardget_core::{Config, DownloadOptions, Engine, EngineOptions, NewDownload, Status};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: [u8; 16] = *b"0123456789abcdef";

fn piece(i: usize) -> Vec<u8> {
    (0..50_000).map(|j| ((i * 31 + j * 7) % 251) as u8).collect()
}

fn encrypt(data: &[u8], seq: u64) -> Vec<u8> {
    use cbc::cipher::block_padding::Pkcs7;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit};
    let iv = (seq as u128).to_be_bytes();
    cbc::Encryptor::<aes::Aes128>::new(&KEY.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(data)
}

/// Serves fixed paths; each answer is delayed by `delay` (to have time to pause).
async fn serve(routes: HashMap<String, (&'static str, Vec<u8>)>, delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let routes = Arc::clone(&routes);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let req = String::from_utf8_lossy(&buf);
                let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_owned();
                let ranged = req.to_ascii_lowercase().contains("range: bytes=0-0");
                tokio::time::sleep(delay).await;
                let head = match routes.get(&path) {
                    Some((kind, body)) if ranged => {
                        // The probe: one byte, like a real server.
                        let h = format!(
                            "HTTP/1.1 206 Partial Content\r\ncontent-type: {kind}\r\ncontent-range: bytes 0-0/{}\r\ncontent-length: 1\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(h.as_bytes()).await;
                        let _ = sock.write_all(&body[..1]).await;
                        return;
                    }
                    Some((kind, body)) => {
                        let h = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(h.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                        return;
                    }
                    None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned(),
                };
                let _ = sock.write_all(head.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

/// A master playlist with two qualities; the high one has 12 pieces, the
/// first four AES-128 encrypted. Returns the base URL and the expected file.
async fn stream(delay: Duration) -> (String, Vec<u8>) {
    let mut routes: HashMap<String, (&'static str, Vec<u8>)> = HashMap::new();
    let mpegurl = "application/vnd.apple.mpegurl";
    routes.insert(
        "/v/master.m3u8".into(),
        (mpegurl, b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=4000000\nhigh/index.m3u8\n".to_vec()),
    );
    routes.insert("/v/low.m3u8".into(), (mpegurl, b"#EXTM3U\n#EXTINF:4,\nwrong.ts\n#EXT-X-ENDLIST\n".to_vec()));
    let mut playlist = String::from("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:3\n#EXT-X-KEY:METHOD=AES-128,URI=\"/keys/k1\"\n");
    let mut expected = Vec::new();
    for i in 0..12 {
        if i == 4 {
            playlist.push_str("#EXT-X-KEY:METHOD=NONE\n");
        }
        playlist.push_str(&format!("#EXTINF:4.0,\np{i}.ts\n"));
        let plain = piece(i);
        let body = if i < 4 { encrypt(&plain, 3 + i as u64) } else { plain.clone() };
        routes.insert(format!("/v/high/p{i}.ts"), ("video/mp2t", body));
        expected.extend(plain);
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    routes.insert("/v/high/index.m3u8".into(), (mpegurl, playlist.into_bytes()));
    routes.insert("/keys/k1".into(), ("application/octet-stream", KEY.to_vec()));
    (serve(routes, delay).await, expected)
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("stewardget-hls-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn engine(dir: &std::path::Path) -> Engine {
    let config = Config { download_dir: dir.to_path_buf(), connections: 4, ..Config::default() };
    Engine::start(EngineOptions::in_memory().with_config(config)).unwrap()
}

async fn wait_for(engine: &Engine, id: u64, target: Status, secs: u64) -> Status {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let status = engine.download(id).unwrap().status;
        if status == target || status == Status::Failed || std::time::Instant::now() > deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saves_an_encrypted_stream_as_one_file() {
    let (base, expected) = stream(Duration::ZERO).await;
    let dir = temp_dir("plain");
    let engine = engine(&dir);
    let id = engine
        .add(NewDownload {
            url: format!("{base}/v/master.m3u8"),
            options: DownloadOptions { file_name: Some("My Video.m3u8".into()), ..Default::default() },
            ..Default::default()
        })
        .unwrap();
    let status = wait_for(&engine, id, Status::Completed, 30).await;
    let view = engine.download(id).unwrap();
    assert_eq!(status, Status::Completed, "{:?}", view.error);
    assert_eq!(view.file_name, "My Video.ts");
    assert_eq!(std::fs::read(&view.path).unwrap(), expected, "pieces decrypted and in order");
    assert!(!dir.join("My Video.ts.vxmeta").exists());

    let info = engine.check_link(&format!("{base}/v/master.m3u8"), &DownloadOptions::default()).await.unwrap();
    assert_eq!(info.file_name, "master.ts");
    assert!(info.resumable);
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_stream_resumes_where_it_stopped() {
    let (base, expected) = stream(Duration::from_millis(250)).await;
    let dir = temp_dir("resume");
    let engine = engine(&dir);
    let id = engine.add(NewDownload::new(format!("{base}/v/master.m3u8"))).unwrap();
    // Let a few pieces arrive, then pause.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while engine.download(id).unwrap().downloaded < 150_000 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    engine.pause(id).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let paused = engine.download(id).unwrap();
    assert_eq!(paused.status, Status::Paused);
    assert!(paused.downloaded > 0 && paused.downloaded < expected.len() as u64);
    engine.resume(id).unwrap();
    let status = wait_for(&engine, id, Status::Completed, 30).await;
    let view = engine.download(id).unwrap();
    assert_eq!(status, Status::Completed, "{:?}", view.error);
    assert_eq!(std::fs::read(&view.path).unwrap(), expected);
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_streams_are_refused_clearly() {
    let mut routes: HashMap<String, (&'static str, Vec<u8>)> = HashMap::new();
    routes.insert("/live.m3u8".into(), ("application/x-mpegurl", b"#EXTM3U\n#EXTINF:4,\na.ts\n".to_vec()));
    let base = serve(routes, Duration::ZERO).await;
    let dir = temp_dir("live");
    let engine = engine(&dir);
    let id = engine.add(NewDownload::new(format!("{base}/live.m3u8"))).unwrap();
    assert_eq!(wait_for(&engine, id, Status::Failed, 20).await, Status::Failed);
    assert!(engine.download(id).unwrap().error.unwrap().contains("live stream"));
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn byte_range_pieces_survive_a_server_that_ignores_ranges() {
    // Three pieces cut from one file with #EXT-X-BYTERANGE; this server answers
    // every range with the whole file, so each piece has to be cut out locally.
    let whole: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
    let mut routes: HashMap<String, (&'static str, Vec<u8>)> = HashMap::new();
    let playlist = "#EXTM3U\n#EXT-X-BYTERANGE:1000@0\n#EXTINF:4,\nall.ts\n#EXT-X-BYTERANGE:1500\n#EXTINF:4,\nall.ts\n#EXT-X-BYTERANGE:500@3000\n#EXTINF:4,\nall.ts\n#EXT-X-ENDLIST\n";
    routes.insert("/s/index.m3u8".into(), ("application/vnd.apple.mpegurl", playlist.as_bytes().to_vec()));
    routes.insert("/s/all.ts".into(), ("video/mp2t", whole.clone()));
    let base = serve(routes, Duration::ZERO).await;
    let dir = temp_dir("byterange");
    let engine = engine(&dir);
    let id = engine.add(NewDownload::new(format!("{base}/s/index.m3u8"))).unwrap();
    let status = wait_for(&engine, id, Status::Completed, 30).await;
    let view = engine.download(id).unwrap();
    assert_eq!(status, Status::Completed, "{:?}", view.error);
    let expected: Vec<u8> = [&whole[..2500], &whole[3000..3500]].concat();
    let got = std::fs::read(&view.path).unwrap();
    assert_eq!(got.len(), expected.len());
    assert!(got == expected, "the pieces are not the right parts of the file");
}
