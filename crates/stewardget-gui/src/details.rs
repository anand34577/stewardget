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
        if ui.available_width() < 560.0 {
            self.inspector(ui, &d);
            return;
        }

        // Header line: icon, then the actions on the right, and the name, status
        // and details in the room that leaves (truncated, never under the buttons).
        let status = status_label(&d, &p);
        let mut sub = format!("{}  \u{00b7}  {}", self.cfg.category_label(d.category), crate::capture::host_of(&d.url));
        if let Some(total) = d.total {
            sub = format!("{sub}  \u{00b7}  {}", format_bytes(total));
        }
        ui.horizontal(|ui| {
            widgets::icon_tile(ui, Icon::for_category(d.category), 38.0, p.category(d.category), &p);
            ui.add_space(4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if widgets::icon_button(ui, Icon::Close, "Close details", &p).clicked() {
                    self.show_details = false;
                }
                if widgets::icon_button(ui, Icon::Sliders, "Properties (F2)", &p).clicked() {
                    self.open_properties(&d);
                }
                if widgets::icon_button(ui, Icon::FolderOpen, "Show in folder", &p).clicked() {
                    self.reveal(&d.path);
                }
                ui.add_space(4.0);
                if d.status == Status::Completed
                    && crate::table::file_exists(&d.path)
                    && widgets::button(ui, ButtonKind::Primary, Some(Icon::External), "Open", true, &p).clicked()
                {
                    self.open_path(&d.path);
                }
                if d.status.can_pause()
                    && widgets::button(ui, ButtonKind::Ghost, Some(Icon::Pause), "Pause", true, &p).clicked()
                {
                    let r = self.engine.pause(d.id);
                    self.report(r);
                }
                if d.status.can_resume() && problem_of(&d) != Some(Problem::Changed) {
                    let label = if d.status == Status::Failed { "Retry" } else { "Resume" };
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Play), label, true, &p).clicked() {
                        let r = self.engine.resume(d.id);
                        self.report(r);
                    }
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.add(
                            egui::Label::new(
                                RichText::new(&d.file_name)
                                    .size(theme::size::LG + 1.0)
                                    .family(theme::semi())
                                    .color(p.text),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&d.file_name);
                        ui.horizontal(|ui| {
                            widgets::pill(ui, &status.0, status.1);
                            ui.add(
                                egui::Label::new(RichText::new(&sub).size(theme::size::SM).color(p.text_dim))
                                    .truncate(),
                            );
                        });
                    });
                });
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
                ui.allocate_ui_with_layout(vec2(avail * 0.34, height), column, |ui| {
                    self.map_block(ui, &d, height, true)
                });
                ui.separator();
                ui.allocate_ui_with_layout(vec2(ui.available_width(), height), column, |ui| {
                    self.speed_block(ui, &d, height, true)
                });
            });
        } else {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.map_block(ui, &d, 90.0, false);
                ui.add_space(5.0);
                self.info_grid(ui, &d);
                ui.add_space(5.0);
                self.speed_block(ui, &d, 120.0, false);
            });
        }
    }

    /// The details as a column beside the list: the file, a progress ring with
    /// the live figures, its actions, then connections, speed and information.
    fn inspector(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(ui.available_width() - 6.0);
            ui.horizontal(|ui| {
                widgets::caption(ui, "Details", &p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, "Close details", &p).clicked() {
                        self.show_details = false;
                    }
                    if widgets::icon_button(ui, Icon::Sliders, "Properties (F2)", &p).clicked() {
                        self.open_properties(d);
                    }
                });
            });
            ui.add_space(6.0);

            // The file.
            ui.horizontal_top(|ui| {
                let (tile, _) = ui.allocate_exact_size(vec2(56.0, 56.0), egui::Sense::hover());
                widgets::paint_file_tile(
                    ui.painter(),
                    tile,
                    Icon::for_category(d.category),
                    widgets::extension(&d.file_name),
                    p.category(d.category),
                    &p,
                );
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add(
                        egui::Label::new(
                            RichText::new(&d.file_name).size(theme::size::LG).family(theme::semi()).color(p.text),
                        )
                        .wrap(),
                    );
                    ui.horizontal(|ui| {
                        let (label, color) = status_label(d, &p);
                        widgets::pill(ui, &label, color);
                        ui.add(
                            egui::Label::new(
                                RichText::new(crate::capture::host_of(&d.url)).size(theme::size::SM).color(p.text_dim),
                            )
                            .truncate(),
                        );
                    });
                });
            });
            ui.add_space(14.0);

            // Progress ring and the figures that go with it.
            let (hero, _) = ui.allocate_exact_size(vec2(ui.available_width(), 128.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect(
                hero,
                egui::CornerRadius::same(theme::radius::LG),
                p.bg,
                egui::Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            );
            let center = hero.left_center() + vec2(64.0, 0.0);
            let (from, to) = widgets::status_gradient(d.status, &p);
            let live = d.status == Status::Downloading;
            if live {
                widgets::paint_glow(painter, center, 80.0, to.gamma_multiply(if p.dark { 0.22 } else { 0.14 }));
            }
            let frac = d.fraction().unwrap_or(0.0);
            widgets::paint_ring(painter, center, 42.0, 8.0, frac, from, to, p.track);
            let pct = match d.fraction() {
                Some(f) if f >= 1.0 => "100%".to_owned(),
                Some(f) => format!("{:.0}%", (f * 100.0).floor()),
                None => "—".to_owned(),
            };
            painter.text(
                center - vec2(0.0, 6.0),
                egui::Align2::CENTER_CENTER,
                pct,
                egui::FontId::new(theme::size::XL, theme::semi()),
                p.text,
            );
            painter.text(
                center + vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                if d.status == Status::Completed { "done" } else { "complete" },
                egui::FontId::proportional(11.0),
                p.text_dim,
            );
            let figures: [(&str, String); 3] = match d.status {
                Status::Completed => {
                    let secs = match d.finished_at {
                        Some(t) if d.active_secs == 0 => t.saturating_sub(d.added_at),
                        _ => d.active_secs,
                    }
                    .max(1);
                    [
                        ("Size", format_bytes(d.total.unwrap_or(d.downloaded))),
                        ("Took", format_duration(secs)),
                        ("Average", format_speed(d.downloaded as f64 / secs as f64)),
                    ]
                }
                _ => [
                    ("Speed", if live { format_speed(d.speed) } else { "—".to_owned() }),
                    ("Time left", d.eta.filter(|_| live).map_or("—".to_owned(), format_duration)),
                    (
                        "Downloaded",
                        match d.total {
                            Some(t) => format!("{} / {}", format_bytes(d.downloaded), format_bytes(t)),
                            None => format_bytes(d.downloaded),
                        },
                    ),
                ],
            };
            let x = center.x + 64.0;
            for (i, (k, v)) in figures.iter().enumerate() {
                let y = hero.top() + 24.0 + i as f32 * 36.0;
                painter.text(
                    egui::pos2(x, y - 7.0),
                    egui::Align2::LEFT_CENTER,
                    *k,
                    egui::FontId::proportional(11.5),
                    p.text_dim,
                );
                let font = egui::FontId::new(if i == 0 { theme::size::LG } else { theme::size::MD }, theme::semi());
                let v = widgets::fit_text(painter, v, &font, hero.right() - 12.0 - x);
                painter.text(egui::pos2(x, y + 9.0), egui::Align2::LEFT_CENTER, v, font, p.text);
            }
            ui.add_space(12.0);

            // Actions.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                if d.status == Status::Completed
                    && crate::table::file_exists(&d.path)
                    && widgets::button(ui, ButtonKind::Primary, Some(Icon::External), "Open file", true, &p).clicked()
                {
                    self.open_path(&d.path);
                }
                if d.status.can_resume() && problem_of(d) != Some(Problem::Changed) {
                    let label = if d.status == Status::Failed { "Retry" } else { "Resume" };
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Play), label, true, &p).clicked() {
                        let r = self.engine.resume(d.id);
                        self.report(r);
                    }
                }
                if d.status.can_pause()
                    && widgets::button(ui, ButtonKind::Ghost, Some(Icon::Pause), "Pause", true, &p).clicked()
                {
                    let r = self.engine.pause(d.id);
                    self.report(r);
                }
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Folder", true, &p).clicked() {
                    self.reveal(&d.path);
                }
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy link", true, &p).clicked() {
                    ui.ctx().copy_text(d.url.clone());
                    let accent = p.accent2;
                    self.toast("Link copied", accent);
                }
            });
            self.problem_bar(ui, d);
            if d.status.is_active() {
                ui.add_space(8.0);
                self.insight_card(ui, d);
                self.lanes_card(ui, d);
            }

            ui.add_space(16.0);
            self.map_block(ui, d, 84.0, false);
            ui.add_space(14.0);
            self.speed_block(ui, d, 120.0, false);
            ui.add_space(16.0);
            widgets::caption(ui, "Information", &p);
            ui.add_space(4.0);
            self.info_grid(ui, d);
            ui.add_space(12.0);
        });
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
        let hint = crate::table::problem_hint(kind, &err);
        ui.add_space(8.0);
        crate::dialogs::callout(ui, &p, p.red, Icon::Alert, |ui| {
            ui.label(RichText::new(crate::app::capitalize(&err)).size(13.0).family(theme::semi()).color(p.red));
            if !hint.is_empty() {
                ui.add(egui::Label::new(RichText::new(hint).size(12.5).color(p.text_dim)).wrap());
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
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
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
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
            widgets::path_label(ui, &d.path.display().to_string(), 12.5, p.text);
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

    /// `fit`: use the space left in the pane (the wide layout, whose columns have a
    /// fixed height) instead of exactly `height` (the scrolling inspector).
    fn map_block(&mut self, ui: &mut Ui, d: &DownloadView, height: f32, fit: bool) {
        let p = self.palette;
        ui.horizontal(|ui| {
            widgets::caption(ui, "CONNECTION MAP", &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pct = d.fraction().map_or("—".into(), |f| format!("{:.1}%", f * 100.0));
                ui.label(RichText::new(pct).family(theme::semi()).color(p.text));
            });
        });
        // Fit the space left in the pane; never make the pane grow.
        let room = if fit { ui.available_height().min(height) } else { height };
        let map_h = (room - 24.0).clamp(28.0, 220.0);
        widgets::connection_map(ui, d, &p, map_h);
        let text = if d.segments.is_empty() {
            if d.resumable || d.status == Status::Completed { String::new() } else { "single connection".into() }
        } else {
            format!("{} segments · {} active connections", d.segments.len(), d.connections)
        };
        ui.label(RichText::new(text).size(12.0).color(p.text_dim));
    }

    fn speed_block(&mut self, ui: &mut Ui, d: &DownloadView, height: f32, fit: bool) {
        let p = self.palette;
        ui.horizontal(|ui| {
            widgets::caption(ui, "SPEED", &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Nothing sampled this session: no peak to show, rather than "peak 0 B/s".
                let peak = d.history.iter().copied().fold(0.0f32, f32::max);
                if peak > 0.0 {
                    ui.label(RichText::new(format!("peak {}", format_speed(peak as f64))).size(12.0).color(p.text_dim));
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
        let room = if fit { ui.available_height().min(height) } else { height };
        let size = vec2(ui.available_width(), (room - 6.0).clamp(40.0, 220.0));
        widgets::area_chart(ui, &d.history, size, p.accent2, &p);
    }
}
