//! Model Context Protocol server, so AI assistants can manage downloads.
//!
//! It speaks JSON-RPC 2.0 over the local API (`POST /mcp`, the "streamable
//! HTTP" transport, answered with plain JSON) and is off unless switched on in
//! Settings. Calls need the MCP token shown there. The `stewardget mcp` command
//! offers the same tools over stdin/stdout for clients that start a program.
//!
//! The tools can add, inspect, pause, resume, restart and remove downloads and
//! change the speed limit. They cannot reach outside StewardGet: a download goes to
//! a folder below the download folder, settings are read-only, and no tool runs
//! programs or reads files.

use std::path::{Component, Path};

use serde_json::{Value, json};

use crate::engine::Engine;
use crate::{DownloadView, Error, NewDownload, Status};

const LATEST: &str = "2025-06-18";
const KNOWN: [&str; 3] = ["2024-11-05", "2025-03-26", LATEST];

/// Answers one JSON-RPC message. `None` for notifications, which get no reply.
pub async fn handle(engine: &Engine, message: Value) -> Option<Value> {
    if let Value::Array(batch) = message {
        let mut replies = Vec::new();
        for m in batch {
            if let Some(r) = Box::pin(handle(engine, m)).await {
                replies.push(r);
            }
        }
        return (!replies.is_empty()).then_some(Value::Array(replies));
    }
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // No id: a notification (`notifications/initialized`, …).
    let id = id?;
    let outcome = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(call(engine, &params)),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

fn initialize(params: &Value) -> Value {
    let wanted = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(LATEST);
    let version = if KNOWN.contains(&wanted) { wanted } else { LATEST };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "stewardget", "title": "StewardGet Download Manager", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Manage downloads in the user's StewardGet app: add links, check progress, pause, resume or remove."
    })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        "annotations": { "readOnlyHint": read_only, "openWorldHint": name == "add_download" },
    })
}

fn tools() -> Vec<Value> {
    let id = json!({ "id": { "type": "integer", "description": "Download id from list_downloads" } });
    vec![
        tool(
            "list_downloads",
            "List downloads with progress, speed and status.",
            json!({ "status": { "type": "string", "enum": ["queued", "connecting", "downloading", "verifying", "paused", "completed", "failed"], "description": "Only downloads in this state" } }),
            &[],
            true,
        ),
        tool("get_download", "Details of one download.", id.clone(), &["id"], true),
        tool(
            "add_download",
            "Start downloading an http(s) link.",
            json!({
                "url": { "type": "string" },
                "file_name": { "type": "string", "description": "Save under this name" },
                "folder": { "type": "string", "description": "Sub-folder of the download folder, e.g. \"Movies\"" },
                "connections": { "type": "integer", "minimum": 1, "maximum": 64 },
                "speed_limit_kib": { "type": "integer", "description": "KiB/s cap for this download; 0 = none" },
                "paused": { "type": "boolean", "description": "Add without starting" }
            }),
            &["url"],
            false,
        ),
        tool("pause_download", "Pause a download; its progress is kept.", id.clone(), &["id"], false),
        tool("resume_download", "Resume a paused or failed download.", id.clone(), &["id"], false),
        tool("restart_download", "Discard partial data and download again.", id.clone(), &["id"], false),
        tool(
            "remove_download",
            "Remove a download from the list.",
            json!({ "id": { "type": "integer" }, "delete_file": { "type": "boolean", "description": "Also delete the file from disk" } }),
            &["id"],
            false,
        ),
        tool("pause_all", "Pause every running or queued download.", json!({}), &[], false),
        tool("resume_all", "Resume every paused or failed download.", json!({}), &[], false),
        tool(
            "set_speed_limit",
            "Cap the total download speed.",
            json!({ "kib_per_sec": { "type": "integer", "description": "0 removes the cap" } }),
            &["kib_per_sec"],
            false,
        ),
        tool("get_status", "Overall speed, counts and lifetime totals.", json!({}), &[], true),
        tool("list_queues", "Download queues and their limits.", json!({}), &[], true),
    ]
}

fn call(engine: &Engine, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    match run(engine, name, &args) {
        Ok(value) => reply(&value, false),
        Err(message) => reply(&Value::String(message), true),
    }
}

fn reply(value: &Value, is_error: bool) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn id_of(args: &Value) -> Result<u64, String> {
    args.get("id").and_then(Value::as_u64).ok_or_else(|| "`id` (integer) is required".to_owned())
}

