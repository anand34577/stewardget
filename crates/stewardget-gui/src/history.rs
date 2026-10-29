//! History page: every finished download, searchable, even after it left the list.

use std::collections::{BTreeSet, HashMap};

use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use stewardget_core::util::{format_bytes, format_relative, format_speed, format_timestamp, now_unix};
use stewardget_core::{Category, HistoryEntry, NewDownload, Status};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

const ROW_H: f32 = 44.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Completed,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    #[default]
    Anytime,
    Week,
    Month,
    Year,
}

impl Period {
    fn seconds(self) -> Option<u64> {
        match self {
            Period::Anytime => None,
            Period::Week => Some(7 * 86_400),
            Period::Month => Some(30 * 86_400),
            Period::Year => Some(365 * 86_400),
        }
    }
}

/// State of the History page.
#[derive(Default)]
pub struct HistoryView {
    /// Engine generation the cached entries were read at.
    generation: u64,
    /// Newest first.
    entries: Vec<HistoryEntry>,
    pub query: String,
    pub status: StatusFilter,
    pub period: Period,
    pub category: Option<Category>,
    /// Ids of the rows that match, newest first, as indexes into `entries`.
    matching: Vec<usize>,
    /// What `matching` was computed for.
    matched_for: Option<(u64, String, StatusFilter, Period, Option<Category>)>,
    pub selected: BTreeSet<u64>,
    confirm_clear: bool,
    /// Mean speed per host over completed downloads, for "slower than usual".
    host_speed: HashMap<String, f64>,
}

impl HistoryView {
    /// Mean speed earlier downloads from `host` reached.
    pub fn host_average(&self, host: &str) -> Option<f64> {
        self.host_speed.get(host).copied()
    }

    /// Completed downloads per local hour of the day.
    pub fn hours(&self) -> [u32; 24] {
        let offset = stewardget_core::util::local_offset_secs(now_unix());
        let mut hours = [0u32; 24];
        for e in self.entries.iter().filter(|e| e.status == Status::Completed) {
            hours[((e.finished_at as i64 + offset).rem_euclid(86_400) / 3600) as usize] += 1;
        }
        hours
    }

    /// Hosts with the highest mean speed (at least two downloads each).
    pub fn fastest_hosts(&self, limit: usize) -> Vec<stewardget_core::HostSummary> {
        let mut log = stewardget_core::History::default();
        for e in &self.entries {
            log.record(e.clone());
        }
        let mut hosts = log.top_hosts(usize::MAX);
        hosts.retain(|h| h.downloads >= 2 && h.active_secs > 0);
        hosts.sort_by(|a, b| b.average_speed().total_cmp(&a.average_speed()));
        hosts.truncate(limit);
        hosts
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Re-reads the log when the engine says it changed.
    pub fn refresh(&mut self, engine: &stewardget_core::Engine) {
        let generation = engine.history_generation();
        if generation != self.generation || self.entries.is_empty() && generation == 0 {
            let mut entries = engine.history();
            entries.reverse();
            let mut sums: HashMap<&str, (u64, u64)> = HashMap::new();
            for e in entries.iter().filter(|e| e.status == Status::Completed && e.active_secs >= 5) {
                let s = sums.entry(e.host.as_str()).or_default();
                s.0 += e.size.unwrap_or(0);
                s.1 += e.active_secs;
            }
            self.host_speed = sums
                .into_iter()
                .filter(|(_, (_, secs))| *secs > 0)
                .map(|(h, (b, s))| (h.to_owned(), b as f64 / s as f64))
                .collect();
            self.entries = entries;
            self.generation = generation;
        }
    }

    fn rematch(&mut self) {
        let key = (self.generation, self.query.clone(), self.status, self.period, self.category);
        if self.matched_for.as_ref() == Some(&key) {
            return;
        }
        let now = now_unix();
        let cutoff = self.period.seconds().map(|s| now.saturating_sub(s));
        self.matching = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| match self.status {
                StatusFilter::All => true,
                StatusFilter::Completed => e.status == Status::Completed,
                StatusFilter::Failed => e.status == Status::Failed,
            })
            .filter(|(_, e)| cutoff.is_none_or(|c| e.finished_at >= c))
            .filter(|(_, e)| self.category.is_none_or(|c| e.category == c))
            .filter(|(_, e)| e.matches(&self.query))
            .map(|(i, _)| i)
            .collect();
        self.matched_for = Some(key);
    }
}

