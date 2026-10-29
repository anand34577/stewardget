//! Bottom details pane: file info, problem resolution, connection map, speed
//! graph and a hash tool.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout, RichText, Ui, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_speed, format_timestamp, parse_size};
use stewardget_core::{DownloadView, HashAlgo, Status};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::table::{Problem, problem_of, status_label};
use crate::theme;
use crate::widgets::{self, ButtonKind};

/// A file hash computed on demand in the background.
pub struct HashJob {
    pub algo: HashAlgo,
    pub result: Arc<Mutex<Option<Result<String, String>>>>,
}

impl StewardGetApp {
    pub fn details_pane(&mut self, ui: &mut Ui) {
        let p = self.palette;
        // The full view: snapshots leave out the speed history of finished downloads.
        let Some(d) = self.single_selected().map(|d| self.engine.download(d.id).unwrap_or(d)) else {
            let sel = self.selected_views();
            let total: u64 = sel.iter().filter_map(|d| d.total).sum();
            let done: u64 = sel.iter().map(|d| d.downloaded).sum();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} downloads selected", sel.len())).family(theme::semi()).color(p.text),
                );
                ui.label(
                    RichText::new(format!("{} of {} downloaded", format_bytes(done), format_bytes(total)))
                        .color(p.text_dim),
                );
            });
            return;
        };

        // Header line: icon, name, status, quick actions.
        ui.horizontal(|ui| {
            widgets::icon(ui, Icon::for_category(d.category), 20.0, p.category(d.category));
            ui.add(
                egui::Label::new(RichText::new(&d.file_name).size(15.0).family(theme::semi()).color(p.text)).truncate(),
            );
            let (label, color) = status_label(&d, &p);
            widgets::pill(ui, &label, color);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if widgets::icon_button(ui, Icon::Close, "Close details", &p).clicked() {
                    self.show_details = false;
                }
                if widgets::icon_button(ui, Icon::Sliders, "Properties (F2)", &p).clicked() {
                    self.open_properties(&d);
                }
                if widgets::icon_button(ui, Icon::FolderOpen, "Show in folder", &p).clicked() {
                    let _ = stewardget_core::util::reveal_in_file_manager(&d.path);
                }
                if d.status == Status::Completed
                    && crate::table::file_exists(&d.path)
                    && widgets::button(ui, ButtonKind::Tool, Some(Icon::External), "Open", true, &p).clicked()
                {
                    let _ = stewardget_core::util::open_with_system(&d.path);
                }
                if d.status.can_pause()
                    && widgets::button(ui, ButtonKind::Tool, Some(Icon::Pause), "Pause", true, &p).clicked()
                {
                    let r = self.engine.pause(d.id);
                    self.report(r);
                }
                if d.status.can_resume() && problem_of(&d) != Some(Problem::Changed) {
                    let label = if d.status == Status::Failed { "Retry" } else { "Resume" };
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::Play), label, true, &p).clicked() {
                        let r = self.engine.resume(d.id);
                        self.report(r);
                    }
                }
            });
        });
        self.problem_bar(ui, &d);
        if d.status.is_active() {
            ui.add_space(4.0);
            self.insight_card(ui, &d);
            self.lanes_card(ui, &d);
        }
        ui.add_space(6.0);

        let wide = ui.available_width() > 760.0;
        if wide {
            let avail = ui.available_width();
            let height = ui.available_height();
            ui.horizontal_top(|ui| {
                let column = Layout::top_down(Align::Min);
                ui.allocate_ui_with_layout(vec2(avail * 0.38, height), column, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("info")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.info_grid(ui, &d));
                });
                ui.separator();
                ui.allocate_ui_with_layout(vec2(avail * 0.34, height), column, |ui| self.map_block(ui, &d, height));
                ui.separator();
                ui.allocate_ui_with_layout(vec2(ui.available_width(), height), column, |ui| {
                    self.speed_block(ui, &d, height)
                });
            });
        } else {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.map_block(ui, &d, 90.0);
                ui.add_space(5.0);
                self.info_grid(ui, &d);
                ui.add_space(5.0);
                self.speed_block(ui, &d, 120.0);
            });
        }
    }

    /// Explains what went wrong and offers the right way forward; partial data
    /// is never thrown away without asking.
    fn problem_bar(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        let missing = d.status == Status::Completed && !crate::table::file_exists(&d.path);
        if missing {
            ui.horizontal_wrapped(|ui| {
                widgets::icon(ui, Icon::Warning, 14.0, p.yellow);
                ui.label(RichText::new("The file was moved or deleted.").size(12.5).color(p.yellow));
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Retry), "Download again", true, &p).clicked() {
                    let r = self.engine.restart(d.id);
                    self.report(r);
                }
            });
            return;
        }
        if d.awaiting_link {
            ui.label(
                RichText::new("Waiting for a fresh link: start this download again in your browser and StewardGet continues it automatically.")
                    .size(12.5)
                    .color(p.yellow),
            );
            return;
        }
        if let Some(secs) = d.retry_in {
            let err = d.error.clone().unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                widgets::icon(ui, Icon::Timer, 14.0, p.yellow);
                ui.label(
                    RichText::new(format!(
                        "{} — retrying automatically in {} (attempt {})",
                        crate::app::capitalize(&err),
                        format_duration(secs),
                        d.retries + 1
                    ))
                    .size(12.5)
                    .color(p.yellow),
                );
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Play), "Retry now", true, &p).clicked() {
                    let r = self.engine.pause(d.id).and_then(|()| self.engine.resume(d.id));
                    self.report(r);
                }
            });
            return;
        }
        let Some(err) = d.error.clone().filter(|_| d.status == Status::Failed) else { return };
        let kind = problem_of(d).unwrap_or(Problem::Other);
        let hint = match kind {
            Problem::Expired => "The link has probably expired. Refresh it to continue from where it stopped.",
            Problem::NotFound => "Check that the link is right, or open the page and start the download again.",
            Problem::Denied => {
                "The site wants you to be logged in. Start the download from your browser so StewardGet gets its cookies, or refresh the link."
            }
            Problem::Changed => {
                "Your partial data is kept. If it is still the same file (e.g. a re-signed link), resume anyway; otherwise start over."
            }
            Problem::SizeChanged => "This looks like a different file now. Refresh the link, or start over.",
            Problem::NoResume => {
                "The partial data can't be continued from this server. Try a fresh link, or start over."
            }
            Problem::DiskFull => "Free up space or move the download to another drive, then retry.",
            Problem::Checksum => "The file is corrupt or the checksum is wrong. Start over, or edit the checksum.",
            Problem::Other => "",
        };
        ui.horizontal_wrapped(|ui| {
            widgets::icon(ui, Icon::Alert, 14.0, p.red);
            ui.label(RichText::new(crate::app::capitalize(&err)).size(12.5).color(p.red));
            if !hint.is_empty() {
                ui.label(RichText::new(hint).size(12.5).color(p.text_dim));
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if kind == Problem::Changed
                && widgets::button(ui, ButtonKind::Primary, Some(Icon::Play), "Resume anyway", true, &p).clicked()
            {
                let r = self.engine.resume_anyway(d.id);
                self.report(r);
            }
            if matches!(
                kind,
                Problem::Expired
                    | Problem::NotFound
                    | Problem::Denied
                    | Problem::Changed
                    | Problem::SizeChanged
                    | Problem::NoResume
            ) && widgets::button(ui, ButtonKind::Ghost, Some(Icon::Link), "Refresh link", true, &p).clicked()
            {
                self.open_refresh(d);
            }
            if kind == Problem::DiskFull
                && widgets::button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Move…", true, &p).clicked()
            {
                self.open_properties(d);
            }
            if matches!(kind, Problem::Changed | Problem::SizeChanged | Problem::NoResume | Problem::Checksum)
                && widgets::button(ui, ButtonKind::Ghost, Some(Icon::Retry), "Start over", true, &p).clicked()
            {
                self.restart_selected();
            }
        });
    }

    fn info_grid(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        egui::Grid::new(("stewardget-info", d.id)).num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
            let key = |ui: &mut Ui, k: &str| {
                ui.label(RichText::new(k).size(12.5).color(p.text_dim));
            };
            let value = |ui: &mut Ui, v: String| {
                ui.add(egui::Label::new(RichText::new(v).size(12.5).color(p.text)).truncate());
            };
            key(ui, "Size");
            value(
                ui,
                match d.total {
                    Some(t) => format!("{} of {}", format_bytes(d.downloaded), format_bytes(t)),
                    None => format!("{} (size unknown)", format_bytes(d.downloaded)),
                },
            );
            ui.end_row();
            if d.status == Status::Downloading {
                key(ui, "Speed");
                value(ui, format!("{} · {} left", format_speed(d.speed), d.eta.map_or("—".into(), format_duration)));
                ui.end_row();
            }
            key(ui, "Saved to");
            value(ui, d.path.display().to_string());
            ui.end_row();
            key(ui, "Queue");
            value(ui, self.engine.config().queue(d.queue).name.clone());
            ui.end_row();
            key(ui, "Link");
            ui.horizontal(|ui| {
                if widgets::icon_button(ui, Icon::Copy, "Copy link", &p).clicked() {
                    ui.ctx().copy_text(d.url.clone());
                }
                ui.add(egui::Label::new(RichText::new(&d.url).size(12.5).color(p.accent2)).truncate());
            });
            ui.end_row();
            if let Some(r) = &d.options.referrer {
                key(ui, "Page");
                value(ui, r.clone());
                ui.end_row();
            }
            key(ui, "Resumable");
            value(ui, if d.resumable { "Yes".into() } else { "No — the server doesn't support ranges".into() });
            ui.end_row();
            key(ui, "Added");
            value(ui, format_timestamp(d.added_at));
            ui.end_row();
            if let Some(t) = d.finished_at {
                key(ui, "Finished");
                let secs = t.saturating_sub(d.added_at).max(1);
                value(ui, format!("{} · avg {}", format_timestamp(t), format_speed(d.downloaded as f64 / secs as f64)));
                ui.end_row();
            }
            if let Some(c) = &d.options.checksum {
                key(ui, c.algo.name().to_uppercase().as_str());
                value(ui, if d.checksum_verified { "Verified".into() } else { "Will verify on completion".into() });
                ui.end_row();
            }
            if d.status == Status::Completed && crate::table::file_exists(&d.path) {
                key(ui, "Hash");
                self.hash_row(ui, d);
                ui.end_row();
            }
            if d.status != Status::Completed {
                key(ui, "Speed limit");
                ui.horizontal(|ui| {
                    let hint = if d.options.speed_limit == 0 {
                        "unlimited".to_owned()
                    } else {
                        format_speed(d.options.speed_limit as f64)
                    };
                    let r = ui.add(widgets::input(&mut self.limit_text).hint_text(hint).desired_width(100.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if widgets::button(ui, ButtonKind::Ghost, None, "Set", true, &p).clicked() || enter {
                        let text = self.limit_text.trim();
                        match parse_size(if text.is_empty() { "0" } else { text }) {
                            Some(rate) => {
                                let r = self.engine.set_download_speed_limit(d.id, rate);
                                self.report(r);
                                self.limit_text.clear();
                            }
                            None => {
                                let red = p.red;
                                self.toast("Use a rate like 500k or 2M", red);
                            }
                        }
                    }
                });
                ui.end_row();
            }
        });
    }

    /// Computes MD5/SHA-1/SHA-256/SHA-512 of a finished file in the background.
    fn hash_row(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        let job = self.hashes.get(&d.id).map(|j| (j.algo, j.result.lock().unwrap().clone()));
        ui.horizontal(|ui| match job {
            Some((_, None)) => {
                ui.add(egui::Spinner::new().size(13.0).color(p.accent2));
                ui.label(RichText::new("Hashing…").size(12.5).color(p.text_dim));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            Some((algo, Some(Ok(hex)))) => {
                if widgets::icon_button(ui, Icon::Copy, "Copy hash", &p).clicked() {
                    ui.ctx().copy_text(hex.clone());
                }
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{}  {hex}", algo.name().to_uppercase()))
                            .size(12.0)
                            .monospace()
                            .color(p.text),
                    )
                    .truncate(),
                );
            }
            other => {
                if let Some((_, Some(Err(e)))) = other {
                    ui.label(RichText::new(e).size(12.5).color(p.red));
                }
                for algo in [HashAlgo::Sha256, HashAlgo::Sha1, HashAlgo::Md5, HashAlgo::Sha512] {
                    if widgets::button(ui, ButtonKind::Ghost, None, &algo.name().to_uppercase(), true, &p)
                        .on_hover_text("Compute this hash of the file")
                        .clicked()
                    {
                        let slot = Arc::new(Mutex::new(None));
                        let (path, out) = (d.path.clone(), Arc::clone(&slot));
                        self.runtime.spawn_blocking(move || {
                            let r = stewardget_core::hash_file(algo, &path).map_err(|e| e.to_string());
                            *out.lock().unwrap() = Some(r);
                        });
                        self.hashes.insert(d.id, HashJob { algo, result: slot });
                    }
                }
            }
        });
    }

    fn map_block(&mut self, ui: &mut Ui, d: &DownloadView, height: f32) {
        let p = self.palette;
        ui.horizontal(|ui| {
            widgets::caption(ui, "CONNECTION MAP", &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pct = d.fraction().map_or("—".into(), |f| format!("{:.1}%", f * 100.0));
                ui.label(RichText::new(pct).family(theme::semi()).color(p.text));
            });
        });
        // Fit the space left in the pane; never make the pane grow.
        let map_h = (ui.available_height().min(height) - 24.0).clamp(28.0, 220.0);
        widgets::connection_map(ui, d, &p, map_h);
        let text = if d.segments.is_empty() {
            if d.resumable || d.status == Status::Completed { String::new() } else { "single connection".into() }
        } else {
            format!("{} segments · {} active connections", d.segments.len(), d.connections)
        };
        ui.label(RichText::new(text).size(11.5).color(p.text_dim));
    }

    fn speed_block(&mut self, ui: &mut Ui, d: &DownloadView, height: f32) {
        let p = self.palette;
        ui.horizontal(|ui| {
            widgets::caption(ui, "SPEED", &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Nothing sampled this session: no peak to show, rather than "peak 0 B/s".
                let peak = d.history.iter().copied().fold(0.0f32, f32::max);
                if peak > 0.0 {
                    ui.label(RichText::new(format!("peak {}", format_speed(peak as f64))).size(11.5).color(p.text_dim));
                }
            });
        });
        if d.history.is_empty() {
            // Nothing sampled in this session (e.g. finished before StewardGet was restarted).
            let text = match (d.status, d.finished_at) {
                (Status::Completed, Some(t)) => {
                    // Time spent transferring; older records only know when they were added.
                    let secs = if d.active_secs > 0 { d.active_secs } else { t.saturating_sub(d.added_at) }.max(1);
                    format!(
                        "Finished in {} · average {}",
                        format_duration(secs),
                        format_speed(d.downloaded as f64 / secs as f64)
                    )
                }
                _ => "The speed graph appears while downloading.".to_owned(),
            };
            ui.add_space(5.0);
            ui.label(RichText::new(text).color(p.text_dim));
            return;
        }
        let size = vec2(ui.available_width(), (ui.available_height().min(height) - 6.0).clamp(40.0, 220.0));
        widgets::area_chart(ui, &d.history, size, p.accent2, &p);
    }
}
