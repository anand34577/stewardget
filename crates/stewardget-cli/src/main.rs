//! `stewardget` — command line and terminal UI front-end.

mod get;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use stewardget_core::util::{format_bytes, format_duration, format_speed, format_timestamp};
use stewardget_core::{EngineOptions, Status};

/// StewardGet — a fast, modern, privacy-respecting download manager.
#[derive(Parser)]
#[command(name = "stewardget", version, about, long_about = None, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Download URLs right now with live progress (resumable: re-run to continue).
    #[command(visible_alias = "dl")]
    Get(Box<GetArgs>),
    /// Open the interactive terminal dashboard (the default).
    Tui,
    /// Queue URLs in the running StewardGet app (GUI or TUI).
    Add {
        /// One or more URLs.
        #[arg(required = true)]
        urls: Vec<String>,
        /// Page the links came from (sent as Referer).
        #[arg(long)]
        referrer: Option<String>,
        /// Queue to add them to, by name [default: the main queue].
        #[arg(short, long, value_name = "NAME")]
        queue: Option<String>,
        /// Add them paused, to start later (or with the queue's schedule).
        #[arg(long)]
        paused: bool,
        /// Show them in the desktop app's new-download window instead of adding them directly.
        #[arg(long)]
        ask: bool,
    },
    /// List the download queues.
    Queues,
    /// Show the downloads managed by the TUI/GUI.
    #[command(visible_alias = "ls")]
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Show lifetime statistics.
    Stats,
    /// Show where settings live and what they are.
    Config,
    /// Pause a download in the running app (`all` for every download).
    Pause { target: String },
    /// Resume (or retry) a download in the running app (`all` for every one).
    #[command(visible_alias = "retry")]
    Resume { target: String },
    /// Discard a download's partial data and start it again.
    Restart { id: u64 },
    /// Remove a download from the running app.
    #[command(visible_alias = "rm")]
    Remove {
        id: u64,
        /// Also delete the file from disk.
        #[arg(long)]
        delete_file: bool,
    },
    /// Cap the total speed of the running app, e.g. `2M`; `0` removes the cap.
    Limit {
        #[arg(value_parser = parse_rate)]
        rate: u64,
    },
    /// Run an MCP server on stdin/stdout so an AI assistant can drive the
    /// running app (turn MCP on in Settings first).
    Mcp {
        /// Print the settings to paste into an MCP client instead.
        #[arg(long)]
        config: bool,
    },
    /// Find the files linked from a web page (and optionally the pages it links to).
    Grab {
        url: String,
        /// How many levels of links to follow from the page.
        #[arg(short, long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=3))]
        depth: u8,
        /// Only these extensions, comma separated (default: every known kind of file).
        #[arg(short, long, value_delimiter = ',')]
        ext: Vec<String>,
        /// Also follow links to other websites.
        #[arg(long)]
        other_sites: bool,
        /// Add every file found to the running app.
        #[arg(long)]
        add: bool,
    },
    /// Print a shell completion script.
    Completions { shell: clap_complete::Shell },
    /// Set up the browser extension: unpacks it and prints the steps.
    Extension {
        /// Also open the folder in the file manager.
        #[arg(long)]
        open: bool,
    },
}

