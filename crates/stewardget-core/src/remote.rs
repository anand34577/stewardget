//! Talks to a StewardGet that is already running, through its local API (see
//! the `api` module). Used by `stewardget add` and when the desktop app is launched
//! a second time.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::Deserialize;

use crate::{Capture, CaptureAction, Config, DownloadId};

/// Why a running StewardGet could not take a request.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("browser integration is turned off, so StewardGet can't be reached from outside")]
    Disabled,
    #[error("StewardGet is not running (nothing answers on 127.0.0.1:{0})")]
    NotRunning(u16),
    #[error("StewardGet refused the request: {0}")]
    Refused(String),
    #[error("talking to StewardGet failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Queues downloads in the running StewardGet.
pub fn add(config: &Config, captures: &[Capture]) -> Result<Vec<(DownloadId, CaptureAction)>, RemoteError> {
    #[derive(Deserialize)]
    struct Added {
        id: DownloadId,
        action: CaptureAction,
    }
    let body = serde_json::json!({ "items": captures }).to_string();
    let reply = call(config, "POST", "/api/v1/downloads", &body)?;
    let added: Vec<Added> = serde_json::from_str(&reply).map_err(|e| RemoteError::Refused(e.to_string()))?;
    Ok(added.into_iter().map(|a| (a.id, a.action)).collect())
}

/// Asks the running StewardGet to bring its window forward. `Ok(false)` means it
/// runs without a window (the terminal dashboard).
pub fn show(config: &Config) -> Result<bool, RemoteError> {
    #[derive(Deserialize)]
    struct Shown {
        shown: bool,
    }
    let reply = call(config, "POST", "/api/v1/show", "{}")?;
    Ok(serde_json::from_str::<Shown>(&reply).is_ok_and(|s| s.shown))
}

/// Asks the running desktop app to quit and waits until it has, for up to
/// `wait`. `Ok(false)`: nothing was running, or it runs without a window.
pub fn quit(config: &Config, wait: Duration) -> Result<bool, RemoteError> {
    #[derive(Deserialize)]
    struct Quit {
        quitting: bool,
    }
    let reply = match call(config, "POST", "/api/v1/quit", "{}") {
        Err(RemoteError::NotRunning(_)) => return Ok(false),
        other => other?,
    };
    if !serde_json::from_str::<Quit>(&reply).is_ok_and(|q| q.quitting) {
        return Ok(false);
    }
    let deadline = std::time::Instant::now() + wait;
    while std::time::Instant::now() < deadline {
        match call(config, "GET", "/api/v1/ping", "") {
            Err(RemoteError::NotRunning(_)) => return Ok(true),
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    Err(RemoteError::Refused("StewardGet did not quit in time".into()))
}

/// Sends a request to the running StewardGet's local API and returns the reply body.
pub fn api(config: &Config, method: &str, path: &str, body: &str) -> Result<String, RemoteError> {
    call(config, method, path, body)
}

/// Sends one MCP (JSON-RPC) message to the running StewardGet. An empty string
/// means the message was a notification and has no reply.
pub fn mcp(config: &Config, message: &str) -> Result<String, RemoteError> {
    call(config, "POST", "/mcp", message)
}

fn call(config: &Config, method: &str, path: &str, body: &str) -> Result<String, RemoteError> {
    let mcp = path == "/mcp";
    if !mcp && !config.api_enabled {
        return Err(RemoteError::Disabled);
    }
    if mcp && !config.mcp_enabled {
        return Err(RemoteError::Refused(
            "MCP is switched off: turn it on in StewardGet's Settings › AI & automation".into(),
        ));
    }
    let port = config.api_port;
    let token =
        if mcp { config.mcp_token.as_ref() } else { config.api_token.as_ref().or(config.control_token.as_ref()) };
    let auth = token.map(|t| format!("authorization: Bearer {t}\r\n")).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nx-stewardget-client: cli\r\ncontent-type: application/json\r\n{auth}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| RemoteError::NotRunning(port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    if head.starts_with("HTTP/1.1 202") {
        return Ok(String::new());
    }
    if !head.starts_with("HTTP/1.1 200") {
        let reason = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| head.lines().next().unwrap_or("no answer").to_owned());
        return Err(RemoteError::Refused(reason));
    }
    Ok(body.to_owned())
}
