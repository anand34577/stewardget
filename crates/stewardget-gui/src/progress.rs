//! Progress windows: one small window per download, like IDM's and AB
//! Download Manager's. It shows the numbers, a bar per connection and, on
//! request, the parts; it has the download's own speed limit, and it turns
//! into the "Download complete" window when the file is done.
//!
//! They are drawn from the main window's pass, which runs even while the
//! main window is in the tray (see `StewardGetApp::logic`), so they appear on
//! their own when the browser hands over a download.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, RichText, Stroke, Ui, ViewportId, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_speed};
use stewardget_core::{DownloadId, DownloadView, Status};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::table::{Problem, file_exists, problem_of, status_label};
use crate::theme::{self, Palette};
use crate::widgets::{self, ButtonKind};

const WIDTH: f32 = 470.0;
/// Per-download speed limits offered in the window, bytes/second (0 = unlimited).
const LIMITS: [u64; 7] = [0, 256 << 10, 512 << 10, 1 << 20, 2 << 20, 5 << 20, 10 << 20];

/// An open progress window.
pub struct ProgressWindow {
    pub id: DownloadId,
    /// Close the window by itself once the download has finished.
    close_when_done: bool,
    /// Opened to announce the finished file (Settings › "Download complete" window).
    announce: bool,
    /// Pause was pressed on a download that can't resume: ask first.
    confirm_stop: bool,
    /// What went wrong with the last button, shown in the window (the main
    /// window may be in the tray).
    error: Option<String>,
    /// Raise it on the next frame.
    focus: bool,
    /// Height of the content last frame; the window is sized to it.
    height: f32,
    /// Where it opened, picked once so it never jumps.
    pos: Option<egui::Pos2>,
}

impl StewardGetApp {
    /// Opens (or brings forward) the progress window of a download.
    pub fn open_progress(&mut self, id: DownloadId) {
        self.open_progress_window(id, false);
    }

    /// Shows the finished download in its window, opening one if needed.
    pub fn announce_completion(&mut self, id: DownloadId) {
        self.open_progress_window(id, true);
    }

    fn open_progress_window(&mut self, id: DownloadId, announce: bool) {
        if let Some(w) = self.progress.iter_mut().find(|w| w.id == id) {
            w.focus = true;
            w.announce |= announce;
            return;
        }
        // A handful at most: each is a real window.
        if self.progress.len() >= 8 {
            self.progress.remove(0);
        }
        self.progress.push(ProgressWindow {
            id,
            close_when_done: false,
            announce,
            confirm_stop: false,
            error: None,
            focus: true,
            height: 300.0,
            pos: None,
        });
    }

    pub fn progress_windows(&mut self, ctx: &egui::Context) {
        let windows = std::mem::take(&mut self.progress);
        for mut w in windows {
            let Some(d) = self.snap.downloads.iter().find(|d| d.id == w.id).cloned() else { continue };
            if w.close_when_done && d.status == Status::Completed && !w.announce {
                continue;
            }
            if self.progress_window(ctx, &mut w, &d) {
                self.progress.push(w);
            }
        }
        // Opened from one of them, e.g. "Don't show this again".
        self.progress.dedup_by_key(|w| w.id);
    }