#[derive(Args)]
pub struct GetArgs {
    /// One or more URLs.
    #[arg(required_unless_present = "input")]
    pub urls: Vec<String>,
    /// Read additional URLs from a file (one per line, `#` comments allowed).
    #[arg(short, long, value_name = "FILE")]
    pub input: Option<PathBuf>,
    /// Destination directory [default: current directory].
    #[arg(short, long, value_name = "DIR")]
    pub dir: Option<PathBuf>,
    /// Output file name (only with a single URL).
    #[arg(short, long, value_name = "NAME")]
    pub output: Option<String>,
    /// Connections per download.
    #[arg(short, long, value_name = "N")]
    pub connections: Option<usize>,
    /// Downloads to run at the same time.
    #[arg(short, long, value_name = "N")]
    pub jobs: Option<usize>,
    /// Global speed limit, e.g. `500k`, `2M`.
    #[arg(short, long, value_name = "RATE", value_parser = parse_rate)]
    pub limit: Option<u64>,
    /// Expected checksum: `sha256:<hex>` (also md5, sha1, sha512).
    #[arg(long, value_name = "ALGO:HEX")]
    pub checksum: Option<stewardget_core::Checksum>,
    /// Extra request header, e.g. `-H "Cookie: a=b"`. Repeatable.
    #[arg(short = 'H', long = "header", value_name = "HEADER", value_parser = parse_header)]
    pub headers: Vec<(String, String)>,
    /// Custom User-Agent.
    #[arg(short = 'U', long, value_name = "UA")]
    pub user_agent: Option<String>,
    /// Proxy URL (http, https, socks5).
    #[arg(long, value_name = "URL")]
    pub proxy: Option<String>,
    /// Accept invalid TLS certificates (dangerous).
    #[arg(long)]
    pub insecure: bool,
    /// Only print errors and the final summary.
    #[arg(short, long)]
    pub quiet: bool,
}

fn parse_rate(s: &str) -> Result<u64, String> {
    stewardget_core::util::parse_size(s).ok_or_else(|| format!("`{s}` is not a rate like 500k or 2M"))
}

fn parse_header(s: &str) -> Result<(String, String), String> {
    let (k, v) = s.split_once(':').ok_or("expected `Name: value`")?;
    Ok((k.trim().to_owned(), v.trim().to_owned()))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Command::Tui) {
        Command::Get(args) => runtime().and_then(|rt| rt.block_on(get::run(*args))),
        Command::Tui => runtime().and_then(|rt| tui::run(&rt)).map(|()| ExitCode::SUCCESS),
        Command::Add { urls, referrer, queue, paused, ask } => {
            add(&urls, referrer.as_deref(), queue.as_deref(), paused, ask).map(|()| ExitCode::SUCCESS)
        }
        Command::Queues => queues().map(|()| ExitCode::SUCCESS),
        Command::List { json } => list(json).map(|()| ExitCode::SUCCESS),
        Command::Pause { target } => control("pause", &target).map(|()| ExitCode::SUCCESS),
        Command::Resume { target } => control("resume", &target).map(|()| ExitCode::SUCCESS),
        Command::Restart { id } => {
            api_call("POST", &format!("/api/v1/downloads/{id}/restart"), "").map(|_| ExitCode::SUCCESS)
        }
        Command::Remove { id, delete_file } => {
            let path = format!("/api/v1/downloads/{id}?delete_file={}", u8::from(delete_file));
            api_call("DELETE", &path, "").map(|_| ExitCode::SUCCESS)
        }
        Command::Limit { rate } => {
            api_call("POST", "/api/v1/speed-limit", &serde_json::json!({ "bytes_per_sec": rate }).to_string())
                .map(|_| ExitCode::SUCCESS)
        }
        Command::Grab { url, depth, ext, other_sites, add } => {
            runtime().and_then(|rt| rt.block_on(grab(&url, depth, ext, other_sites, add))).map(|()| ExitCode::SUCCESS)
        }
        Command::Mcp { config } => mcp(config).map(|()| ExitCode::SUCCESS),
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut <Cli as clap::CommandFactory>::command(),
                "stewardget",
                &mut std::io::stdout(),
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Stats => stats().map(|()| ExitCode::SUCCESS),
        Command::Config => config().map(|()| ExitCode::SUCCESS),
        Command::Extension { open } => extension(open).map(|()| ExitCode::SUCCESS),
    };
    result.unwrap_or_else(|e| {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        ExitCode::FAILURE
    })
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().context("failed to start async runtime")
}

