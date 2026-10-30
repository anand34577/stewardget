//! HLS streams (`.m3u8`): the playlist is read, the best quality picked, and
//! its pieces fetched several at a time but written in order into one file:
//! MPEG-TS pieces make a `.ts`, fragmented MP4 pieces (with `#EXT-X-MAP`) an
//! `.mp4`. AES-128 encrypted streams are decrypted. Progress is saved per
//! piece, so a paused or interrupted stream resumes where it stopped.
//!
//! Live streams (no `#EXT-X-ENDLIST`) and SAMPLE-AES (DRM-style) encryption
//! are refused with a clear message.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::control::{meta_path, part_path};
use crate::probe::Probe;
use crate::segment::{Segment, SegmentTable};
use crate::task::{Job, Outcome};
use crate::{Error, Status, fsio};

/// Most pieces fetched at the same time.
const MAX_PARALLEL: usize = 8;
/// Largest playlist read.
const MAX_PLAYLIST: usize = 8 << 20;
const META_VERSION: u32 = 1;

/// Whether a link (as probed) is an HLS playlist.
pub(crate) fn is_playlist(url: &Url, probe: &Probe) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.ends_with(".m3u8") || probe.content_type.as_deref().is_some_and(|t| t.to_ascii_lowercase().contains("mpegurl"))
}

#[derive(Debug, Clone, PartialEq)]
struct Key {
    uri: Url,
    iv: Option<[u8; 16]>,
}

#[derive(Debug, Clone, PartialEq)]
struct Piece {
    url: Url,
    /// `(offset, length)` within `url` (`#EXT-X-BYTERANGE`).
    range: Option<(u64, u64)>,
    key: Option<Key>,
    /// Media sequence number (the default IV).
    seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct MediaPlaylist {
    /// `#EXT-X-MAP`: the fMP4 initialisation section.
    init: Option<Piece>,
    pieces: Vec<Piece>,
}

/// Resume record kept in `<file>.vxmeta` while a stream downloads.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct StreamMeta {
    version: u32,
    url: String,
    pieces: usize,
    /// Pieces (after the init section) fully written.
    done: usize,
    /// Bytes of the file that hold them.
    bytes: u64,
}

pub(crate) async fn run(job: &Job, headers: &HeaderMap, probe: Probe) -> Result<Outcome, Error> {
    // A server that stalls mid-playlist must neither hang the download nor ignore Pause.
    let timeout = Duration::from_secs(job.config.read_timeout_secs.max(5) * 2);
    let media = tokio::select! {
        biased;
        _ = job.cancel.cancelled() => return Err(Error::Cancelled),
        m = tokio::time::timeout(timeout, load(&job.client, headers, &probe.final_url)) => m.map_err(|_| Error::Timeout)??,
    };
    if media.pieces.is_empty() {
        return Err(Error::Unsupported("the stream's playlist lists no pieces".into()));
    }
    let ext = if media.init.is_some() { "mp4" } else { "ts" };
    let stream_probe =
        Probe { stream_ext: Some(ext), total: None, ranges: false, etag: None, last_modified: None, ..probe };
    let dest = job.hooks.on_probe(&stream_probe);
    let (part, meta) = (part_path(&dest), meta_path(&dest));
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }

    // Continue an earlier run of the same stream, or start clean.
    let previous = std::fs::read(&meta)
        .ok()
        .and_then(|b| serde_json::from_slice::<StreamMeta>(&b).ok())
        // A refreshed link (new token in the URL) continues the same stream.
        .filter(|m| {
            m.version == META_VERSION
                && (m.url == job.url.as_str() || job.relaxed_resume)
                && m.pieces == media.pieces.len()
        })
        .filter(|m| std::fs::metadata(&part).is_ok_and(|f| f.len() >= m.bytes));
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&part)?;
    let (mut done, mut bytes) = previous.map_or((0, 0), |m| (m.done, m.bytes));
    file.set_len(bytes)?;
    let file = Arc::new(file);

    let shared = &job.shared;
    let set_progress = |done: usize, bytes: u64| {
        // An estimate of the whole from the pieces so far drives the bar and the time left.
        let estimate = if done == 0 { u64::MAX } else { bytes / done as u64 * media.pieces.len() as u64 };
        let total = if done == media.pieces.len() { bytes } else { estimate.max(bytes + 1) };
        shared.total.store(total, Ordering::Relaxed);
        let mut seg = Segment::new(0, if total == u64::MAX { crate::segment::UNBOUNDED } else { total }, bytes);
        seg.active = done < media.pieces.len();
        *shared.table.lock().unwrap() = SegmentTable { segments: vec![seg] };
    };
    set_progress(done, bytes);
    shared.resumable.store(true, Ordering::Relaxed);
    job.hooks.on_status(Status::Downloading);

    let ctx = Fetcher { job, headers, keys: tokio::sync::Mutex::new(HashMap::new()) };
    if bytes == 0
        && let Some(init) = &media.init
    {
        let data = ctx.piece(init).await?;
        fsio::write_at(&file, 0, data.clone()).await?;
        bytes = data.len() as u64;
    }
    let parallel = job.options.connections.unwrap_or(job.config.connections).clamp(1, MAX_PARALLEL);
    let ctx = &ctx;
    // Owned pieces: a stream over borrowed ones trips up the spawned task's lifetimes.
    let mut fetched = futures_util::stream::iter(media.pieces[done..].to_vec())
        .map(move |p| async move { ctx.piece(&p).await })
        .buffered(parallel);
    let mut saved = Instant::now();
    loop {
        let next = tokio::select! {
            biased;
            _ = job.cancel.cancelled() => None,
            n = fetched.next() => Some(n),
        };
        let Some(next) = next else {
            save_meta(&meta, job, media.pieces.len(), done, bytes, &file).await;
            return Err(Error::Cancelled);
        };
        let Some(data) = next else { break };
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                save_meta(&meta, job, media.pieces.len(), done, bytes, &file).await;
                return Err(e);
            }
        };
        let len = data.len() as u64;
        fsio::write_at(&file, bytes, data).await?;
        bytes += len;
        done += 1;
        set_progress(done, bytes);
        if saved.elapsed() >= Duration::from_secs(3) {
            save_meta(&meta, job, media.pieces.len(), done, bytes, &file).await;
            saved = Instant::now();
        }
    }
    drop(fetched);
    set_progress(done, bytes);
    let final_probe = Probe { total: Some(bytes), ..stream_probe };
    crate::task::finalize(job, &final_probe, file, &dest, &part, &meta).await
}

