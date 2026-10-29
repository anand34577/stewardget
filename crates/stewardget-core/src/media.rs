//! Finding the real files behind video and audio pages.
//!
//! * **yt-dlp**: when the program is installed, it is asked what a page offers
//!   (`yt-dlp -J`) and the direct links it reports are downloaded by
//!   StewardGet's own engine, with all its connections, pausing and resuming.
//!   StewardGet doesn't bundle or update yt-dlp.
//! * **DASH** (`.mpd`) manifests whose streams are single files are read
//!   directly. Manifests made of many small segments aren't supported.
//!
//! Many sites offer the best quality as a separate video and audio stream. Both
//! are downloaded and joined without re-encoding by `ffmpeg`, if it's installed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, filename};

/// Which stream of a merged download a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Video,
    Audio,
}

/// Marks a download as one half of a video that must be joined with its other
/// half once both have arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mux {
    /// Shared by both halves.
    pub group: String,
    pub stream: Stream,
    /// File name of the joined result.
    pub output: String,
}

/// One file of a choice.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaPart {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub ext: String,
    /// `None`: the file already holds picture and sound (or only sound).
    pub stream: Option<Stream>,
    pub size: Option<u64>,
}

/// Something the user can pick: one file, or a video and an audio file to join.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaOption {
    pub label: String,
    pub parts: Vec<MediaPart>,
    /// Picture height, for ordering.
    pub height: u32,
}

impl MediaOption {
    pub fn needs_merge(&self) -> bool {
        self.parts.len() > 1
    }

    pub fn size(&self) -> Option<u64> {
        self.parts.iter().map(|p| p.size).sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub title: String,
    /// What found it: `yt-dlp` or `DASH manifest`.
    pub source: &'static str,
    pub options: Vec<MediaOption>,
}

/// An executable named `name`. The path the user chose comes first (a folder
/// holding the program is fine too), then `PATH`, then the places installers
/// and package managers put programs, which a running app often doesn't see
/// in its `PATH` until it is restarted.
pub fn find_tool(configured: Option<&Path>, name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    if let Some(p) = configured {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
        if let Some(inside) = [p.join(&exe), p.join("bin").join(&exe)].into_iter().find(|c| c.is_file()) {
            return Some(inside);
        }
    }
    let on_path =
        std::env::var_os("PATH").map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>()).unwrap_or_default();
    on_path.into_iter().chain(common_dirs()).map(|d| d.join(&exe)).find(|p| p.is_file())
}

fn common_dirs() -> Vec<PathBuf> {
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(local) = var("LOCALAPPDATA") {
            dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
        }
        if let Some(home) = var("USERPROFILE") {
            dirs.push(home.join("scoop").join("shims"));
        }
        if let Some(data) = var("ProgramData") {
            dirs.push(data.join("chocolatey").join("bin"));
        }
        if let Some(pf) = var("ProgramFiles") {
            dirs.push(pf.join("ffmpeg").join("bin"));
        }
    } else {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/snap/bin"].map(PathBuf::from));
        if let Some(home) = var("HOME") {
            dirs.push(home.join(".local").join("bin"));
        }
    }
    dirs
}

/// Runs the program with its version option and checks that it really is
/// `name`; returns the version. Blocking (a few seconds at most).
pub fn tool_version(path: &Path, name: &str) -> Result<String, String> {
    let arg = if name == "ffmpeg" { "-version" } else { "--version" };
    let mut cmd = std::process::Command::new(path);
    cmd.arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("could not start it: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            return Err("it didn't answer in time".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout.by_ref().take(4096).read_to_string(&mut out);
    }
    let first = out.lines().next().unwrap_or("").trim();
    parse_version(name, first).ok_or_else(|| format!("that file doesn't look like {name}"))
}

fn parse_version(name: &str, first_line: &str) -> Option<String> {
    if name == "ffmpeg" {
        // "ffmpeg version 7.1-full_build-www.gyan.dev Copyright …"
        let rest = first_line.strip_prefix("ffmpeg version ")?;
        return rest.split_whitespace().next().map(str::to_owned);
    }
    // yt-dlp prints a date such as 2025.09.26.
    let looks_like_date = first_line.len() >= 8
        && first_line.split('.').count() >= 3
        && first_line.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    looks_like_date.then(|| first_line.to_owned())
}