    /// Draws one window; `false` once it was closed.
    fn progress_window(&mut self, ctx: &egui::Context, w: &mut ProgressWindow, d: &DownloadView) -> bool {
        let p = self.palette;
        let done = d.status == Status::Completed;
        let title = if done {
            format!("Download complete – {}", d.file_name)
        } else {
            let pct = d.fraction().map(|f| format!("{}% ", (f * 100.0).floor() as u32)).unwrap_or_default();
            format!("{pct}{} – StewardGet", d.file_name)
        };
        let nth = self.progress.len();
        let pos = *w.pos.get_or_insert_with(|| crate::app::screen_spot(ctx, vec2(WIDTH, 420.0), nth));
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_position(pos)
            .with_app_id("dev.stewardget.StewardGet")
            .with_inner_size([WIDTH, w.height])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(crate::app_icon());
        let mut open = true;
        let mut action: Option<Action> = None;
        let mut details = self.progress_details;
        ctx.show_viewport_immediate(ViewportId::from_hash_of(("stewardget-progress", w.id)), builder, |ui, class| {
            if std::mem::take(&mut w.focus) && class == egui::ViewportClass::Immediate {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            ui.painter().rect_filled(ui.max_rect(), CornerRadius::ZERO, p.card);
            let inner = Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                header(ui, &p, d);
                ui.add_space(6.0);
                if let Some(e) = &w.error {
                    ui.add(egui::Label::new(RichText::new(e).size(12.5).color(p.red)).wrap());
                }
                action = if done { complete_body(ui, &p, d, w) } else { running_body(ui, &p, d, w, &mut details) };
            });
            // Fit the window to what it shows (details open or not, finished or not).
            let wanted = inner.response.rect.height() + 2.0;
            if class == egui::ViewportClass::Immediate && (wanted - w.height).abs() > 1.5 {
                w.height = wanted;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIDTH, wanted)));
            }
            if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        });
        self.progress_details = details;
        let action_taken = action.is_some();
        let result = match action {
            Some(Action::Pause) if !d.resumable && d.downloaded > 0 && !w.confirm_stop => {
                w.confirm_stop = true;
                Ok(())
            }
            Some(Action::Pause) => {
                w.confirm_stop = false;
                self.engine.pause(d.id)
            }
            Some(Action::KeepGoing) => {
                w.confirm_stop = false;
                Ok(())
            }
            Some(Action::Resume) => self.engine.resume(d.id),
            Some(Action::Restart) => self.engine.restart(d.id),
            Some(Action::Limit(bytes)) => self.engine.set_download_speed_limit(d.id, bytes),
            Some(Action::Open) => {
                open = false;
                stewardget_core::util::open_with_system(&d.path).map_err(|e| stewardget_core::Error::Io(e.to_string()))
            }
            Some(Action::Folder) => stewardget_core::util::reveal_in_file_manager(&d.path)
                .map_err(|e| stewardget_core::Error::Io(e.to_string())),
            Some(Action::ShowInList) => {
                self.show_in_list(ctx, d.id);
                Ok(())
            }
            Some(Action::NeverAnnounce) => {
                let mut config = self.engine.config();
                config.completion_dialog = false;
                w.announce = false;
                self.engine.update_config(config)
            }
            Some(Action::Close) => {
                open = false;
                Ok(())
            }
            None => Ok(()),
        };
        if action_taken {
            w.error = result.err().map(|e| crate::app::capitalize(&e.to_string()));
        }
        open
    }
}

enum Action {
    Pause,
    /// The "can't resume" warning was answered with "Keep downloading".
    KeepGoing,
    Resume,
    Restart,
    Limit(u64),
    Open,
    Folder,
    ShowInList,
    NeverAnnounce,
    Close,
}

/// File icon, name and where it comes from.
fn header(ui: &mut Ui, p: &Palette, d: &DownloadView) {
    ui.horizontal(|ui| {
        widgets::icon(ui, Icon::for_category(d.category), 26.0, p.category(d.category));
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(
                egui::Label::new(RichText::new(&d.file_name).size(15.0).family(theme::semi()).color(p.text)).truncate(),
            );
            ui.add(egui::Label::new(RichText::new(&d.url).size(11.5).color(p.text_dim)).truncate())
                .on_hover_text(&d.url);
        });
    });
}