/// `name,url,...` CSV with every field quoted.
pub fn to_csv(entries: &[&HistoryEntry]) -> String {
    fn field(s: &str) -> String {
        // Spreadsheet formulas in a file name must not run when the file is opened.
        let guarded = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_owned() };
        format!("\"{}\"", guarded.replace('"', "\"\""))
    }
    let mut out = String::from("file,status,size_bytes,finished,seconds,avg_bytes_per_sec,host,url,folder,tags\n");
    for e in entries {
        let line = [
            field(&e.file_name),
            field(e.status.label()),
            e.size.map_or(String::new(), |s| s.to_string()),
            field(&format_timestamp(e.finished_at)),
            e.active_secs.to_string(),
            format!("{:.0}", e.average_speed()),
            field(&e.host),
            field(&e.url),
            field(&e.path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
            field(&e.tags.join(";")),
        ];
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

impl StewardGetApp {
    pub fn history_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        self.history.refresh(&self.engine);
        self.history.rematch();
        let now = now_unix();

        let shown: Vec<usize> = self.history.matching.clone();
        let (mut bytes, mut secs, mut ok) = (0u64, 0u64, 0usize);
        for &i in &shown {
            let e = &self.history.entries[i];
            if e.status == Status::Completed {
                ok += 1;
                bytes += e.size.unwrap_or(0);
                secs += e.active_secs;
            }
        }
        let mean = if secs > 0 { bytes as f64 / secs as f64 } else { 0.0 };

        egui::Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("History").size(18.0).family(crate::theme::semi()).color(p.text));
                ui.label(
                    RichText::new(format!("{} of {}", shown.len(), self.history.entries.len()))
                        .size(12.5)
                        .color(p.text_dim),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let any = !self.history.entries.is_empty();
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Trash), "Clear…", any, &p).clicked() {
                        self.history.confirm_clear = true;
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Export), "Export CSV", !shown.is_empty(), &p)
                        .clicked()
                    {
                        self.export_history(&shown);
                    }
                });
            });
            ui.add_space(5.0);

            ui.horizontal_wrapped(|ui| {
                let search = ui.add_sized(
                    [260.0, crate::theme::CONTROL_H],
                    widgets::input(&mut self.history.query).hint_text("Search name, site, folder or tag"),
                );
                if self.focus_search {
                    search.request_focus();
                    self.focus_search = false;
                }
                widgets::segmented(
                    ui,
                    &mut self.history.status,
                    &[
                        (StatusFilter::All, "All"),
                        (StatusFilter::Completed, "Completed"),
                        (StatusFilter::Failed, "Failed"),
                    ],
                    &p,
                );
                widgets::segmented(
                    ui,
                    &mut self.history.period,
                    &[
                        (Period::Anytime, "Any time"),
                        (Period::Week, "7 days"),
                        (Period::Month, "30 days"),
                        (Period::Year, "Year"),
                    ],
                    &p,
                );
                let label = self.history.category.map_or("All types".to_owned(), |c| self.cfg.category_label(c));
                egui::ComboBox::from_id_salt("history-category").selected_text(label).width(130.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.history.category, None, "All types");
                    for c in self.cfg.categories() {
                        let name = self.cfg.category_label(c);
                        ui.selectable_value(&mut self.history.category, Some(c), name);
                    }
                });
            });
            ui.add_space(5.0);

            let per_row = if ui.available_width() > 480.0 { 4 } else { 2 };
            let tiles = [
                ("Finished", ok.to_string(), p.green),
                ("Total size", format_bytes(bytes), p.accent),
                ("Average speed", format_speed(mean), p.accent2),
                ("Failed", shown.len().saturating_sub(ok).to_string(), p.red),
            ];
            for chunk in tiles.chunks(per_row) {
                ui.columns(per_row, |cols| {
                    for (col, (label, value, color)) in cols.iter_mut().zip(chunk) {
                        widgets::stat_tile(col, &p, label, value, "", *color);
                    }
                });
                ui.add_space(6.0);
            }
        });

        if self.history.entries.is_empty() {
            self.message_state(
                ui,
                Icon::History,
                "Nothing here yet",
                "Finished downloads are recorded here and stay, even after you clear them from the list.",
            );
            self.history_dialogs(ui.ctx());
            return;
        }
        if shown.is_empty() {
            self.message_state(ui, Icon::Search, "No matches", "Try a different word or widen the filters.");
            self.history_dialogs(ui.ctx());
            return;
        }

        let mut action: Option<RowAction> = None;
        egui::Frame::new().inner_margin(Margin::symmetric(18, 0)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, shown.len(), |ui, range| {
                for &i in &shown[range] {
                    let e = &self.history.entries[i];
                    if let Some(a) = history_row(ui, e, &p, now, self.history.selected.contains(&e.id)) {
                        action = Some(a);
                    }
                }
            });
        });

        if let Some(action) = action {
            self.apply_row_action(action, ui.ctx());
        }
        self.history_dialogs(ui.ctx());
    }

    fn message_state(&self, ui: &mut Ui, icon: Icon, title: &str, text: &str) {
        let p = self.palette;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(20.0));
            widgets::icon(ui, icon, 34.0, p.text_dim);
            ui.add_space(5.0);
            ui.label(RichText::new(title).size(16.0).family(crate::theme::semi()).color(p.text));
            ui.label(RichText::new(text).color(p.text_dim));
        });
    }

    fn export_history(&mut self, shown: &[usize]) {
        let rows: Vec<&HistoryEntry> = shown.iter().map(|&i| &self.history.entries[i]).collect();
        let csv = to_csv(&rows);
        let Some(path) = rfd::FileDialog::new().set_file_name("stewardget-history.csv").save_file() else { return };
        let accent = self.palette.accent2;
        let red = self.palette.red;
        match std::fs::write(&path, csv) {
            Ok(()) => self.toast(format!("Exported {} entries", rows.len()), accent),
            Err(e) => self.toast(format!("Couldn't write the file: {e}"), red),
        }
    }

    fn apply_row_action(&mut self, action: RowAction, ctx: &egui::Context) {
        let (accent, red) = (self.palette.accent2, self.palette.red);
        match action {
            RowAction::Open(path) => {
                if stewardget_core::util::open_with_system(&path).is_err() {
                    self.toast("The file is no longer there", red);
                }
            }
            RowAction::Reveal(path) => {
                let _ = stewardget_core::util::reveal_in_file_manager(&path);
            }
            RowAction::Again(url, file_name) => {
                let mut new = NewDownload::new(url);
                new.options.file_name = Some(file_name);
                let r = self.engine.add(new).map(|_| ());
                if r.is_ok() {
                    self.toast("Added to the download list", accent);
                }
                self.report(r);
            }
            RowAction::CopyLink(url) => {
                ctx.copy_text(url);
                self.toast("Link copied", accent);
            }
            RowAction::Forget(id) => {
                self.engine.remove_history(&[id]);
                self.history.selected.remove(&id);
            }
            RowAction::Select(id) => {
                if !self.history.selected.remove(&id) {
                    self.history.selected.insert(id);
                }
            }
        }
    }

    fn history_dialogs(&mut self, ctx: &egui::Context) {
        if !self.history.confirm_clear {
            return;
        }
        let p = self.palette;
        let mut open = true;
        egui::Window::new("Clear the history?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Every entry will be forgotten. Downloaded files and the download list are not touched.");
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, ButtonKind::Danger, Some(Icon::Trash), "Clear history", true, &p).clicked() {
                        self.engine.clear_history();
                        self.history.selected.clear();
                        self.history.confirm_clear = false;
                    }
                    if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                        self.history.confirm_clear = false;
                    }
                });
            });
        if !open {
            self.history.confirm_clear = false;
        }
    }
}

