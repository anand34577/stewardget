//! `stewardget get`: one-shot downloads with live progress bars.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use stewardget_core::util::{format_bytes, format_duration, format_speed};
use stewardget_core::{Config, DownloadOptions, Engine, EngineOptions, NewDownload, Status};

use crate::GetArgs;

pub async fn run(args: GetArgs) -> anyhow::Result<ExitCode> {
    let mut urls = args.urls.clone();
    if let Some(path) = &args.input {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        urls.extend(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned));
    }
    if urls.is_empty() {
        bail!("no URLs given");
    }
    if args.output.is_some() && urls.len() > 1 {
        bail!("--output can only be used with a single URL");
    }

    // Start from the user's saved settings, then apply flags.
    let base = EngineOptions::peek_config(&EngineOptions::default_data_dir()).unwrap_or_default();
    let config = Config {
        download_dir: match args.dir.clone() {
            Some(d) => d,
            None => std::env::current_dir()?,
        },
        auto_categorize: false,
        connections: args.connections.unwrap_or(base.connections),
        max_concurrent: args.jobs.unwrap_or(base.max_concurrent.max(urls.len().min(4))),
        speed_limit: args.limit.unwrap_or(0),
        user_agent: args.user_agent.clone().unwrap_or(base.user_agent.clone()),
        proxy: args.proxy.clone().or(base.proxy.clone()),
        insecure_tls: args.insecure,
        ..base
    };
    let engine = Engine::start(EngineOptions::in_memory().with_config(config))?;

    let mut ids = Vec::new();
    for url in &urls {
        let options = DownloadOptions {
            file_name: args.output.clone(),
            headers: args.headers.clone(),
            checksum: args.checksum.clone(),
            ..Default::default()
        };
        match engine.add(NewDownload { url: url.clone(), options, ..Default::default() }) {
            Ok(id) => ids.push(id),
            Err(e) => eprintln!("\x1b[31m✗\x1b[0m {url}: {e}"),
        }
    }
    if ids.is_empty() {
        return Ok(ExitCode::FAILURE);
    }

    let started = Instant::now();
    let multi = MultiProgress::with_draw_target(if args.quiet {
        ProgressDrawTarget::hidden()
    } else {
        ProgressDrawTarget::stderr_with_hz(10)
    });
    let style = ProgressStyle::with_template("{prefix:.bold} {wide_bar:.cyan/238} {msg}")
        .expect("valid template")
        .progress_chars("━━─");
    let mut bars: HashMap<u64, ProgressBar> = HashMap::new();
    for &id in &ids {
        let bar = multi.add(ProgressBar::new(0).with_style(style.clone()));
        bars.insert(id, bar);
    }

    let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let interrupted = loop {
        tokio::select! {
            _ = &mut ctrl_c => break true,
            _ = tick.tick() => {}
        }
        let snap = engine.snapshot();
        let mut all_done = true;
        for d in snap.downloads.iter().filter(|d| bars.contains_key(&d.id)) {
            let bar = &bars[&d.id];
            if bar.is_finished() {
                continue;
            }
            bar.set_prefix(truncate(&d.file_name, 28));
            bar.set_length(d.total.unwrap_or(0).max(d.downloaded));
            bar.set_position(d.downloaded);
            match d.status {
                Status::Completed => {
                    let secs = d.finished_at.zip(Some(d.added_at)).map_or(0, |(f, a)| f.saturating_sub(a));
                    let check = if d.checksum_verified { " · checksum ✓" } else { "" };
                    bar.finish_with_message(format!(
                        "\x1b[32m✓ {}\x1b[0m in {}{check}",
                        format_bytes(d.downloaded),
                        format_duration(secs.max(1))
                    ));
                }
                Status::Failed => {
                    bar.abandon_with_message(format!("\x1b[31m✗ {}\x1b[0m", d.error.as_deref().unwrap_or("failed")));
                }
                status => {
                    all_done = false;
                    bar.set_message(progress_message(d, status));
                }
            }
        }
        if all_done {
            break false;
        }
    };

    engine.shutdown().await;
    let snap = engine.snapshot();
    for bar in bars.values() {
        if !bar.is_finished() {
            bar.abandon_with_message("\x1b[33m‖ paused\x1b[0m".to_owned());
        }
    }
    let completed = snap.count(|s| s == Status::Completed);
    let failed = snap.count(|s| s == Status::Failed);
    let bytes: u64 = snap.downloads.iter().filter(|d| d.status == Status::Completed).map(|d| d.downloaded).sum();
    let secs = started.elapsed().as_secs_f64().max(0.001);
    eprintln!(
        "\n\x1b[1m{completed}/{} done\x1b[0m · {} in {} · avg {}",
        ids.len(),
        format_bytes(bytes),
        format_duration(secs as u64),
        format_speed(bytes as f64 / secs)
    );
    if interrupted {
        eprintln!("\x1b[33mInterrupted.\x1b[0m Partial files were kept — run the same command again to resume.");
        return Ok(ExitCode::from(130));
    }
    Ok(if failed > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

fn progress_message(d: &stewardget_core::DownloadView, status: Status) -> String {
    match status {
        Status::Downloading => {
            let pct = d.fraction().map_or("  ? ".into(), |f| format!("{:>3.0}%", f * 100.0));
            let size = match d.total {
                Some(t) => format!("{}/{}", format_bytes(d.downloaded), format_bytes(t)),
                None => format_bytes(d.downloaded),
            };
            let eta = d.eta.map_or("--".into(), format_duration);
            format!(
                "{pct} {size:>21} \x1b[36m{:>11}\x1b[0m ETA {eta:>7} \x1b[2m×{}\x1b[0m",
                format_speed(d.speed),
                d.connections
            )
        }
        other => format!("\x1b[2m{}…\x1b[0m", other.label()),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return format!("{s:<max$}");
    }
    let head: String = s.chars().take(max - 1).collect();
    format!("{head}…")
}