/// What a page or manifest offers.
pub async fn inspect(client: &Client, ytdlp: Option<&Path>, url: &str) -> Result<MediaInfo, Error> {
    let lower = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if lower.ends_with(".mpd") {
        let text = client.get(url).send().await?.error_for_status()?.text().await?;
        let title = filename::from_url(&url::Url::parse(url).map_err(|e| Error::InvalidUrl(e.to_string()))?)
            .map(|n| n.trim_end_matches(".mpd").to_owned())
            .unwrap_or_else(|| "video".into());
        return parse_mpd(&text, url, &title);
    }
    let Some(tool) = ytdlp else {
        return Err(Error::Unsupported(
            "this page needs yt-dlp, which isn't installed. Install it (for example `winget install yt-dlp`) and try again".into(),
        ));
    };
    let output = tokio::time::timeout(
        Duration::from_secs(90),
        tokio::process::Command::new(tool)
            .args(["-J", "--no-playlist", "--no-warnings"])
            .arg("--")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::Timeout)?
    .map_err(|e| Error::Io(format!("could not start yt-dlp: {e}")))?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr);
        let last = msg.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("yt-dlp failed");
        return Err(Error::Unsupported(last.trim().trim_start_matches("ERROR: ").to_owned()));
    }
    let json: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| Error::Io(format!("unreadable yt-dlp answer: {e}")))?;
    Ok(options_from_ytdlp(&json))
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty() && *s != "none").map(str::to_owned)
}

fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

struct Format {
    part: MediaPart,
    height: u32,
    has_video: bool,
    has_audio: bool,
    rate: f64,
    codec: String,
}

fn format_of(f: &Value) -> Option<Format> {
    let url = text(f, "url")?;
    // Fragmented streams (HLS, DASH segments) have no single file to fetch.
    let protocol = text(f, "protocol").unwrap_or_default();
    if !matches!(protocol.as_str(), "https" | "http" | "") {
        return None;
    }
    let has_video = text(f, "vcodec").is_some();
    let has_audio = text(f, "acodec").is_some();
    if !has_video && !has_audio {
        return None;
    }
    let headers = f
        .get("http_headers")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))).collect())
        .unwrap_or_default();
    let ext = text(f, "ext").unwrap_or_else(|| "bin".into());
    Some(Format {
        part: MediaPart {
            url,
            headers,
            ext,
            stream: None,
            size: number(f, "filesize").or_else(|| number(f, "filesize_approx")).map(|n| n as u64),
        },
        height: number(f, "height").unwrap_or(0.0) as u32,
        has_video,
        has_audio,
        rate: number(f, "tbr").or_else(|| number(f, "abr")).unwrap_or(0.0),
        codec: text(f, "acodec").unwrap_or_default(),
    })
}

