//! Local browser-integration API.
//!
//! A deliberately tiny HTTP/1.1 server bound to `127.0.0.1` only. The browser
//! extension posts captured downloads here.
//!
//! Security model:
//! * Loopback only – never reachable from the network.
//! * Requests carrying a web `Origin` (`http://`/`https://`) are rejected, so
//!   pages you visit can't queue downloads behind your back. Extensions send
//!   `chrome-extension://` / `moz-extension://` origins.
//! * The `Host` header must name the loopback address, which defeats DNS
//!   rebinding (a web page whose domain is made to resolve to 127.0.0.1 still
//!   sends its own host name).
//! * Everything except `ping` needs the custom `X-StewardGet-Client` header, which
//!   forces a CORS pre-flight that we never approve for web pages.
//! * An optional shared token (`Authorization: Bearer …`) can be required.
//!
//! Endpoints (JSON):
//! * `GET  /api/v1/ping`       – `{ "app": "stewardget", "version": "…" }`
//! * `GET  /api/v1/downloads`  – summary of all downloads
//! * `POST /api/v1/downloads`  – a [`Capture`] or `{ "items": [ … ] }`; returns
//!   `[{ "id", "action" }]`, where `action` is `added`, `refreshed` or `pending`
//!   (the desktop app asks the user first; `id` is then 0)
//! * `POST /api/v1/show`       – bring the desktop window forward; `{ "shown" }`
//! * `GET  /api/v1/downloads/{id}`, `POST …/{id}/pause|resume|restart`,
//!   `DELETE …/{id}[?delete_file=1]`
//! * `GET  /api/v1/stats`, `GET /api/v1/queues`, `POST /api/v1/pause-all|resume-all`,
//!   `POST /api/v1/speed-limit` (`{ "bytes_per_sec" }`)
//! * `POST /mcp`               – Model Context Protocol (JSON-RPC), see [`crate::mcp`]
//! * `POST /api/v1/quit`       – close the desktop app (used by the installer); `{ "quitting" }`
//!
//! The extension identifies itself with `X-StewardGet-Browser` and
//! `X-StewardGet-Extension-Version`, so the app can show that it is connected.
//! An extension from before 0.7, when the app was called Velox, sends the same
//! headers named `X-Velox-…`; they are accepted as well.

use std::sync::Weak;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::engine::{Engine, Inner};
use crate::{Capture, Status};

const MAX_HEADER: usize = 16 * 1024;
/// Room for "download all links" from a big page, cookies included.
const MAX_BODY: usize = 8 * 1024 * 1024;

pub(crate) async fn serve(listener: TcpListener, inner: Weak<Inner>, stop: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            _ = stop.cancelled() => return,
            a = listener.accept() => a,
        };
        let Ok((stream, _)) = accepted else { continue };
        let inner = inner.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(10), handle(stream, inner)).await;
        });
    }
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// One of the app's own `x-stewardget-…` headers, or its pre-0.7 `x-velox-…` name.
    fn own_header(&self, name: &str) -> Option<&str> {
        self.header(&format!("x-stewardget-{name}")).or_else(|| self.header(&format!("x-velox-{name}")))
    }

    /// Whether the query string has `name` set to something other than `0`/`false`.
    fn flag(&self, name: &str) -> bool {
        self.query
            .split('&')
            .filter_map(|p| p.split_once('=').or(Some((p, "1"))))
            .any(|(k, v)| k == name && v != "0" && v != "false")
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER {
            return None;
        }
    };
    let head = std::str::from_utf8(&buf[..header_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.to_owned(), query.to_owned());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return None;
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    Some(Request { method, path, query, headers, body })
}

async fn respond(stream: &mut TcpStream, status: u16, body: &impl Serialize) {
    // 204 must not carry a body.
    let json = if status == 204 { Vec::new() } else { serde_json::to_vec(body).unwrap_or_default() };
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
        json.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&json).await;
    let _ = stream.shutdown().await;
}

#[derive(Serialize)]
struct Message<'a> {
    error: &'a str,
}

#[derive(Serialize)]
struct Summary {
    id: u64,
    file_name: String,
    status: Status,
    total: Option<u64>,
    downloaded: u64,
    speed: f64,
    url: String,
    path: String,
    category: crate::Category,
    queue: crate::QueueId,
    eta: Option<u64>,
    error: Option<String>,
}

impl From<crate::DownloadView> for Summary {
    fn from(d: crate::DownloadView) -> Self {
        Self {
            id: d.id,
            file_name: d.file_name,
            status: d.status,
            total: d.total,
            downloaded: d.downloaded,
            speed: d.speed,
            url: d.url,
            path: d.path.display().to_string(),
            category: d.category,
            queue: d.queue,
            eta: d.eta,
            error: d.error,
        }
    }
}