/// Flushes the data, then records how far the stream got.
async fn save_meta(meta: &Path, job: &Job, pieces: usize, done: usize, bytes: u64, file: &Arc<std::fs::File>) {
    let f = Arc::clone(file);
    if !matches!(tokio::task::spawn_blocking(move || f.sync_data()).await, Ok(Ok(()))) {
        return;
    }
    let record = StreamMeta { version: META_VERSION, url: job.url.to_string(), pieces, done, bytes };
    let (meta, json) = (meta.to_path_buf(), serde_json::to_vec(&record).unwrap_or_default());
    let _ = tokio::task::spawn_blocking(move || fsio::write_atomic(&meta, &json)).await;
}

/// The media playlist behind a link: the best variant of a master playlist, or the playlist itself.
async fn load(client: &reqwest::Client, headers: &HeaderMap, url: &Url) -> Result<MediaPlaylist, Error> {
    let (playlist_url, text) = fetch_playlist(client, headers, url).await?;
    match best_variant(&text, &playlist_url) {
        Some(variant) => parse_media(&fetch_playlist(client, headers, &variant).await?.1, &variant),
        None => parse_media(&text, &playlist_url),
    }
}

/// For the "Add download" check: the file type a stream will be saved as.
pub(crate) async fn inspect(client: &reqwest::Client, headers: &HeaderMap, url: &Url) -> Result<&'static str, Error> {
    let media = load(client, headers, url).await?;
    Ok(if media.init.is_some() { "mp4" } else { "ts" })
}

async fn fetch_playlist(client: &reqwest::Client, headers: &HeaderMap, url: &Url) -> Result<(Url, String), Error> {
    let resp = client.get(url.clone()).headers(headers.clone()).send().await?;
    if !resp.status().is_success() {
        return Err(crate::probe::http_error(&resp));
    }
    let final_url = resp.url().clone();
    let too_large = || Error::Unsupported("the stream's playlist is too large".into());
    if resp.content_length().is_some_and(|n| n > MAX_PLAYLIST as u64) {
        return Err(too_large());
    }
    // Checked while reading: a mislabelled link to a big file must not end up in memory.
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        if body.len() > MAX_PLAYLIST {
            return Err(too_large());
        }
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err(Error::Unsupported("this link is not an HLS playlist".into()));
    }
    Ok((final_url, text))
}