/// Turns yt-dlp's JSON into choices: ready-made files, separate video and audio
/// to join, and sound only.
pub(crate) fn options_from_ytdlp(json: &Value) -> MediaInfo {
    let title = text(json, "title").unwrap_or_else(|| "video".into());
    let formats: Vec<Format> = json
        .get("formats")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(format_of).collect())
        .unwrap_or_default();
    // A single-file answer (no formats list) still has a top-level url.
    let formats = if formats.is_empty() { format_of(json).into_iter().collect() } else { formats };
    let best = |list: &mut Vec<&Format>| {
        list.sort_by(|a, b| b.rate.partial_cmp(&a.rate).unwrap_or(std::cmp::Ordering::Equal));
    };
    let mut options = Vec::new();

    // Files with picture and sound: best bit rate per picture height.
    let mut whole: Vec<&Format> = formats.iter().filter(|f| f.has_video && f.has_audio).collect();
    best(&mut whole);
    let mut seen = Vec::new();
    for f in whole {
        if seen.contains(&f.height) {
            continue;
        }
        seen.push(f.height);
        options.push(MediaOption {
            label: format!("{} · {} (picture and sound)", height_label(f.height), f.part.ext.to_uppercase()),
            parts: vec![f.part.clone()],
            height: f.height,
        });
    }

    // Separate streams: usually the highest quality. Needs joining.
    let mut audio: Vec<&Format> = formats.iter().filter(|f| f.has_audio && !f.has_video).collect();
    best(&mut audio);
    let mut video: Vec<&Format> = formats.iter().filter(|f| f.has_video && !f.has_audio).collect();
    best(&mut video);
    let mut seen = Vec::new();
    for v in video {
        if seen.contains(&v.height) {
            continue;
        }
        // MP4 video joins best with AAC; WebM with Opus/Vorbis; otherwise take the best.
        let wants = if v.part.ext == "mp4" { "m4a" } else { "webm" };
        let Some(a) = audio.iter().find(|a| a.part.ext == wants).or_else(|| audio.first()) else { continue };
        seen.push(v.height);
        let mut vp = v.part.clone();
        let mut ap = a.part.clone();
        vp.stream = Some(Stream::Video);
        ap.stream = Some(Stream::Audio);
        options.push(MediaOption {
            label: format!(
                "{} · {} + {} (joined with ffmpeg)",
                height_label(v.height),
                v.part.ext.to_uppercase(),
                a.part.ext.to_uppercase()
            ),
            parts: vec![vp, ap],
            height: v.height,
        });
    }
    options.sort_by_key(|o| std::cmp::Reverse((o.height, o.needs_merge())));

    // Sound only.
    for a in audio.iter().take(2) {
        let kbps = if a.rate > 0.0 { format!(" {:.0} kbps", a.rate) } else { String::new() };
        options.push(MediaOption {
            label: format!(
                "Audio only · {}{kbps}{}",
                a.part.ext.to_uppercase(),
                if a.codec.is_empty() { String::new() } else { format!(" ({})", a.codec) }
            ),
            parts: vec![a.part.clone()],
            height: 0,
        });
    }
    options.truncate(14);
    MediaInfo { title, source: "yt-dlp", options }
}

fn height_label(h: u32) -> String {
    if h == 0 { "Video".into() } else { format!("{h}p") }
}