/// Queues URLs in the running StewardGet through its local API.
fn add(urls: &[String], referrer: Option<&str>, queue: Option<&str>, paused: bool, ask: bool) -> anyhow::Result<()> {
    let config = EngineOptions::peek_config(&EngineOptions::default_data_dir()).unwrap_or_default().sanitized();
    let queue = match queue {
        None => None,
        Some(name) => Some(
            config.queues.iter().find(|q| q.name.eq_ignore_ascii_case(name.trim())).map(|q| q.id).ok_or_else(|| {
                let names: Vec<_> = config.queues.iter().map(|q| q.name.as_str()).collect();
                anyhow::anyhow!("there is no queue named `{name}`; the queues are: {}", names.join(", "))
            })?,
        ),
    };
    let items: Vec<_> = urls
        .iter()
        .map(|url| stewardget_core::Capture {
            url: url.clone(),
            referrer: referrer.map(str::to_owned),
            paused,
            queue,
            direct: !ask,
            ..Default::default()
        })
        .collect();
    let results = stewardget_core::remote::add(&config, &items).map_err(|e| match e {
        stewardget_core::remote::RemoteError::NotRunning(_) => {
            anyhow::anyhow!("{e}. Start `stewardget` or `stewardget-gui`, or download directly with `stewardget get`.")
        }
        e => e.into(),
    })?;
    for (url, (id, action)) in urls.iter().zip(results) {
        match action {
            stewardget_core::CaptureAction::Pending => {
                println!("\x1b[33masking\x1b[0m     {url}  (confirm in the StewardGet window)")
            }
            stewardget_core::CaptureAction::Refreshed => println!("\x1b[32mrefreshed\x1b[0m  #{id}  {url}"),
            stewardget_core::CaptureAction::Added => println!("\x1b[32madded\x1b[0m      #{id}  {url}"),
        }
    }
    Ok(())
}

fn queues() -> anyhow::Result<()> {
    let dir = EngineOptions::default_data_dir();
    let config = EngineOptions::peek_config(&dir).unwrap_or_default().sanitized();
    let downloads = EngineOptions::peek_downloads(&dir);
    println!("{:<24} {:>9}  {:>11}  SCHEDULE", "QUEUE", "DOWNLOADS", "AT ONCE");
    for q in &config.queues {
        let count = downloads.iter().filter(|d| config.queue(d.queue).id == q.id).count();
        let at_once = if q.max_concurrent == 0 { "any".to_owned() } else { q.max_concurrent.to_string() };
        let s = &q.schedule;
        let schedule = if s.enabled {
            let hhmm = |m: u16| format!("{:02}:{:02}", m / 60, m % 60);
            let stop = s.stop_minute.map_or(String::new(), |m| format!("–{}", hhmm(m)));
            format!("{}{stop}", hhmm(s.start_minute))
        } else {
            "off".to_owned()
        };
        println!("{:<24} {:>9}  {:>11}  {schedule}", q.name, count, at_once);
    }
    Ok(())
}

async fn grab(url: &str, depth: u8, ext: Vec<String>, other_sites: bool, add: bool) -> anyhow::Result<()> {
    let config = running_config();
    let engine = stewardget_core::Engine::start(EngineOptions::in_memory().with_config(config.clone()))?;
    let options = stewardget_core::GrabOptions {
        depth,
        other_sites,
        extensions: stewardget_core::CustomCategory::parse_extensions(&ext.join(",")),
    };
    let found = engine.grab_links(url, &options).await?;
    for (file, _) in &found.files {
        println!("{file}");
    }
    eprintln!(
        "{} file(s) on {} page(s){}{}",
        found.files.len(),
        found.pages,
        if found.blocked_by_robots > 0 {
            format!(", {} page(s) skipped by robots.txt", found.blocked_by_robots)
        } else {
            String::new()
        },
        if found.truncated { "; stopped at the search limit" } else { "" },
    );
    if add && !found.files.is_empty() {
        let items: Vec<_> = found
            .files
            .iter()
            .map(|(file, page)| stewardget_core::Capture {
                url: file.clone(),
                referrer: Some(page.clone()),
                direct: true,
                ..Default::default()
            })
            .collect();
        stewardget_core::remote::add(&config, &items)?;
        eprintln!("added to StewardGet");
    }
    engine.shutdown().await;
    Ok(())
}

