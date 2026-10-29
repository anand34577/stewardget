//! Discovers size, name and range support with a single tiny request.
//!
//! A `GET` with `Range: bytes=0-0` is used instead of `HEAD`: many CDNs and
//! signed-URL services answer `HEAD` incorrectly (or not at all), while a
//! one-byte ranged `GET` tells us everything in one round-trip.

use reqwest::header::{self, HeaderMap};
use reqwest::{Client, StatusCode};
use url::Url;

use crate::{Error, filename};

#[derive(Debug, Clone)]
pub(crate) struct Probe {
    /// URL after redirects; segments go straight here.
    pub final_url: Url,
    pub total: Option<u64>,
    pub ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    /// Set for HLS streams: the extension the saved file gets (`ts`, `mp4`).
    pub stream_ext: Option<&'static str>,
}

pub(crate) async fn probe(client: &Client, url: &Url, headers: &HeaderMap) -> Result<Probe, Error> {
    if url.scheme() == "ftp" {
        return crate::ftp::probe(url).await;
    }
    let resp = client.get(url.clone()).headers(headers.clone()).header(header::RANGE, "bytes=0-0").send().await?;
    let status = resp.status();
    let h = resp.headers();
    let text = |name: header::HeaderName| h.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);

    let (total, ranges) = match status {
        StatusCode::PARTIAL_CONTENT => {
            let total = text(header::CONTENT_RANGE).as_deref().and_then(parse_content_range_total);
            (total, total.is_some())
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            // Typically an empty file: `Content-Range: bytes */0`.
            (text(header::CONTENT_RANGE).as_deref().and_then(parse_content_range_total), false)
        }
        s if s.is_success() => (resp.content_length(), false),
        _ => return Err(http_error(&resp)),
    };

    let etag = text(header::ETAG);
    let probe = Probe {
        final_url: resp.url().clone(),
        total,
        ranges,
        // Weak validators can't be used with If-Range and don't guarantee byte equality.
        etag: etag.filter(|e| !e.starts_with("W/")),
        last_modified: text(header::LAST_MODIFIED),
        // Many servers put raw UTF-8 (or Latin-1) into `filename="…"`, which
        // `to_str` rejects; read the bytes so those names aren't lost.
        file_name: h
            .get(header::CONTENT_DISPOSITION)
            .map(|v| header_text(v.as_bytes()))
            .as_deref()
            .and_then(filename::from_content_disposition),
        content_type: text(header::CONTENT_TYPE),
        stream_ext: None,
    };
    // Dropping `resp` without reading the body closes/reuses the connection.
    Ok(probe)
}

/// A header value as text: UTF-8 when it is valid UTF-8, otherwise Latin-1.
fn header_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// The error for an unsuccessful response. `429` and `503` with a
/// `Retry-After` (seconds or an HTTP date) become [`Error::Throttled`].
pub(crate) fn http_error(resp: &reqwest::Response) -> Error {
    let code = resp.status().as_u16();
    if matches!(code, 429 | 503)
        && let Some(secs) =
            resp.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(parse_retry_after)
    {
        return Error::Throttled { code, retry_after: secs };
    }
    Error::Http(code)
}

/// `120` or an HTTP date → seconds to wait (capped at five minutes).
pub(crate) fn parse_retry_after(value: &str) -> Option<u64> {
    let value = value.trim();
    let secs = match value.parse::<u64>() {
        Ok(s) => s,
        Err(_) => {
            let at = httpdate::parse_http_date(value).ok()?;
            at.duration_since(std::time::SystemTime::now()).map_or(0, |d| d.as_secs())
        }
    };
    Some(secs.min(300))
}

/// `bytes 0-0/12345` → `Some(12345)`; `bytes 0-0/*` → `None`.
pub(crate) fn parse_content_range_total(value: &str) -> Option<u64> {
    value.trim().rsplit_once('/')?.1.trim().parse().ok()
}

/// `bytes 100-199/1000` → `Some((100, 199))`.
pub(crate) fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes")?.trim();
    range.split_once('-')?.0.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range_total("bytes 0-0/12345"), Some(12345));
        assert_eq!(parse_content_range_total("bytes */0"), Some(0));
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None);
        assert_eq!(parse_content_range_start("bytes 100-199/1000"), Some(100));
    }

    #[test]
    fn non_ascii_disposition() {
        let raw = "attachment; filename=\"отчёт.pdf\"";
        assert_eq!(header_text(raw.as_bytes()), raw);
        assert_eq!(header_text(b"attachment; filename=\"caf\xe9.txt\""), "attachment; filename=\"café.txt\"");
    }

    #[test]
    fn retry_after() {
        assert_eq!(parse_retry_after("7"), Some(7));
        assert_eq!(parse_retry_after("100000"), Some(300));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), Some(0));
        assert_eq!(parse_retry_after("soon"), None);
    }
}