/// Reads a DASH manifest whose representations are single files (`BaseURL`).
pub(crate) fn parse_mpd(xml: &str, manifest_url: &str, title: &str) -> Result<MediaInfo, Error> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| Error::Unsupported(format!("unreadable DASH manifest: {e}")))?;
    let base = url::Url::parse(manifest_url).map_err(|e| Error::InvalidUrl(e.to_string()))?;
    let mut videos: Vec<(u64, u32, MediaPart)> = Vec::new();
    let mut audios: Vec<(u64, MediaPart)> = Vec::new();
    let mut segmented = false;
    // `BaseURL` may sit on the manifest, period, adaptation set or representation.
    let base_url = |n: roxmltree::Node| -> Option<String> {
        n.children().find(|c| c.has_tag_name("BaseURL")).and_then(|c| c.text()).map(|t| t.trim().to_owned())
    };
    for set in doc.descendants().filter(|n| n.has_tag_name("AdaptationSet")) {
        let set_type = set.attribute("contentType").or_else(|| set.attribute("mimeType")).unwrap_or("");
        for rep in set.children().filter(|n| n.has_tag_name("Representation")) {
            let mime = rep.attribute("mimeType").unwrap_or(set_type);
            let has_template =
                rep.descendants().any(|n| n.has_tag_name("SegmentTemplate") || n.has_tag_name("SegmentList"))
                    || set.children().any(|n| n.has_tag_name("SegmentTemplate"));
            let Some(rel) = base_url(rep) else {
                segmented |= has_template;
                continue;
            };
            if has_template {
                segmented = true;
                continue;
            }
            let Ok(url) = base.join(&rel) else { continue };
            let bandwidth = rep.attribute("bandwidth").and_then(|b| b.parse().ok()).unwrap_or(0u64);
            let ext = match mime.split('/').nth(1).unwrap_or("bin") {
                "mp4" => {
                    if mime.starts_with("audio") {
                        "m4a"
                    } else {
                        "mp4"
                    }
                }
                "webm" => "webm",
                other => other,
            }
            .to_owned();
            let mut part = MediaPart { url: url.to_string(), headers: Vec::new(), ext, stream: None, size: None };
            if mime.starts_with("video") || set_type.starts_with("video") {
                let height = rep.attribute("height").and_then(|h| h.parse().ok()).unwrap_or(0);
                part.stream = Some(Stream::Video);
                videos.push((bandwidth, height, part));
            } else if mime.starts_with("audio") || set_type.starts_with("audio") {
                part.stream = Some(Stream::Audio);
                audios.push((bandwidth, part));
            }
        }
    }
    if videos.is_empty() && audios.is_empty() {
        return Err(Error::Unsupported(if segmented {
            "this DASH manifest delivers the stream in many small segments, which StewardGet can't download yet. If yt-dlp is installed, give it the page address instead".into()
        } else {
            "no downloadable streams found in this manifest".into()
        }));
    }
    audios.sort_by_key(|a| std::cmp::Reverse(a.0));
    videos.sort_by_key(|v| std::cmp::Reverse((v.1, v.0)));
    let mut options = Vec::new();
    for (_, height, v) in &videos {
        let mut parts = vec![v.clone()];
        let mut label = format!("{} · {}", height_label(*height), v.ext.to_uppercase());
        if let Some((_, a)) = audios.first() {
            parts.push(a.clone());
            label.push_str(&format!(" + {} (joined with ffmpeg)", a.ext.to_uppercase()));
        }
        options.push(MediaOption { label, parts, height: *height });
    }
    if let Some((rate, a)) = audios.first() {
        options.push(MediaOption {
            label: format!("Audio only · {} {} kbps", a.ext.to_uppercase(), rate / 1000),
            parts: vec![MediaPart { stream: None, ..a.clone() }],
            height: 0,
        });
    }
    Ok(MediaInfo { title: title.to_owned(), source: "DASH manifest", options })
}