fn running_config() -> stewardget_core::Config {
    EngineOptions::peek_config(&EngineOptions::default_data_dir()).unwrap_or_default().sanitized()
}

/// One request to the running app; the reply body.
fn api_call(method: &str, path: &str, body: &str) -> anyhow::Result<String> {
    stewardget_core::remote::api(&running_config(), method, path, body).map_err(|e| match e {
        stewardget_core::remote::RemoteError::NotRunning(_) => {
            anyhow::anyhow!("{e}. Start `stewardget` or `stewardget-gui` first.")
        }
        e => e.into(),
    })
}

/// `stewardget pause|resume <id|all>`.
fn control(action: &str, target: &str) -> anyhow::Result<()> {
    if target.eq_ignore_ascii_case("all") {
        api_call("POST", &format!("/api/v1/{action}-all"), "{}")?;
    } else {
        let id: u64 = target.parse().context("expected a download id (see `stewardget list`) or `all`")?;
        api_call("POST", &format!("/api/v1/downloads/{id}/{action}"), "{}")?;
    }
    println!("{action}d {target}");
    Ok(())
}

/// Bridges an MCP client on stdin/stdout to the running app.
fn mcp(print_config: bool) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    let config = running_config();
    if print_config {
        let exe = std::env::current_exe()?;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": { "stewardget": { "command": exe.display().to_string(), "args": ["mcp"] } }
            }))?
        );
        if let (true, Some(token)) = (config.mcp_enabled, &config.mcp_token) {
            eprintln!(
                "
Or, for clients that speak HTTP: http://127.0.0.1:{}/mcp  with header  Authorization: Bearer {token}",
                config.api_port
            );
        } else {
            eprintln!(
                "
MCP is switched off. Turn it on in StewardGet's Settings › AI & automation first."
            );
        }
        return Ok(());
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        // Windows shells like to start piped text with a byte-order mark.
        let line = line.trim_start_matches('\u{feff}').to_owned();
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|v| v.get("id").cloned());
        match stewardget_core::remote::mcp(&config, &line) {
            Ok(reply) if !reply.is_empty() => writeln!(out, "{}", reply.trim())?,
            Ok(_) => {}
            Err(e) => {
                if let Some(id) = id {
                    let error = serde_json::json!({
                        "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e.to_string() }
                    });
                    writeln!(out, "{error}")?;
                }
            }
        }
        out.flush()?;
    }
    Ok(())
}

fn list(json: bool) -> anyhow::Result<()> {
    if json {
        // Live data from the running app; the saved list when it is closed.
        let live = stewardget_core::remote::api(&running_config(), "GET", "/api/v1/downloads", "");
        let text = match live {
            Ok(text) => text,
            Err(_) => serde_json::to_string(
                &EngineOptions::peek_downloads(&EngineOptions::default_data_dir())
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "id": d.id, "file_name": d.file_name, "status": d.status, "total": d.total,
                            "downloaded": d.downloaded, "url": d.url, "path": d.path.display().to_string(),
                            "error": d.error,
                        })
                    })
                    .collect::<Vec<_>>(),
            )?,
        };
        println!("{text}");
        return Ok(());
    }
    let downloads = EngineOptions::peek_downloads(&EngineOptions::default_data_dir());
    if downloads.is_empty() {
        println!(
            "No downloads yet. Start the dashboard with `stewardget` or download directly with `stewardget get <URL>`."
        );
        return Ok(());
    }
    println!("{:>4}  {:<11} {:>10}  {:>6}  NAME", "ID", "STATUS", "SIZE", "DONE");
    for d in downloads {
        let pct = d.fraction().map_or("—".into(), |f| format!("{:.0}%", f * 100.0));
        let size = d.total.map_or("?".into(), format_bytes);
        let color = match d.status {
            Status::Completed => "32",
            Status::Failed => "31",
            Status::Paused => "33",
            _ => "36",
        };
        println!("{:>4}  \x1b[{color}m{:<11}\x1b[0m {:>10}  {:>6}  {}", d.id, d.status.label(), size, pct, d.file_name);
    }
    Ok(())
}