fn text_of<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn run(engine: &Engine, name: &str, args: &Value) -> Result<Value, String> {
    let done = |r: Result<(), Error>, what: &str| r.map(|()| json!(what)).map_err(|e| e.to_string());
    match name {
        "list_downloads" => {
            let wanted = text_of(args, "status").map(str::to_ascii_lowercase);
            let list: Vec<Value> = engine
                .snapshot()
                .downloads
                .iter()
                .filter(|d| wanted.as_deref().is_none_or(|w| d.status.label().eq_ignore_ascii_case(w)))
                .map(view)
                .collect();
            Ok(json!(list))
        }
        "get_download" => engine.download(id_of(args)?).map(|d| view(&d)).ok_or_else(|| "no such download".to_owned()),
        "add_download" => {
            let url = text_of(args, "url").ok_or("`url` is required")?;
            let mut new = NewDownload::new(url);
            new.paused = args.get("paused").and_then(Value::as_bool).unwrap_or(false);
            new.options.file_name = text_of(args, "file_name").map(str::to_owned);
            if let Some(folder) = text_of(args, "folder") {
                new.options.directory = Some(sub_folder(&engine.config().download_dir, folder)?);
            }
            new.options.connections = args.get("connections").and_then(Value::as_u64).map(|c| c.clamp(1, 64) as usize);
            new.options.speed_limit = args.get("speed_limit_kib").and_then(Value::as_u64).unwrap_or(0).saturating_mul(1024);
            let id = engine.add(new).map_err(|e| e.to_string())?;
            Ok(json!({ "id": id, "message": "added" }))
        }
        "pause_download" => done(engine.pause(id_of(args)?), "paused"),
        "resume_download" => done(engine.resume(id_of(args)?), "resumed"),
        "restart_download" => done(engine.restart(id_of(args)?), "restarted"),
        "remove_download" => {
            let delete = args.get("delete_file").and_then(Value::as_bool).unwrap_or(false);
            done(engine.remove(id_of(args)?, delete), "removed")
        }
        "pause_all" => {
            engine.pause_all();
            Ok(json!("paused everything"))
        }
        "resume_all" => {
            engine.resume_all();
            Ok(json!("resumed everything"))
        }
        "set_speed_limit" => {
            let kib = args.get("kib_per_sec").and_then(Value::as_u64).ok_or("`kib_per_sec` (integer) is required")?;
            engine.set_speed_limit(kib.saturating_mul(1024));
            Ok(json!(if kib == 0 { "speed limit removed".to_owned() } else { format!("limited to {kib} KiB/s") }))
        }
        "get_status" => {
            let s = engine.snapshot();
            Ok(json!({
                "speed_bytes_per_sec": s.speed,
                "speed_limit_bytes_per_sec": s.speed_limit,
                "active": s.count(Status::is_active),
                "queued": s.count(|st| st == Status::Queued),
                "paused": s.count(|st| st == Status::Paused),
                "completed_total": s.stats.completed,
                "failed_total": s.stats.failed,
                "downloaded_bytes_total": s.stats.total_bytes,
            }))
        }
        "list_queues" => Ok(json!(
            engine
                .config()
                .queues
                .iter()
                .map(|q| json!({ "id": q.id, "name": q.name, "max_concurrent": q.max_concurrent, "speed_limit": q.speed_limit }))
                .collect::<Vec<_>>()
        )),
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// A folder below the download folder: relative, no `..`.
fn sub_folder(root: &Path, folder: &str) -> Result<std::path::PathBuf, String> {
    let relative = Path::new(folder);
    if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("`folder` must be a plain sub-folder name such as \"Movies\"".to_owned());
    }
    Ok(root.join(relative))
}

fn view(d: &DownloadView) -> Value {
    json!({
        "id": d.id,
        "file_name": d.file_name,
        "status": d.status.label().to_ascii_lowercase(),
        "progress_percent": d.fraction().map(|f| (f * 1000.0).round() / 10.0),
        "downloaded_bytes": d.downloaded,
        "total_bytes": d.total,
        "speed_bytes_per_sec": d.speed.round(),
        "eta_secs": d.eta,
        "connections": d.connections,
        "resumable": d.resumable,
        "url": d.url,
        "saved_to": d.path.display().to_string(),
        "error": d.error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_stay_below_the_download_folder() {
        let root = Path::new("/dl");
        assert!(sub_folder(root, "Movies/2025").is_ok());
        assert!(sub_folder(root, "../etc").is_err());
        assert!(sub_folder(root, "/etc").is_err());
        assert!(sub_folder(root, "C:\\Windows").is_err() || cfg!(not(windows)));
    }

    #[tokio::test]
    async fn speaks_the_protocol() {
        let engine = Engine::start(crate::EngineOptions::in_memory()).unwrap();
        let init = handle(
            &engine,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } }),
        )
        .await
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert!(handle(&engine, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await.is_none());
        let list = handle(&engine, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await.unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "add_download"));
        let bad = handle(&engine, json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" })).await.unwrap();
        assert_eq!(bad["error"]["code"], -32601);
        let status = handle(
            &engine,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "get_download", "arguments": { "id": 99 } } }),
        )
        .await
        .unwrap();
        assert_eq!(status["result"]["isError"], true);
        engine.shutdown().await;
    }
}