/// Joins a video and an audio file into `output` without re-encoding.
pub fn merge(ffmpeg: &Path, video: &Path, audio: &Path, output: &Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .arg("-i")
        .arg(audio)
        .args(["-c", "copy"])
        .arg(output);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdin(std::process::Stdio::null());
    let result = cmd.output()?;
    if result.status.success() && output.is_file() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&result.stderr);
        Err(std::io::Error::other(msg.lines().last().unwrap_or("ffmpeg failed").to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fmt(id: &str, ext: &str, v: &str, a: &str, height: u32, tbr: f64) -> Value {
        json!({ "format_id": id, "url": format!("https://cdn.test/{id}.{ext}"), "ext": ext, "protocol": "https",
                "vcodec": v, "acodec": a, "height": height, "tbr": tbr, "filesize": 1000 * tbr as u64,
                "http_headers": { "User-Agent": "yt" } })
    }

    #[test]
    fn ytdlp_formats_become_choices() {
        let mut hls = fmt("hls", "mp4", "avc1", "mp4a", 2160, 9000.0);
        hls["protocol"] = json!("m3u8_native");
        let info = options_from_ytdlp(&json!({
            "title": "Demo",
            "formats": [
                fmt("18", "mp4", "avc1", "mp4a", 360, 500.0),
                fmt("137", "mp4", "avc1", "none", 1080, 4000.0),
                fmt("136", "mp4", "avc1", "none", 720, 2000.0),
                fmt("140", "m4a", "none", "mp4a", 0, 128.0),
                fmt("251", "webm", "none", "opus", 0, 160.0),
                hls,
            ]
        }));
        assert_eq!(info.title, "Demo");
        let labels: Vec<_> = info.options.iter().map(|o| o.label.as_str()).collect();
        assert!(labels[0].starts_with("1080p") && labels[0].contains("joined"), "{labels:?}");
        assert!(labels.iter().any(|l| l.starts_with("360p") && l.contains("picture and sound")));
        assert!(labels.iter().any(|l| l.starts_with("Audio only")));
        assert!(!labels.iter().any(|l| l.contains("2160")), "fragmented streams are left out");
        let merged = &info.options[0];
        assert_eq!(merged.parts[0].stream, Some(Stream::Video));
        assert_eq!(merged.parts[1].ext, "m4a", "mp4 video pairs with m4a audio");
        assert_eq!(merged.parts[0].headers, vec![("User-Agent".to_owned(), "yt".to_owned())]);
    }

    #[test]
    fn dash_manifests_with_single_files_are_read() {
        let xml = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011"><Period>
            <AdaptationSet mimeType="video/mp4">
              <Representation bandwidth="3000000" height="1080"><BaseURL>v1080.mp4</BaseURL></Representation>
              <Representation bandwidth="900000" height="480"><BaseURL>v480.mp4</BaseURL></Representation>
            </AdaptationSet>
            <AdaptationSet mimeType="audio/mp4">
              <Representation bandwidth="128000"><BaseURL>a.m4a</BaseURL></Representation>
            </AdaptationSet></Period></MPD>"#;
        let info = parse_mpd(xml, "https://x.test/dir/stream.mpd", "stream").unwrap();
        assert_eq!(info.options.len(), 3);
        assert_eq!(info.options[0].parts[0].url, "https://x.test/dir/v1080.mp4");
        assert_eq!(info.options[0].parts[1].url, "https://x.test/dir/a.m4a");
        assert!(info.options[2].label.starts_with("Audio only"));
    }

    #[test]
    fn versions_are_recognised_and_other_programs_are_not() {
        assert_eq!(
            parse_version("ffmpeg", "ffmpeg version 7.1-full_build Copyright (c)").as_deref(),
            Some("7.1-full_build")
        );
        assert_eq!(parse_version("yt-dlp", "2025.09.26").as_deref(), Some("2025.09.26"));
        assert!(parse_version("yt-dlp", "Python 3.12.1").is_none());
        assert!(parse_version("ffmpeg", "usage: something").is_none());
    }

    #[test]
    fn a_folder_holding_the_program_counts() {
        let dir = std::env::temp_dir().join(format!("stewardget-tool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let exe = dir.join("bin").join(if cfg!(windows) { "faketool.exe" } else { "faketool" });
        std::fs::write(&exe, b"x").unwrap();
        assert_eq!(find_tool(Some(&dir), "faketool"), Some(exe.clone()));
        assert_eq!(find_tool(Some(&exe), "faketool"), Some(exe));
        assert!(find_tool(Some(&dir.join("nope")), "definitely-not-installed-xyz").is_none());
    }

    #[test]
    fn segmented_manifests_are_refused_with_a_reason() {
        let xml = r#"<MPD><Period><AdaptationSet mimeType="video/mp4">
            <SegmentTemplate media="s$Number$.m4s" duration="4"/>
            <Representation bandwidth="1" height="480"/></AdaptationSet></Period></MPD>"#;
        let e = parse_mpd(xml, "https://x.test/a.mpd", "a").unwrap_err();
        assert!(e.to_string().contains("small segments"), "{e}");
    }

    #[test]
    fn ffmpeg_joins_two_files_when_installed() {
        let Some(ffmpeg) = find_tool(None, "ffmpeg") else { return };
        let dir = std::env::temp_dir().join(format!("stewardget-mux-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (v, a, out) = (dir.join("v.mp4"), dir.join("a.m4a"), dir.join("out.mp4"));
        let run = |args: &[&str]| {
            std::process::Command::new(&ffmpeg)
                .args(["-y", "-loglevel", "error"])
                .args(args)
                .status()
                .unwrap()
                .success()
        };
        assert!(run(&["-f", "lavfi", "-i", "testsrc=duration=1:size=64x64:rate=5", v.to_str().unwrap()]));
        assert!(run(&["-f", "lavfi", "-i", "sine=duration=1", "-c:a", "aac", a.to_str().unwrap()]));
        merge(&ffmpeg, &v, &a, &out).unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
        assert!(merge(&ffmpeg, &v, &dir.join("missing.m4a"), &dir.join("x.mp4")).is_err());
    }
}