/// In a master playlist, the variant with the highest bandwidth. Variants
/// whose sound is a separate track (`#EXT-X-MEDIA:TYPE=AUDIO` with its own
/// URI) come second: StewardGet saves one track, and that video would be silent.
fn best_variant(text: &str, base: &Url) -> Option<Url> {
    let separate_audio: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("#EXT-X-MEDIA:"))
        .filter(|a| {
            attribute(a, "TYPE").is_some_and(|t| t.eq_ignore_ascii_case("AUDIO")) && attribute(a, "URI").is_some()
        })
        .filter_map(|a| attribute(a, "GROUP-ID"))
        .collect();
    let mut best: Option<((bool, u64), Url)> = None;
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") else { continue };
        let bandwidth = attribute(attrs, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0);
        let muxed = attribute(attrs, "AUDIO").is_none_or(|g| !separate_audio.contains(&g));
        let rank = (muxed, bandwidth);
        let Some(uri) = lines.by_ref().find(|l| !l.is_empty() && !l.starts_with('#')) else { break };
        if let Ok(url) = base.join(uri)
            && best.as_ref().is_none_or(|(r, _)| rank > *r)
        {
            best = Some((rank, url));
        }
    }
    best.map(|(_, url)| url)
}

fn parse_media(text: &str, base: &Url) -> Result<MediaPlaylist, Error> {
    let mut seq: u64 = 0;
    let mut key: Option<Key> = None;
    let mut init = None;
    let mut pieces = Vec::new();
    let mut range: Option<(u64, Option<u64>)> = None;
    let mut ended = false;
    // Where the previous byte range of each URI ended (`#EXT-X-BYTERANGE:n` without `@o`).
    let mut range_end: HashMap<Url, u64> = HashMap::new();
    let join = |uri: &str| base.join(uri).map_err(|e| Error::InvalidUrl(format!("stream piece {uri}: {e}")));
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(n) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = n.trim().parse().unwrap_or(0);
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-KEY:") {
            key = match attribute(attrs, "METHOD").as_deref() {
                Some("NONE") | None => None,
                Some("AES-128") => {
                    let uri =
                        attribute(attrs, "URI").ok_or_else(|| Error::Unsupported("stream key without URI".into()))?;
                    let iv = attribute(attrs, "IV").and_then(|iv| parse_iv(&iv));
                    Some(Key { uri: join(&uri)?, iv })
                }
                Some(other) => {
                    return Err(Error::Unsupported(format!(
                        "this stream is protected ({other}); StewardGet can only save unprotected or AES-128 streams"
                    )));
                }
            };
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            let uri = attribute(attrs, "URI").ok_or_else(|| Error::Unsupported("stream map without URI".into()))?;
            let url = join(&uri)?;
            let range = attribute(attrs, "BYTERANGE").and_then(|r| parse_range(&r, 0));
            init = Some(Piece { url, range, key: key.clone(), seq: 0 });
        } else if let Some(r) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            range = parse_range_raw(r);
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if line.starts_with('#') {
            // Durations, discontinuities and the rest don't change the bytes.
        } else {
            let url = join(line)?;
            let piece_range = range.take().map(|(len, offset)| {
                let offset = offset.unwrap_or_else(|| range_end.get(&url).copied().unwrap_or(0));
                range_end.insert(url.clone(), offset + len);
                (offset, len)
            });
            pieces.push(Piece { url, range: piece_range, key: key.clone(), seq });
            seq += 1;
        }
    }
    if !ended {
        return Err(Error::Unsupported("this is a live stream; StewardGet saves finished videos only".into()));
    }
    Ok(MediaPlaylist { init, pieces })
}

/// `n@o` → `(n, Some(o))`, `n` → `(n, None)`.
fn parse_range_raw(r: &str) -> Option<(u64, Option<u64>)> {
    let mut it = r.trim().split('@');
    let len = it.next()?.trim().parse().ok()?;
    let offset = it.next().and_then(|o| o.trim().parse().ok());
    Some((len, offset))
}

fn parse_range(r: &str, default_offset: u64) -> Option<(u64, u64)> {
    parse_range_raw(r).map(|(len, o)| (o.unwrap_or(default_offset), len))
}