fn stats() -> anyhow::Result<()> {
    let s = EngineOptions::peek_stats(&EngineOptions::default_data_dir()).unwrap_or_default();
    let now = stewardget_core::util::now_unix();
    println!("\x1b[1mStewardGet statistics\x1b[0m  (stored locally, never shared)\n");
    println!("  Downloaded      {}", format_bytes(s.total_bytes));
    println!("  Today           {}", format_bytes(s.today(now)));
    println!("  Completed       {}", s.completed);
    println!("  Failed          {}", s.failed);
    if let Some(rate) = s.success_rate() {
        println!("  Success rate    {:.1}%", rate * 100.0);
    }
    println!("  Peak speed      {}", format_speed(s.peak_speed));
    println!("  Average speed   {}", format_speed(s.average_speed()));
    println!("  Time active     {}", format_duration(s.active_secs as u64));
    if s.first_seen > 0 {
        println!("  Since           {}", format_timestamp(s.first_seen));
    }
    let days = s.recent_days(14, now);
    let max = days.iter().map(|d| d.bytes).max().unwrap_or(0).max(1);
    println!("\n  Last 14 days");
    for d in days {
        let width = (d.bytes as f64 / max as f64 * 40.0).round() as usize;
        println!("  {}  \x1b[36m{:<40}\x1b[0m {}", d.label(), "█".repeat(width), format_bytes(d.bytes));
    }
    Ok(())
}

fn config() -> anyhow::Result<()> {
    let dir = EngineOptions::default_data_dir();
    let mut config = EngineOptions::peek_config(&dir).unwrap_or_default();
    // This output gets pasted into bug reports: keep the secrets out of it.
    for secret in [&mut config.control_token, &mut config.api_token, &mut config.mcp_token] {
        if secret.is_some() {
            *secret = Some("<hidden>".into());
        }
    }
    if config.proxy.as_deref().is_some_and(|p| p.contains('@')) {
        config.proxy = Some("<hidden: contains credentials>".into());
    }
    println!("Data directory: {}", dir.display());
    println!("Settings file:  {}\n", dir.join("settings.json").display());
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

fn extension(open: bool) -> anyhow::Result<()> {
    let dir = stewardget_core::extension::unpack(&EngineOptions::default_data_dir())
        .context("could not write the extension into the StewardGet data folder")?;
    stewardget_core::extension::refresh_legacy_copy();
    println!("\x1b[1mStewardGet browser extension\x1b[0m\n");
    let firefox = stewardget_core::extension::firefox_folder(&dir);
    println!("Chrome, Edge, Brave, Opera, Vivaldi:");
    println!("  1. Open chrome://extensions (edge://extensions, brave://extensions, …)");
    println!("  2. Turn on Developer mode");
    println!("  3. Click \"Load unpacked\" and choose this folder:");
    println!("     {}\n", dir.display());
    println!("Firefox:");
    println!("  Install stewardget-firefox.xpi from the release page when it has one, or open");
    println!("  about:debugging#/runtime/this-firefox, click \"Load Temporary Add-on\" and");
    println!("  choose manifest.json in this folder (lasts until Firefox restarts):");
    println!("     {}\n", firefox.display());
    println!("Keep StewardGet running (`stewardget` or `stewardget-gui`) to receive downloads.");
    if open {
        stewardget_core::util::open_with_system(&dir).context("could not open the folder")?;
    }
    Ok(())
}