/// While downloading, paused or failed: IDM's "Download status" page.
fn running_body(
    ui: &mut Ui,
    p: &Palette,
    d: &DownloadView,
    w: &mut ProgressWindow,
    details: &mut bool,
) -> Option<Action> {
    let mut action = None;
    facts(ui, p, d);
    ui.add_space(8.0);
    big_bar(ui, p, d);
    if d.segments.len() > 1 && d.total.is_some() {
        ui.add_space(8.0);
        ui.label(RichText::new("Download progress by connection").size(11.5).color(p.text_dim));
        connection_bars(ui, p, d);
    }
    if *details && !d.segments.is_empty() {
        ui.add_space(6.0);
        parts_table(ui, p, d);
    }
    if d.status != Status::Failed {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Speed limit").color(p.text_dim));
            let current = d.options.speed_limit;
            let label = |v: u64| if v == 0 { "Unlimited".to_owned() } else { format!("{}/s", format_bytes(v)) };
            egui::ComboBox::from_id_salt(("progress-limit", d.id)).width(120.0).selected_text(label(current)).show_ui(
                ui,
                |ui| {
                    for v in LIMITS {
                        if ui.selectable_label(current == v, label(v)).clicked() && v != current {
                            action = Some(Action::Limit(v));
                        }
                    }
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.checkbox(&mut w.close_when_done, "Close when finished");
            });
        });
    }
    if w.confirm_stop && d.status.can_pause() {
        ui.add_space(6.0);
        Frame::new()
            .fill(p.yellow.gamma_multiply(if p.dark { 0.14 } else { 0.1 }))
            .stroke(Stroke::new(1.0, p.yellow.gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::icon(ui, Icon::Warning, 14.0, p.yellow);
                    ui.label(
                        RichText::new("This server can't resume. Stopping now means starting over from zero later.")
                            .size(12.5)
                            .color(p.text),
                    );
                });
                ui.horizontal(|ui| {
                    if widgets::button(ui, ButtonKind::Danger, None, "Stop anyway", true, p).clicked() {
                        action = Some(Action::Pause);
                    }
                    if widgets::button(ui, ButtonKind::Ghost, None, "Keep downloading", true, p).clicked() {
                        action = Some(Action::KeepGoing);
                    }
                });
            });
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if d.status.can_pause() {
            if widgets::button(ui, ButtonKind::Primary, Some(Icon::Pause), "Pause", true, p).clicked() {
                action = Some(Action::Pause);
            }
        } else if d.status == Status::Failed && problem_of(d) == Some(Problem::NoResume) {
            if widgets::button(ui, ButtonKind::Primary, Some(Icon::Retry), "Start over", true, p).clicked() {
                action = Some(Action::Restart);
            }
        } else if d.status.can_resume() {
            let label = if d.status == Status::Failed { "Retry" } else { "Resume" };
            let icon = if d.status == Status::Failed { Icon::Retry } else { Icon::Play };
            if widgets::button(ui, ButtonKind::Primary, Some(icon), label, true, p).clicked() {
                action = Some(Action::Resume);
            }
        }
        let (label, arrow) = if *details { ("Hide details", Icon::CaretUp) } else { ("Show details", Icon::CaretDown) };
        if widgets::button(ui, ButtonKind::Tool, Some(arrow), label, !d.segments.is_empty(), p).clicked() {
            *details = !*details;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::button(ui, ButtonKind::Ghost, None, "Close", true, p).on_hover_text("Esc").clicked() {
                action = Some(Action::Close);
            }
            if widgets::icon_button(ui, Icon::FolderOpen, "Show in folder", p).clicked() {
                action = Some(Action::Folder);
            }
            if d.status == Status::Failed && widgets::icon_button(ui, Icon::Link, "Show in the list", p).clicked() {
                action = Some(Action::ShowInList);
            }
        });
    });
    action
}

/// The finished file, like IDM's and ABDM's "Download complete" window.
fn complete_body(ui: &mut Ui, p: &Palette, d: &DownloadView, w: &ProgressWindow) -> Option<Action> {
    let mut action = None;
    let exists = file_exists(&d.path);
    ui.horizontal(|ui| {
        widgets::icon(ui, Icon::CheckCircle, 16.0, p.green);
        ui.label(RichText::new("Download complete").size(14.0).family(theme::semi()).color(p.green));
    });
    ui.add_space(4.0);
    let secs = d.active_secs.max(1);
    let rows = [
        ("Size", format_bytes(d.downloaded), p.text),
        (
            "Took",
            format!("{} · average {}", format_duration(secs), format_speed(d.downloaded as f64 / secs as f64)),
            p.text,
        ),
    ];
    egui::Grid::new(("progress-done", d.id)).num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
        for (label, value, color) in rows {
            ui.label(RichText::new(label).color(p.text_dim));
            ui.label(RichText::new(value).color(color));
            ui.end_row();
        }
        ui.label(RichText::new("Saved to").color(p.text_dim));
        ui.add(egui::Label::new(RichText::new(d.path.display().to_string()).color(p.text)).truncate())
            .on_hover_text(d.path.display().to_string());
        ui.end_row();
    });
    if d.checksum_verified {
        ui.label(RichText::new("Checksum verified").size(12.5).color(p.green));
    }
    if !exists {
        ui.label(RichText::new("The file is no longer there: it was moved or deleted.").size(12.5).color(p.yellow));
    }
    if stewardget_core::util::disguised_program(&d.file_name) {
        ui.label(
            RichText::new("A program disguised as a document or picture: do not open it.").size(12.5).color(p.red),
        );
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if widgets::button(ui, ButtonKind::Primary, Some(Icon::External), "Open", exists, p).clicked() {
            action = Some(Action::Open);
        }
        if widgets::button(ui, ButtonKind::Tool, Some(Icon::FolderOpen), "Show in folder", exists, p).clicked() {
            action = Some(Action::Folder);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::button(ui, ButtonKind::Ghost, None, "Close", true, p).on_hover_text("Esc").clicked() {
                action = Some(Action::Close);
            }
        });
    });
    if w.announce {
        ui.add_space(4.0);
        let r = ui
            .add(
                egui::Label::new(
                    RichText::new("Don't show this window for finished downloads")
                        .size(11.5)
                        .color(p.text_dim)
                        .underline(),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_text("Turn it back on in Settings › General.")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            action = Some(Action::NeverAnnounce);
        }
    }
    action
}