fn parse_iv(iv: &str) -> Option<[u8; 16]> {
    let hex = iv.trim().strip_prefix("0x").or_else(|| iv.trim().strip_prefix("0X"))?;
    let hex = format!("{hex:0>32}");
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A playlist attribute: `NAME=value` or `NAME="value"`, commas inside quotes respected.
fn attribute(attrs: &str, name: &str) -> Option<String> {
    let mut rest = attrs;
    while !rest.is_empty() {
        let (key, after) = rest.split_once('=')?;
        let (value, next) = match after.strip_prefix('"') {
            Some(q) => {
                let end = q.find('"')?;
                (&q[..end], q[end + 1..].trim_start_matches(','))
            }
            None => after.split_once(',').unwrap_or((after, "")),
        };
        if key.trim().eq_ignore_ascii_case(name) {
            return Some(value.to_owned());
        }
        rest = next;
    }
    None
}

struct Fetcher<'a> {
    job: &'a Job,
    headers: &'a HeaderMap,
    keys: tokio::sync::Mutex<HashMap<Url, [u8; 16]>>,
}

impl Fetcher<'_> {
    /// One piece, retried like a connection, decrypted when needed.
    async fn piece(&self, piece: &Piece) -> Result<Vec<u8>, Error> {
        let mut attempt = 0;
        let data = loop {
            match self.get(&piece.url, piece.range).await {
                Ok(d) => break d,
                Err(e) if e.is_retryable() && attempt < self.job.config.max_retries => {
                    attempt += 1;
                    self.job.shared.retrying.fetch_add(1, Ordering::Relaxed);
                    let waited = crate::task::backoff(attempt, e.retry_after(), &self.job.cancel).await;
                    self.job.shared.retrying.fetch_sub(1, Ordering::Relaxed);
                    waited?;
                }
                Err(e) => return Err(e),
            }
        };
        match &piece.key {
            None => Ok(data),
            Some(key) => {
                let secret = self.key(&key.uri).await?;
                let iv = key.iv.unwrap_or_else(|| (piece.seq as u128).to_be_bytes());
                decrypt(&secret, &iv, &data)
            }
        }
    }

    async fn key(&self, uri: &Url) -> Result<[u8; 16], Error> {
        let mut keys = self.keys.lock().await;
        if let Some(k) = keys.get(uri) {
            return Ok(*k);
        }
        let bytes = self.get(uri, None).await?;
        let key: [u8; 16] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::Unsupported("the stream's key has the wrong size".into()))?;
        keys.insert(uri.clone(), key);
        Ok(key)
    }

    async fn get(&self, url: &Url, range: Option<(u64, u64)>) -> Result<Vec<u8>, Error> {
        // Cookies and passwords belong to the site they were meant for, not to
        // whatever hosts a playlist names.
        let headers = if crate::task::same_origin(&self.job.url, url) {
            self.headers.clone()
        } else {
            crate::task::without_credentials(self.headers.clone())
        };
        let mut req = self.job.client.get(url.clone()).headers(headers);
        if let Some((offset, len)) = range {
            req = req.header(header::RANGE, format!("bytes={offset}-{}", offset + len.max(1) - 1));
        }
        let timeout = Duration::from_secs(self.job.config.read_timeout_secs);
        let resp = tokio::select! {
            biased;
            _ = self.job.cancel.cancelled() => return Err(Error::Cancelled),
            r = tokio::time::timeout(timeout, req.send()) => r.map_err(|_| Error::Timeout)??,
        };
        if !resp.status().is_success() {
            return Err(crate::probe::http_error(&resp));
        }
        // A server that ignores `Range` sends the whole file: keep only the piece.
        let whole = range.is_some() && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT;
        let mut stream = resp.bytes_stream();
        let mut out = Vec::new();
        loop {
            let next = tokio::select! {
                biased;
                _ = self.job.cancel.cancelled() => return Err(Error::Cancelled),
                n = tokio::time::timeout(timeout, stream.next()) => n.map_err(|_| Error::Timeout)?,
            };
            let Some(chunk) = next else { break };
            let chunk = chunk?;
            let n = chunk.len();
            // Speed limits may make this wait a while; pausing must not.
            tokio::select! {
                biased;
                _ = self.job.cancel.cancelled() => return Err(Error::Cancelled),
                _ = async {
                    self.job.shared.limiter.acquire(n).await;
                    self.job.queue_limiter.acquire(n).await;
                    self.job.global_limiter.acquire(n).await;
                } => {}
            }
            self.job.shared.transferred.fetch_add(n as u64, Ordering::Relaxed);
            out.extend_from_slice(&chunk);
            if let Some((offset, len)) = range.filter(|_| whole)
                && out.len() as u64 >= offset + len
            {
                break; // the rest of the file isn't needed
            }
        }
        if let Some((offset, len)) = range.filter(|_| whole) {
            let (start, end) = (offset as usize, (offset + len) as usize);
            if out.len() < end {
                return Err(Error::Network("a stream piece arrived incomplete".into()));
            }
            out.truncate(end);
            out.drain(..start);
        }
        Ok(out)
    }
}

