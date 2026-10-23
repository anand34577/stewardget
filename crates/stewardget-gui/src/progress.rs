//! Progress windows: one small window per download, like IDM's, with the
//! numbers, the connection map, the parts and Pause / Open buttons.

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Ui, ViewportId, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_speed};
use stewardget_core::{DownloadId, DownloadView, Status};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::table::{file_exists, status_label};
use crate::theme;
use crate::widgets::{self, ButtonKind};

const WIDTH: f32 = 430.0;

/// An open progress window.
pub struct ProgressWindow {
    pub id: DownloadId,
    /// Show the per-part table.
    parts: bool,
    /// Close the window by itself once the download has finished.
    close_when_done: bool,
    /// Raise it on the next frame.
    focus: bool,
    /// Height of the content last frame; the window is sized to it.
    height: f32,
}

impl StewardGetApp {
    /// Opens (or brings forward) the progress window of a download.
    pub fn open_progress(&mut self, id: DownloadId) {
        if let Some(w) = self.progress.iter_mut().find(|w| w.id == id) {
            w.focus = true;
            return;
        }
        // A handful at most: each is a real window.
        if self.progress.len() >= 8 {
            self.progress.remove(0);
        }
        self.progress.push(ProgressWindow { id, parts: false, close_when_done: false, focus: true, height: 420.0 });
    }

    pub fn progress_windows(&mut self, ctx: &egui::Context) {
        let windows = std::mem::take(&mut self.progress);
        for mut w in windows {
            let Some(d) = self.snap.downloads.iter().find(|d| d.id == w.id).cloned() else { continue };
            if w.close_when_done && d.status == Status::Completed {
                continue;
            }
            if self.progress_window(ctx, &mut w, &d) {
                self.progress.push(w);
            }
        }
    }

    /// Draws one window; `false` once it was closed.
    fn progress_window(&mut self, ctx: &egui::Context, w: &mut ProgressWindow, d: &DownloadView) -> bool {
        let p = self.palette;
        let pct = d.fraction().map(|f| format!("{}% ", (f * 100.0).floor() as u32)).unwrap_or_default();
        let builder = egui::ViewportBuilder::default()
            .with_title(format!("{pct}{} – StewardGet", d.file_name))
            .with_app_id("dev.stewardget.StewardGet")
            .with_inner_size([WIDTH, w.height])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(crate::app_icon());
        let mut open = true;
        let mut action: Option<Action> = None;
        ctx.show_viewport_immediate(ViewportId::from_hash_of(("stewardget-progress", w.id)), builder, |ui, class| {
            if std::mem::take(&mut w.focus) && class == egui::ViewportClass::Immediate {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            ui.painter().rect_filled(ui.max_rect(), CornerRadius::ZERO, p.card);
            let inner = Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                header(ui, &p, d);
                ui.add_space(4.0);
                facts(ui, &p, d);
                ui.add_space(6.0);
                big_bar(ui, &p, d);
                ui.add_space(6.0);
                if w.parts && !d.segments.is_empty() {
                    parts_table(ui, &p, d);
                    ui.add_space(4.0);
                }
                if d.status != Status::Completed {
                    ui.checkbox(&mut w.close_when_done, "Close when finished");
                }
                ui.horizontal(|ui| {
                    action = buttons(ui, &p, d, w);
                });
            });
            // Fit the window to what it shows (parts table open or not, finished or not).
            let wanted = inner.response.rect.height() + 2.0;
            if class == egui::ViewportClass::Immediate && (wanted - w.height).abs() > 1.5 {
                w.height = wanted;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIDTH, wanted)));
            }
            if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        });
        match action {
            Some(Action::Pause) => {
                let r = self.engine.pause(d.id);
                self.report(r);
            }
            Some(Action::Resume) => {
                let r = self.engine.resume(d.id);
                self.report(r);
            }
            Some(Action::Open) => {
                let _ = stewardget_core::util::open_with_system(&d.path);
                open = false;
            }
            Some(Action::Folder) => {
                let _ = stewardget_core::util::reveal_in_file_manager(&d.path);
            }
            Some(Action::Close) => open = false,
            None => {}
        }
        open
    }
}

enum Action {
    Pause,
    Resume,
    Open,
    Folder,
    Close,
}

fn header(ui: &mut Ui, p: &theme::Palette, d: &DownloadView) {
    ui.horizontal(|ui| {
        widgets::icon(ui, Icon::for_category(d.category), 22.0, p.category(d.category));
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(&d.file_name).size(15.0).family(theme::semi()).color(p.text)).truncate());
    });
}