/// Name/value lines, like IDM's: status, size, downloaded, rate, time left, resume.
fn facts(ui: &mut Ui, p: &Palette, d: &DownloadView) {
    let (status, color) = status_label(d, p);
    let running = matches!(d.status, Status::Downloading | Status::Connecting);
    let size = d.total.map_or_else(|| "Unknown".to_owned(), format_bytes);
    let downloaded = match d.fraction() {
        Some(f) => format!("{} ({:.1}%)", format_bytes(d.downloaded), (f * 1000.0).floor() / 10.0),
        None => format_bytes(d.downloaded),
    };
    let rate = if running && d.speed > 0.0 { format_speed(d.speed) } else { "—".into() };
    let left = match d.eta {
        Some(eta) if running => format_duration(eta),
        _ => "—".into(),
    };
    // Unknown until the server has answered.
    let (resume, resume_color) = if d.resumable {
        ("Yes", p.green)
    } else if d.total.is_none() && d.downloaded == 0 && !running {
        ("—", p.text)
    } else {
        ("No", p.yellow)
    };
    let rows: [(&str, String, egui::Color32); 6] = [
        ("Status", status, color),
        ("File size", size, p.text),
        ("Downloaded", downloaded, p.text),
        ("Transfer rate", rate, if running { p.accent2 } else { p.text }),
        ("Time left", left, p.text),
        ("Resume support", resume.into(), resume_color),
    ];
    egui::Grid::new(("progress-facts", d.id)).num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
        for (label, value, color) in &rows {
            ui.label(RichText::new(*label).color(p.text_dim));
            ui.label(RichText::new(value).color(*color));
            ui.end_row();
        }
    });
    if d.status == Status::Failed
        && let Some(error) = &d.error
    {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(crate::app::capitalize(error)).size(12.5).color(p.red)).wrap());
    }
}

/// The wide progress bar.
fn big_bar(ui: &mut Ui, p: &Palette, d: &DownloadView) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), egui::Sense::hover());
    widgets::paint_progress(ui, rect, d, p);
}

/// IDM's "start positions and download progress by connections": the file as
/// one strip, each part drawn from where it starts, filled as far as it got.
fn connection_bars(ui: &mut Ui, p: &Palette, d: &DownloadView) {
    let Some(total) = d.total.filter(|&t| t > 0) else { return };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(3), p.track);
    let x = |byte: u64| rect.left() + rect.width() * (byte.min(total) as f64 / total as f64) as f32;
    let (from, to) = widgets::status_gradient(d.status, p);
    for s in &d.segments {
        let end = s.end.min(total);
        let head = (s.start + s.done).min(end);
        if head > s.start {
            let r = Rect::from_min_max(egui::pos2(x(s.start), rect.top()), egui::pos2(x(head), rect.bottom()));
            painter.rect_filled(
                r,
                CornerRadius::ZERO,
                widgets::lerp_color(from, to, x(s.start) / rect.right().max(1.0)),
            );
        }
        // Where each connection starts, and its head while it is writing.
        let start = x(s.start);
        painter
            .line_segment([egui::pos2(start, rect.top()), egui::pos2(start, rect.bottom())], Stroke::new(1.0, p.card));
        if s.active && head < end && d.status == Status::Downloading {
            let hx = x(head);
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(hx - 1.0, rect.top()), egui::pos2(hx + 1.0, rect.bottom())),
                CornerRadius::ZERO,
                p.text,
            );
        }
    }
}

/// One line per part: number, what it is doing, how far it got.
fn parts_table(ui: &mut Ui, p: &Palette, d: &DownloadView) {
    egui::ScrollArea::vertical().id_salt(("parts", d.id)).auto_shrink([false, true]).max_height(160.0).show(ui, |ui| {
        egui::Grid::new(("parts-grid", d.id)).num_columns(4).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
            for title in ["N.", "Downloaded", "Part size", "Info"] {
                ui.label(RichText::new(title).size(11.5).color(p.text_dim));
            }
            ui.end_row();
            for (i, s) in d.segments.iter().enumerate() {
                let len = s.end.saturating_sub(s.start);
                let done = s.done.min(len);
                let (state, color) = if done >= len {
                    ("Done", p.green)
                } else if s.active && d.status == Status::Downloading {
                    ("Receiving data…", p.accent2)
                } else if s.active {
                    ("Connecting…", p.text_dim)
                } else if d.status.is_active() {
                    ("Waiting", p.text_dim)
                } else {
                    ("Stopped", p.text_dim)
                };
                ui.label(RichText::new(format!("{}", i + 1)).color(p.text_dim));
                ui.label(format_bytes(done));
                ui.label(if len == u64::MAX { "—".into() } else { format_bytes(len) });
                ui.label(RichText::new(state).color(color));
                ui.end_row();
            }
        });
    });
}
