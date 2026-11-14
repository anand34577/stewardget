//! End-to-end check of video downloads through yt-dlp against a real site.
//! Needs the network, yt-dlp and ffmpeg, so it only runs when asked:
//!
//! ```text
//! cargo test -p stewardget-core --test media_live -- --ignored --nocapture
//! ```
//!
//! `STEWARDGET_TEST_VIDEO` picks another page (default: a short public video).

use std::time::Duration;

use stewardget_core::{Config, Engine, EngineOptions, Status};

fn page() -> String {
    std::env::var("STEWARDGET_TEST_VIDEO").unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into())
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("stewardget-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Waits until none of `ids` is still in the list unfinished, and no join is pending.
async fn settle(engine: &Engine, ids: &[u64], secs: u64) {
    let mut events = engine.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let pending = ids.iter().filter_map(|id| engine.download(*id)).any(|d| !d.status.is_finished());
        if !pending {
            // A join runs after both halves finished; give it a moment.
            tokio::time::sleep(Duration::from_secs(3)).await;
            return;
        }
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(_) => {}
            Err(_) => panic!(
                "timed out; downloads: {:?}",
                ids.iter()
                    .filter_map(|id| engine.download(*id))
                    .map(|d| (d.file_name, d.status, d.error))
                    .collect::<Vec<_>>()
            ),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the network, yt-dlp and ffmpeg"]
async fn video_choices_download_and_join() {
    let dir = temp_dir("media");
    // The defaults a new user has, category folders included.
    // `STEWARDGET_TEST_YTDLP` points at a yt-dlp that isn't on PATH.
    let ytdlp_path = std::env::var_os("STEWARDGET_TEST_YTDLP").map(Into::into);
    let config = Config { download_dir: dir.clone(), ytdlp_path, ..Config::default() };
    let engine = Engine::start(EngineOptions::in_memory().with_config(config)).unwrap();
    let (ytdlp, ffmpeg) = engine.media_tools();
    assert!(ytdlp.is_some() && ffmpeg.is_some(), "yt-dlp and ffmpeg must be installed: {ytdlp:?} {ffmpeg:?}");

    let info = engine.inspect_media(&page()).await.expect("yt-dlp reads the page");
    println!("{} — {} choices", info.title, info.options.len());
    for o in &info.options {
        println!("  {}  ({} part{})", o.label, o.parts.len(), if o.parts.len() == 1 { "" } else { "s" });
    }

    // One choice with picture and sound in one file (when the site offers one)
    // and the best one that has to be joined.
    let single = info.options.iter().position(|o| !o.needs_merge() && o.height > 0);
    let joined = info.options.iter().position(|o| o.needs_merge());
    assert!(joined.is_some() || single.is_some(), "no video choice at all");

    for index in [single, joined].into_iter().flatten() {
        let label = &info.options[index].label;
        let ids = engine.add_media(&info, index, Some(dir.join(format!("choice-{index}"))), None).unwrap();
        settle(&engine, &ids, 600).await;
        let survivors: Vec<_> = ids.iter().filter_map(|id| engine.download(*id)).collect();
        for d in &survivors {
            assert_eq!(d.status, Status::Completed, "{label}: {} ended {:?}: {:?}", d.file_name, d.status, d.error);
        }
        assert_eq!(survivors.len(), 1, "{label}: a joined choice leaves one entry, got {}", survivors.len());
        let d = &survivors[0];
        let meta = std::fs::metadata(&d.path).unwrap_or_else(|e| panic!("{label}: {} missing: {e}", d.path.display()));
        assert!(meta.len() > 10_000, "{label}: {} is only {} bytes", d.path.display(), meta.len());
        assert!(
            !d.file_name.contains(".video.") && !d.file_name.contains(".audio."),
            "{label}: the halves were not joined: {}",
            d.file_name
        );
        // No half-files left behind anywhere under the download folder.
        let leftovers: Vec<_> = walk(&dir)
            .into_iter()
            .filter(|p| {
                let n = p.file_name().unwrap().to_string_lossy().into_owned();
                n.contains(".video.") || n.contains(".audio.") || n.ends_with(".vxpart") || n.ends_with(".vxmeta")
            })
            .collect();
        assert!(leftovers.is_empty(), "{label}: left behind {leftovers:?}");
        println!("OK {label} -> {} ({} bytes)", d.path.display(), meta.len());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the network and yt-dlp"]
async fn an_expired_video_link_is_renewed_and_the_download_finishes() {
    let dir = temp_dir("renew");
    let ytdlp_path = std::env::var_os("STEWARDGET_TEST_YTDLP").map(Into::into);
    let config = Config { download_dir: dir.clone(), ytdlp_path, ..Config::default() };
    let engine = Engine::start(EngineOptions::in_memory().with_config(config)).unwrap();
    let mut info = engine.inspect_media(&page()).await.expect("yt-dlp reads the page");
    // Sound only: one small file.
    let index = info.options.iter().position(|o| o.height == 0 && !o.needs_merge()).expect("an audio choice");
    // What an old link looks like: the signed expiry no longer matches, and the server says 403.
    let part = &mut info.options[index].parts[0];
    let expire = part.url.split(['&', '?']).find(|p| p.starts_with("expire=")).expect("a signed link").to_owned();
    part.url = part.url.replacen(&expire, "expire=1000000000", 1);
    let ids = engine.add_media(&info, index, None, None).unwrap();
    assert!(engine.download(ids[0]).unwrap().options.media.is_some(), "the page and format are remembered");
    settle(&engine, &ids, 300).await;
    let d = engine.download(ids[0]).unwrap();
    assert_eq!(d.status, Status::Completed, "{:?}", d.error);
    assert!(!d.url.contains("expire=1000000000"), "the link was replaced");
    assert!(std::fs::metadata(&d.path).unwrap().len() > 10_000);
    println!("renewed and finished: {}", d.path.display());
    let _ = std::fs::remove_dir_all(&dir);
}