/// Accepts either one capture or `{ "items": [ … ] }`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CaptureBody {
    Many { items: Vec<Capture> },
    One(Capture),
}

async fn handle(mut stream: TcpStream, inner: Weak<Inner>) {
    let Some(req) = read_request(&mut stream).await else { return };

    // Web pages must never drive the API.
    if req.header("origin").is_some_and(|o| o.starts_with("http://") || o.starts_with("https://")) {
        return respond(&mut stream, 403, &Message { error: "web origins are not allowed" }).await;
    }
    if !req.header("host").is_some_and(is_loopback_host) {
        return respond(&mut stream, 403, &Message { error: "only 127.0.0.1 is accepted as host" }).await;
    }
    let Some(inner) = inner.upgrade() else { return };
    let engine = Engine::from_inner(inner);
    let config = engine.config();

    // MCP clients are programs, not browser extensions: they authenticate with
    // their own token and don't send `X-StewardGet-Client`.
    if req.path == "/mcp" {
        if !config.mcp_enabled {
            return respond(&mut stream, 404, &Message { error: "MCP is switched off in StewardGet's settings" }).await;
        }
        let sent = req.header("authorization").and_then(|v| v.strip_prefix("Bearer "));
        let ok = matches!((sent, config.mcp_token.as_deref()), (Some(a), Some(b)) if crate::util::secrets_equal(a, b));
        if !ok {
            return respond(&mut stream, 401, &Message { error: "missing or wrong token" }).await;
        }
        return match req.method.as_str() {
            "POST" => match serde_json::from_slice::<serde_json::Value>(&req.body) {
                Ok(message) => match crate::mcp::handle(&engine, message).await {
                    Some(reply) => respond(&mut stream, 200, &reply).await,
                    None => respond_empty(&mut stream, 202).await,
                },
                Err(e) => respond(&mut stream, 400, &Message { error: &e.to_string() }).await,
            },
            _ => respond(&mut stream, 405, &Message { error: "POST JSON-RPC messages to /mcp" }).await,
        };
    }
    if !config.api_enabled {
        return respond(&mut stream, 404, &Message { error: "the browser API is switched off" }).await;
    }
    if req.path != "/api/v1/ping" && req.own_header("client").is_none() {
        return respond(&mut stream, 403, &Message { error: "missing X-StewardGet-Client header" }).await;
    }
    if req.method == "OPTIONS" {
        // Deliberately no CORS allowance.
        return respond(&mut stream, 204, &()).await;
    }
    // What the browser extension needs works without a token (unless the user
    // set one); everything else is for the command line, the installer and
    // scripts, which hold the control token or the user's API token.
    let open = matches!(
        (req.method.as_str(), req.path.as_str()),
        ("GET", "/api/v1/ping") | ("POST", "/api/v1/downloads") | ("POST", "/api/v1/show")
    );
    let bearer = req.header("authorization").and_then(|v| v.strip_prefix("Bearer "));
    let holds = |token: &Option<String>| match (bearer, token) {
        (Some(sent), Some(expected)) => crate::util::secrets_equal(sent, expected),
        _ => false,
    };
    let trusted = holds(&config.api_token) || holds(&config.control_token);
    let allowed = match (&config.api_token, open) {
        (Some(_), _) => trusted,
        (None, true) => true,
        (None, false) => trusted,
    };
    if !allowed {
        let why =
            if bearer.is_some() { "missing or wrong token" } else { "this needs a token (Authorization: Bearer …)" };
        return respond(&mut stream, 401, &Message { error: why }).await;
    }
    if req.own_header("client") == Some("extension") {
        let browser = clean(req.own_header("browser").unwrap_or("Browser"));
        let version = clean(req.own_header("extension-version").unwrap_or(""));
        engine.extension_seen(if browser.is_empty() { "Browser" } else { &browser }, &version);
    }

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/ping") => {
            #[derive(Serialize)]
            struct Ping {
                app: &'static str,
                version: &'static str,
            }
            respond(&mut stream, 200, &Ping { app: "stewardget", version: env!("CARGO_PKG_VERSION") }).await;
        }
        ("GET", "/api/v1/downloads") => {
            let list: Vec<Summary> = engine.snapshot().downloads.into_iter().map(Summary::from).collect();
            respond(&mut stream, 200, &list).await;
        }
        ("POST", "/api/v1/downloads") => {
            let captures = match serde_json::from_slice::<CaptureBody>(&req.body) {
                Ok(CaptureBody::Many { items }) => items,
                Ok(CaptureBody::One(c)) => vec![c],
                Err(e) => return respond(&mut stream, 400, &Message { error: &e.to_string() }).await,
            };
            #[derive(Serialize)]
            struct Added {
                id: u64,
                action: crate::CaptureAction,
            }
            // Where files may go is the user's call, not a client's: only a
            // token holder (the command line, scripts) can pick the folder.
            let captures: Vec<Capture> = if trusted {
                captures
            } else {
                captures.into_iter().map(|c| Capture { directory: None, ..c }).collect()
            };
            match engine.capture_many(captures) {
                Ok(results) => {
                    let results: Vec<Added> = results.into_iter().map(|(id, action)| Added { id, action }).collect();
                    respond(&mut stream, 200, &results).await;
                }
                Err(e) => respond(&mut stream, 400, &Message { error: &e.to_string() }).await,
            }
        }
        ("POST", "/api/v1/quit") => {
            #[derive(Serialize)]
            struct Quit {
                quitting: bool,
            }
            respond(&mut stream, 200, &Quit { quitting: engine.request_quit() }).await;
        }
        ("POST", "/api/v1/show") => {
            #[derive(Serialize)]
            struct Shown {
                shown: bool,
            }
            respond(&mut stream, 200, &Shown { shown: engine.request_show() }).await;
        }
        ("GET", "/api/v1/stats") => {
            let snap = engine.snapshot();
            respond(
                &mut stream,
                200,
                &serde_json::json!({
                    "speed": snap.speed,
                    "speed_limit": snap.speed_limit,
                    "active": snap.count(Status::is_active),
                    "queued": snap.count(|s| s == Status::Queued),
                    "paused": snap.count(|s| s == Status::Paused),
                    "completed": snap.stats.completed,
                    "failed": snap.stats.failed,
                    "total_bytes": snap.stats.total_bytes,
                }),
            )
            .await;
        }
        ("GET", "/api/v1/queues") => {
            let queues: Vec<_> = config
                .queues
                .iter()
                .map(|q| {
                    serde_json::json!({
                        "id": q.id, "name": q.name, "max_concurrent": q.max_concurrent, "speed_limit": q.speed_limit
                    })
                })
                .collect();
            respond(&mut stream, 200, &queues).await;
        }
        ("POST", "/api/v1/pause-all") => {
            engine.pause_all();
            respond(&mut stream, 200, &Message { error: "" }).await;
        }
        ("POST", "/api/v1/resume-all") => {
            engine.resume_all();
            respond(&mut stream, 200, &Message { error: "" }).await;
        }
        ("POST", "/api/v1/speed-limit") => {
            #[derive(serde::Deserialize)]
            struct Limit {
                bytes_per_sec: u64,
            }
            match serde_json::from_slice::<Limit>(&req.body) {
                Ok(l) => {
                    engine.set_speed_limit(l.bytes_per_sec);
                    respond(&mut stream, 200, &Message { error: "" }).await;
                }
                Err(e) => respond(&mut stream, 400, &Message { error: &e.to_string() }).await,
            }
        }
        (method, path) if path.starts_with("/api/v1/downloads/") => {
            let rest = &path["/api/v1/downloads/".len()..];
            let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
            let Ok(id) = id.parse::<u64>() else {
                return respond(&mut stream, 400, &Message { error: "bad download id" }).await;
            };
            let result = match (method, action) {
                ("GET", "") => match engine.download(id) {
                    Some(d) => return respond(&mut stream, 200, &Summary::from(d)).await,
                    None => Err(crate::Error::NotFound),
                },
                ("POST", "pause") => engine.pause(id),
                ("POST", "resume") => engine.resume(id),
                ("POST", "restart") => engine.restart(id),
                ("DELETE", "") => engine.remove(id, req.flag("delete_file")),
                _ => return respond(&mut stream, 404, &Message { error: "not found" }).await,
            };
            match result {
                Ok(()) => respond(&mut stream, 200, &Message { error: "" }).await,
                Err(crate::Error::NotFound) => {
                    respond(&mut stream, 404, &Message { error: "download not found" }).await
                }
                Err(e) => respond(&mut stream, 400, &Message { error: &e.to_string() }).await,
            }
        }
        _ => respond(&mut stream, 404, &Message { error: "not found" }).await,
    }
}

async fn respond_empty(stream: &mut TcpStream, status: u16) {
    let head = format!("HTTP/1.1 {status} Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Keeps a client-supplied label short and printable.
fn clean(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(32).collect::<String>().trim().to_owned()
}

/// `127.0.0.1`, `localhost` or `[::1]`, with or without a port.
fn is_loopback_host(host: &str) -> bool {
    let name = host
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(host, |(name, _)| name);
    matches!(name.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::is_loopback_host;

    #[test]
    fn loopback_hosts() {
        for ok in ["127.0.0.1", "127.0.0.1:7853", "localhost:7853", "LOCALHOST", "[::1]", "[::1]:7853"] {
            assert!(is_loopback_host(ok), "{ok}");
        }
        for bad in ["evil.example", "evil.example:7853", "127.0.0.1.evil.example", "localhost.evil:80", "", ":7853"] {
            assert!(!is_loopback_host(bad), "{bad}");
        }
    }
}