enum RowAction {
    Open(std::path::PathBuf),
    Reveal(std::path::PathBuf),
    Again(String, String),
    CopyLink(String),
    Forget(u64),
    Select(u64),
}

/// One history row; returns what the user clicked, if anything.
fn history_row(
    ui: &mut Ui,
    e: &HistoryEntry,
    p: &crate::theme::Palette,
    now: u64,
    selected: bool,
) -> Option<RowAction> {
    let mut action = None;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        p.accent.gamma_multiply(if p.dark { 0.18 } else { 0.1 })
    } else if response.hovered() {
        p.card_hover
    } else {
        p.card
    };
    painter.rect(rect, CornerRadius::same(10), fill, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);

    let color = p.category(e.category);
    let ic = egui::Rect::from_center_size(rect.left_center() + vec2(22.0, 0.0), vec2(26.0, 26.0));
    painter.rect_filled(ic, CornerRadius::same(8), color.gamma_multiply(0.14));
    crate::icons::paint(painter, ic.shrink(6.0), Icon::for_category(e.category), color);

    let missing = e.status == Status::Completed && !crate::table::file_exists(&e.path);
    // Five 30 px buttons at the right edge, then the size/speed and date columns.
    let wide = rect.width() > 700.0;
    let meta_x = rect.right() - 12.0 - 150.0 - 16.0 - 190.0;
    let text_left = rect.left() + 44.0;
    let text_right = if wide { meta_x - 14.0 } else { rect.right() - 12.0 - 150.0 - 8.0 }.max(text_left + 80.0);
    let name = ellipsize(painter, &e.file_name, egui::FontId::new(13.5, crate::theme::semi()), text_right - text_left);
    painter.text(
        egui::pos2(text_left, rect.top() + 13.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::new(13.5, crate::theme::semi()),
        p.text,
    );
    let sub = if e.status == Status::Failed {
        e.error.clone().unwrap_or_else(|| "Failed".into())
    } else if missing {
        format!("File moved or deleted  ·  {}", e.host)
    } else {
        let mut parts = vec![e.host.clone()];
        if !e.tags.is_empty() {
            parts.push(e.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
        }
        parts.retain(|s| !s.is_empty());
        parts.join("  ·  ")
    };
    let sub_color = if e.status == Status::Failed {
        p.red
    } else if missing {
        p.yellow
    } else {
        p.text_dim
    };
    let sub = ellipsize(painter, &sub, egui::FontId::proportional(12.0), text_right - text_left);
    painter.text(
        egui::pos2(text_left, rect.top() + 31.0),
        egui::Align2::LEFT_CENTER,
        sub,
        egui::FontId::proportional(12.0),
        sub_color,
    );

    // Size and speed, then when.
    if wide {
        painter.text(
            egui::pos2(meta_x, rect.top() + 13.0),
            egui::Align2::LEFT_CENTER,
            e.size.map_or("—".into(), format_bytes),
            egui::FontId::proportional(12.5),
            p.text,
        );
        let speed = if e.average_speed() > 0.0 { format_speed(e.average_speed()) } else { String::new() };
        painter.text(
            egui::pos2(meta_x, rect.top() + 31.0),
            egui::Align2::LEFT_CENTER,
            speed,
            egui::FontId::proportional(12.0),
            p.text_dim,
        );
        let when = format_relative(e.finished_at, now);
        painter.text(
            egui::pos2(meta_x + 84.0, rect.top() + 13.0),
            egui::Align2::LEFT_CENTER,
            when,
            egui::FontId::proportional(12.5),
            p.text_dim,
        );
    }

    // Row buttons, drawn from the right edge.
    let mut x = rect.right() - 12.0;
    let mut button = |ui: &mut Ui, icon: Icon, tip: &str, enabled: bool| -> bool {
        let r = egui::Rect::from_min_size(egui::pos2(x - 28.0, rect.center().y - 14.0), vec2(28.0, 28.0));
        x -= 30.0;
        let resp = ui.interact(r, response.id.with(tip), if enabled { Sense::click() } else { Sense::hover() });
        let c = if !enabled {
            p.text_dim.gamma_multiply(0.4)
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(7), p.track);
            p.accent
        } else {
            p.text_dim
        };
        crate::icons::paint(ui.painter(), r.shrink(6.0), icon, c);
        resp.on_hover_text(tip).clicked() && enabled
    };
    if button(ui, Icon::Close, "Forget this entry", true) {
        action = Some(RowAction::Forget(e.id));
    }
    if button(ui, Icon::Copy, "Copy link", true) {
        action = Some(RowAction::CopyLink(e.url.clone()));
    }
    if button(ui, Icon::Retry, "Download again", true) {
        action = Some(RowAction::Again(e.url.clone(), e.file_name.clone()));
    }
    if button(ui, Icon::FolderOpen, "Show in folder", !missing && e.status == Status::Completed) {
        action = Some(RowAction::Reveal(e.path.clone()));
    }
    if button(
        ui,
        Icon::External,
        if missing { "The file was moved or deleted" } else { "Open" },
        !missing && e.status == Status::Completed,
    ) {
        action = Some(RowAction::Open(e.path.clone()));
    }
    if action.is_none() && response.clicked() {
        action = Some(RowAction::Select(e.id));
    }
    action
}

/// `text` shortened with an ellipsis to fit `max_w` points.
fn ellipsize(painter: &egui::Painter, text: &str, font: egui::FontId, max_w: f32) -> String {
    let width = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if width(&candidate) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_and_defuses_formulas() {
        let e = HistoryEntry {
            id: 1,
            url: "https://h/x".into(),
            host: "h".into(),
            file_name: "=cmd|\"x\".csv".into(),
            path: "/d/x.csv".into(),
            category: Category::Other,
            size: Some(5),
            status: Status::Completed,
            error: None,
            added_at: 0,
            finished_at: 0,
            active_secs: 1,
            connections: 4,
            checksum_verified: false,
            tags: vec!["a".into(), "b".into()],
        };
        let csv = to_csv(&[&e]);
        let row = csv.lines().nth(1).unwrap();
        assert!(row.starts_with("\"'=cmd|\"\"x\"\".csv\""), "{row}");
        assert!(row.contains("\"a;b\""));
    }
}