/// Name/value lines, like IDM's, as few as tell the whole story.
fn facts(ui: &mut Ui, p: &theme::Palette, d: &DownloadView) {
    let (status, color) = status_label(d, p);
    let done = match (d.total, d.fraction()) {
        (Some(t), Some(f)) => {
            format!("{} of {} ({:.1}%)", format_bytes(d.downloaded), format_bytes(t), (f * 1000.0).floor() / 10.0)
        }
        _ => format_bytes(d.downloaded),
    };
    let running = matches!(d.status, Status::Downloading | Status::Connecting);
    let speed = match (d.status, d.eta) {
        (Status::Completed, _) => format!("finished in {}", format_duration(d.active_secs.max(1))),
        (_, Some(eta)) if running => format!("{} · {} left", format_speed(d.speed), format_duration(eta)),
        _ if running => format_speed(d.speed),
        _ => "—".into(),
    };
    let rows: [(&str, String, egui::Color32); 4] = [
        ("Status", status, color),
        ("Downloaded", done, p.text),
        ("Speed", speed, if running { p.accent2 } else { p.text }),
        (
            "Resumable",
            if d.resumable { "Yes".into() } else { "No".into() },
            if d.resumable { p.green } else { p.yellow },
        ),
    ];
    egui::Grid::new(("progress-facts", d.id)).num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
        for (label, value, color) in &rows {
            ui.label(RichText::new(*label).color(p.text_dim));
            ui.label(RichText::new(value).color(*color));
            ui.end_row();
        }
    });
}

/// The wide progress bar.
fn big_bar(ui: &mut Ui, p: &theme::Palette, d: &DownloadView) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
    widgets::paint_progress(ui, rect, d, p);
}

/// One line per part: number, state, bytes.
fn parts_table(ui: &mut Ui, p: &theme::Palette, d: &DownloadView) {
    egui::ScrollArea::vertical().id_salt(("parts", d.id)).auto_shrink([false, true]).max_height(150.0).show(ui, |ui| {
        egui::Grid::new(("parts-grid", d.id)).num_columns(4).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
            for title in ["#", "State", "Downloaded", "Part size"] {
                ui.label(RichText::new(title).size(11.5).color(p.text_dim));
            }
            ui.end_row();
            for (i, s) in d.segments.iter().enumerate() {
                let len = s.end.saturating_sub(s.start);
                let done = s.done.min(len);
                let (state, color) = if done >= len {
                    ("Done", p.green)
                } else if s.active && d.status == Status::Downloading {
                    ("Receiving", p.accent2)
                } else if s.active {
                    ("Connecting", p.text_dim)
                } else {
                    ("Waiting", p.text_dim)
                };
                ui.label(RichText::new(format!("{}", i + 1)).color(p.text_dim));
                ui.label(RichText::new(state).color(color));
                ui.label(format_bytes(done));
                ui.label(if len == u64::MAX { "—".into() } else { format_bytes(len) });
                ui.end_row();
            }
        });
    });
}

fn buttons(ui: &mut Ui, p: &theme::Palette, d: &DownloadView, w: &mut ProgressWindow) -> Option<Action> {
    let mut action = None;
    if d.status == Status::Completed {
        let exists = file_exists(&d.path);
        if widgets::button(ui, ButtonKind::Primary, Some(Icon::External), "Open", exists, p).clicked() {
            action = Some(Action::Open);
        }
        if widgets::button(ui, ButtonKind::Tool, Some(Icon::FolderOpen), "Show in folder", true, p).clicked() {
            action = Some(Action::Folder);
        }
    } else {
        if d.status.can_pause() {
            if widgets::button(ui, ButtonKind::Primary, Some(Icon::Pause), "Pause", true, p).clicked() {
                action = Some(Action::Pause);
            }
        } else if d.status.can_resume()
            && widgets::button(ui, ButtonKind::Primary, Some(Icon::Play), "Resume", true, p).clicked()
        {
            action = Some(Action::Resume);
        }
        let label = if w.parts { "Hide parts" } else { "Show parts" };
        let arrow = if w.parts { Icon::CaretUp } else { Icon::CaretDown };
        if widgets::button(ui, ButtonKind::Tool, Some(arrow), label, !d.segments.is_empty(), p).clicked() {
            w.parts = !w.parts;
        }
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::button(ui, ButtonKind::Ghost, None, "Close", true, p).clicked() {
            action = Some(Action::Close);
        }
    });
    action
}