fn decrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, Error> {
    use cbc::cipher::block_padding::Pkcs7;
    use cbc::cipher::{BlockModeDecrypt, KeyIvInit};
    cbc::Decryptor::<aes::Aes128>::new(key.into(), iv.into())
        .decrypt_padded_vec::<Pkcs7>(data)
        .map_err(|_| Error::Unsupported("a stream piece could not be decrypted (wrong key?)".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://cdn.example.com/v/master.m3u8?token=1").unwrap()
    }

    #[test]
    fn picks_the_best_variant() {
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\nhigh/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2500000\nmid/index.m3u8\n";
        assert_eq!(best_variant(master, &base()).unwrap().as_str(), "https://cdn.example.com/v/high/index.m3u8");
        assert!(best_variant("#EXTM3U\n#EXTINF:4,\na.ts\n#EXT-X-ENDLIST", &base()).is_none());
    }

    #[test]
    fn prefers_variants_with_their_sound_inside() {
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\",URI=\"audio/en.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,AUDIO=\"aud\"\nvideo-only.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2000000\nmuxed.m3u8\n";
        assert_eq!(best_variant(master, &base()).unwrap().as_str(), "https://cdn.example.com/v/muxed.m3u8");
        // Only separate-audio variants: still the best of them.
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",URI=\"a.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1,AUDIO=\"aud\"\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=9,AUDIO=\"aud\"\nhigh.m3u8\n";
        assert_eq!(best_variant(master, &base()).unwrap().as_str(), "https://cdn.example.com/v/high.m3u8");
    }

    #[test]
    fn reads_pieces_keys_and_ranges() {
        let text = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://keys.example.com/k?id=1,2\",IV=0x0000000000000000000000000000002A\n#EXTINF:4.0,\nseg1.m4s\n#EXT-X-KEY:METHOD=NONE\n#EXT-X-BYTERANGE:100@0\n#EXTINF:4.0,\nall.m4s\n#EXT-X-BYTERANGE:50\n#EXTINF:4.0,\nall.m4s\n#EXT-X-ENDLIST\n";
        let m = parse_media(text, &base()).unwrap();
        assert_eq!(m.init.unwrap().url.as_str(), "https://cdn.example.com/v/init.mp4");
        assert_eq!(m.pieces.len(), 3);
        let key = m.pieces[0].key.clone().unwrap();
        assert_eq!(key.uri.as_str(), "https://keys.example.com/k?id=1,2");
        assert_eq!(key.iv.unwrap()[15], 42);
        assert_eq!(m.pieces[0].seq, 7);
        assert!(m.pieces[1].key.is_none());
        assert_eq!(m.pieces[1].range, Some((0, 100)));
        assert_eq!(m.pieces[2].range, Some((100, 50)), "continues after the previous range");
    }

    #[test]
    fn refuses_live_and_protected_streams() {
        assert!(parse_media("#EXTM3U\n#EXTINF:4,\na.ts\n", &base()).is_err());
        let drm = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\n#EXTINF:4,\na.ts\n#EXT-X-ENDLIST\n";
        assert!(parse_media(drm, &base()).unwrap_err().to_string().contains("protected"));
    }

    #[test]
    fn decrypts_aes_128() {
        use cbc::cipher::block_padding::Pkcs7;
        use cbc::cipher::{BlockModeEncrypt, KeyIvInit};
        let (key, iv) = ([7u8; 16], [9u8; 16]);
        let plain = b"one piece of a video stream".to_vec();
        let enc = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(&plain);
        assert_eq!(decrypt(&key, &iv, &enc).unwrap(), plain);
        assert!(decrypt(&[1u8; 16], &iv, &enc).is_err() || decrypt(&[1u8; 16], &iv, &enc).unwrap() != plain);
    }
}
