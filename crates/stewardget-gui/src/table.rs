//! The main download table: sortable, responsive columns, virtualised rows,
//! multi-selection and a context menu.

use std::cmp::Ordering;

use egui::{Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_speed, format_timestamp};
use stewardget_core::{DownloadId, DownloadView, Status};

use crate::app::{DraggedDownloads, ListFilter, StewardGetApp, menu_item};
use crate::icons::{self, Icon};
use crate::theme::Palette;
use crate::widgets::{self, ButtonKind};

/// Row height in compact mode, and with name plus details.
const ROW_H_COMPACT: f32 = 28.0;
const ROW_H_COMFORTABLE: f32 = 40.0;
const HEADER_H: f32 = 30.0;
const PAD_X: f32 = 14.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Queue,
    Name,
    Size,
    Progress,
    Speed,
    Eta,
    Status,
    Added,
}

pub struct TableState {
    pub sort: SortKey,
    pub ascending: bool,
}

impl TableState {
    /// Sorts positions in `all`.
    pub fn sort(&self, rows: &mut [usize], all: &[DownloadView]) {
        let key = self.sort;
        if key == SortKey::Queue {
            if !self.ascending {
                rows.reverse();
            }
            return;
        }
        if key == SortKey::Name {
            // Lower-cased once per row, not once per comparison.
            rows.sort_by_cached_key(|&i| all[i].file_name.to_lowercase());
            if !self.ascending {
                rows.reverse();
            }
            return;
        }
        rows.sort_by(|&a, &b| {
            let (a, b) = (&all[a], &all[b]);
            let ord = match key {
                SortKey::Queue | SortKey::Name => Ordering::Equal,
                SortKey::Size => a.total.cmp(&b.total),
                SortKey::Progress => a.fraction().unwrap_or(0.0).total_cmp(&b.fraction().unwrap_or(0.0)),
                SortKey::Speed => a.speed.total_cmp(&b.speed),
                SortKey::Eta => a.eta.unwrap_or(u64::MAX).cmp(&b.eta.unwrap_or(u64::MAX)),
                SortKey::Status => status_rank(a.status).cmp(&status_rank(b.status)),
                SortKey::Added => a.added_at.cmp(&b.added_at),
            };
            if self.ascending { ord } else { ord.reverse() }
        });
    }
}

fn status_rank(s: Status) -> u8 {
    match s {
        Status::Downloading => 0,
        Status::Connecting => 1,
        Status::Verifying => 2,
        Status::Queued => 3,
        Status::Paused => 4,
        Status::Failed => 5,
        Status::Completed => 6,
    }
}

#[derive(Clone, Copy)]
struct Column {
    key: SortKey,
    title: &'static str,
    width: f32,
    right: bool,
}

/// Optional columns the user can switch off (right-click the header): key in
/// [`stewardget_core::Config::hidden_columns`], title.
pub const OPTIONAL_COLUMNS: [(&str, &str); 4] =
    [("size", "Size"), ("speed", "Speed"), ("eta", "Time left"), ("added", "Added")];

fn column_key(key: SortKey) -> &'static str {
    match key {
        SortKey::Size => "size",
        SortKey::Speed => "speed",
        SortKey::Eta => "eta",
        SortKey::Added => "added",
        _ => "",
    }
}

/// Picks the columns that fit, in priority order, and gives the rest to Name.
fn layout_columns(width: f32, hidden: &[String]) -> (f32, Vec<Column>) {
    let progress = Column {
        key: SortKey::Progress,
        title: "Progress",
        width: if width < 760.0 { 120.0 } else { 170.0 },
        right: false,
    };
    let status = Column { key: SortKey::Status, title: "Status", width: 128.0, right: false };
    let optional = [
        Column { key: SortKey::Size, title: "Size", width: 86.0, right: true },
        Column { key: SortKey::Speed, title: "Speed", width: 96.0, right: true },
        Column { key: SortKey::Eta, title: "Time left", width: 78.0, right: true },
        Column { key: SortKey::Added, title: "Added", width: 124.0, right: false },
    ];
    let min_name = 220.0;
    let mut used = progress.width + status.width + PAD_X * 2.0;
    let mut picked: Vec<Column> = Vec::new();
    for c in optional {
        if hidden.iter().any(|h| h == column_key(c.key)) {
            continue;
        }
        if width - used - c.width >= min_name {
            used += c.width;
            picked.push(c);
        }
    }
    // Fixed display order.
    let mut cols = Vec::new();
    let has = |k: SortKey| picked.iter().find(|c| c.key == k).copied();
    cols.extend(has(SortKey::Size));
    cols.push(progress);
    cols.extend(has(SortKey::Speed));
    cols.extend(has(SortKey::Eta));
    cols.push(status);
    cols.extend(has(SortKey::Added));
    ((width - used).max(120.0), cols)
}

impl StewardGetApp {
    pub fn downloads_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let rows = self.visible_rows();
        let full = ui.available_rect_before_wrap();
        let (name_w, cols) = layout_columns(full.width(), &self.cfg.hidden_columns);
        let row_h = if self.cfg.compact_rows { ROW_H_COMPACT } else { ROW_H_COMFORTABLE };

        // Title strip
        let (strip, _) = ui.allocate_exact_size(vec2(full.width(), 40.0), Sense::hover());
        let painter = ui.painter();
        let title = self.filter.title(&self.cfg);
        painter.text(
            strip.left_center() + vec2(PAD_X, 0.0),
            Align2::LEFT_CENTER,
            title,
            FontId::new(16.0, crate::theme::semi()),
            p.text,
        );
        let summary = {
            let total: u64 = rows.iter().filter_map(|&i| self.snap.downloads[i].total).sum();
            let noun = if rows.len() == 1 { "item" } else { "items" };
            let mut s = format!("{} {noun} · {}", rows.len(), format_bytes(total));
            if self.selection.len() > 1 {
                s = format!("{} selected · {s}", self.selection.len());
            }
            s
        };
        painter.text(
            strip.right_center() - vec2(PAD_X, 0.0),
            Align2::RIGHT_CENTER,
            summary,
            FontId::proportional(12.0),
            p.text_dim,
        );

        if self.filter == ListFilter::Failed {
            let retryable: Vec<DownloadId> = rows
                .iter()
                .map(|&i| &self.snap.downloads[i])
                .filter(|d| matches!(problem_of(d), Some(Problem::Other | Problem::DiskFull)))
                .map(|d| d.id)
                .collect();
            if !retryable.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(PAD_X);
                    let label = format!("Retry {} that can be retried", retryable.len());
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::Retry), &label, true, &p).clicked() {
                        for id in retryable {
                            let r = self.engine.resume(id);
                            self.report(r);
                        }
                    }
                });
                ui.add_space(4.0);
            }
        }

        // Header
        let (header, header_r) = ui.allocate_exact_size(vec2(full.width(), HEADER_H), Sense::click());
        ui.painter().rect_filled(header, CornerRadius::ZERO, p.sidebar);
        ui.painter().line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, p.border));
        let mut x = header.left() + PAD_X;
        let name_col = Column { key: SortKey::Name, title: "Name", width: name_w, right: false };
        for col in std::iter::once(name_col).chain(cols.iter().copied()) {
            let cell = Rect::from_min_size(pos2(x, header.top()), vec2(col.width, HEADER_H));
            self.header_cell(ui, cell, col);
            x += col.width;
        }
        header_r.context_menu(|ui| self.columns_menu(ui));

        if rows.is_empty() {
            self.empty_state(ui);
            return;
        }

        let order: Vec<DownloadId> = rows.iter().map(|&i| self.snap.downloads[i].id).collect();
        let mut area = egui::ScrollArea::vertical().id_salt("stewardget-table").auto_shrink([false, false]);
        // Keyboard navigation: bring the row with the cursor into view.
        if let Some(target) = self.scroll_to.take()
            && let Some(i) = order.iter().position(|&id| id == target)
        {
            let stride = row_h + ui.spacing().item_spacing.y;
            let (offset, view_h) = self.table_view;
            let (top, bottom) = (i as f32 * stride, i as f32 * stride + row_h);
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if bottom > offset + view_h {
                area = area.vertical_scroll_offset(bottom - view_h);
            }
        }
        let out = area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for i in range {
                // Only the rows on screen are copied.
                let d = self.snap.downloads[rows[i]].clone();
                self.row(ui, &order, i, &d, name_w, &cols, row_h);
            }
        });
        self.table_view = (out.state.clamped_offset().y, out.inner_rect.height());
    }

    fn header_cell(&mut self, ui: &mut Ui, cell: Rect, col: Column) {
        let p = self.palette;
        let response = ui.interact(cell, egui::Id::unique(("stewardget-col", col.title)), Sense::click());
        let active = self.table.sort == col.key;
        let color = if active || response.hovered() { p.text } else { p.text_dim };
        let (anchor, pos) = if col.right {
            (Align2::RIGHT_CENTER, cell.right_center() - vec2(10.0, 0.0))
        } else {
            (Align2::LEFT_CENTER, cell.left_center())
        };
        let text_rect = ui.painter().text(pos, anchor, col.title, FontId::proportional(12.0), color);
        if active {
            let caret = if self.table.ascending { Icon::CaretUp } else { Icon::CaretDown };
            let at = if col.right {
                text_rect.left_center() - vec2(10.0, 0.0)
            } else {
                text_rect.right_center() + vec2(8.0, 0.0)
            };
            icons::paint(ui.painter(), Rect::from_center_size(at, vec2(10.0, 10.0)), caret, p.accent);
        }
        if response.clicked() {
            if active {
                if !self.table.ascending && col.key != SortKey::Queue {
                    // third click returns to queue order
                    self.table.sort = SortKey::Queue;
                    self.table.ascending = true;
                } else {
                    self.table.ascending = !self.table.ascending;
                }
            } else {
                self.table.sort = col.key;
                self.table.ascending = !matches!(col.key, SortKey::Size | SortKey::Speed | SortKey::Added);
            }
        }
        // The column chooser opens from any header cell, not only the gaps between them.
        response.context_menu(|ui| self.columns_menu(ui));
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    /// Right-click menu of the header: which optional columns to show, and row density.
    fn columns_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(200.0);
        let mut config = self.engine.config();
        let mut changed = false;
        ui.label(RichText::new("Columns").size(12.0).color(self.palette.text_dim));
        for (key, title) in OPTIONAL_COLUMNS {
            let mut shown = !config.hidden_columns.iter().any(|h| h == key);
            if ui.checkbox(&mut shown, title).changed() {
                config.hidden_columns.retain(|h| h != key);
                if !shown {
                    config.hidden_columns.push(key.to_owned());
                }
                changed = true;
            }
        }
        ui.separator();
        changed |= ui.checkbox(&mut config.compact_rows, "Compact rows").changed();
        if changed {
            // An open Settings page must not put the old columns back when saved.
            if let Some(draft) = &mut self.settings_draft {
                draft.hidden_columns = config.hidden_columns.clone();
                draft.compact_rows = config.compact_rows;
            }
            let r = self.engine.update_config(config);
            self.report(r);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut Ui,
        order: &[DownloadId],
        index: usize,
        d: &DownloadView,
        name_w: f32,
        cols: &[Column],
        row_h: f32,
    ) {
        let p = self.palette;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
        let response = ui.interact(rect, egui::Id::unique(("stewardget-row", d.id)), Sense::click_and_drag());
        // Dragging a row onto a queue in the sidebar moves it (with the rest of the selection).
        if response.drag_started() {
            let ids =
                if self.selection.contains(&d.id) { self.selection.iter().copied().collect() } else { vec![d.id] };
            egui::DragAndDrop::set_payload(ui.ctx(), DraggedDownloads(ids));
        }
        let selected = self.selection.contains(&d.id);
        let painter = ui.painter().clone();
        if selected {
            painter.rect_filled(rect, CornerRadius::ZERO, p.accent.gamma_multiply(0.17));
            painter.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), CornerRadius::ZERO, p.accent);
        } else if response.hovered() {
            painter.rect_filled(rect, CornerRadius::ZERO, p.card_hover);
        } else if index % 2 == 1 {
            painter.rect_filled(rect, CornerRadius::ZERO, p.card.gamma_multiply(0.45));
        }

        // Name cell
        let mut x = rect.left() + PAD_X;
        let compact = row_h <= ROW_H_COMPACT;
        let icon_size = if compact { 16.0 } else { 20.0 };
        let icon_rect = Rect::from_center_size(pos2(x + 9.0, rect.center().y), vec2(icon_size, icon_size));
        icons::paint(&painter, icon_rect, Icon::for_category(d.category), p.category(d.category));
        let name_rect = Rect::from_min_max(pos2(x + 30.0, rect.top()), pos2(x + name_w - 12.0, rect.bottom()));
        let clip = painter.with_clip_rect(name_rect);
        if compact {
            clip.text(name_rect.left_center(), Align2::LEFT_CENTER, &d.file_name, FontId::proportional(13.5), p.text);
        } else {
            // Name, and below it what kind of file it is and where it comes from.
            let c = name_rect.left_center();
            clip.text(c - vec2(0.0, 8.5), Align2::LEFT_CENTER, &d.file_name, FontId::proportional(13.5), p.text);
            let host = crate::capture::host_of(&d.url);
            let mut sub = self.cfg.category_label(d.category);
            if !host.is_empty() {
                sub = format!("{sub} \u{00b7} {host}");
            }
            clip.text(c + vec2(0.0, 9.5), Align2::LEFT_CENTER, sub, FontId::proportional(12.0), p.text_dim);
        }
        x += name_w;

        for col in cols {
            let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(col.width, row_h));
            // Nothing transferred and no size known: quiet cells instead of a dash and an empty bar.
            let untouched = d.downloaded == 0 && d.total.is_none() && !d.status.is_active();
            let right = cell.right_center() - vec2(10.0, 0.0);
            let small = FontId::proportional(12.5);
            match col.key {
                SortKey::Size => {
                    let text = match d.total {
                        Some(t) => format_bytes(t),
                        None if d.downloaded > 0 => format_bytes(d.downloaded),
                        None => String::new(),
                    };
                    painter.text(right, Align2::RIGHT_CENTER, text, small, p.text_dim);
                }
                SortKey::Progress if untouched => {}
                SortKey::Progress => {
                    let bar =
                        Rect::from_min_size(pos2(cell.left(), cell.center().y - 2.5), vec2(cell.width() - 52.0, 5.0));
                    widgets::paint_progress(ui, bar, d, &p);
                    let pct = d.fraction().map_or("—".into(), |f| {
                        if f >= 1.0 { "100%".into() } else { format!("{:.1}%", (f * 1000.0).floor() / 10.0) }
                    });
                    painter.text(cell.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER, pct, small, p.text);
                }
                SortKey::Speed => {
                    if d.status == Status::Downloading {
                        painter.text(right, Align2::RIGHT_CENTER, format_speed(d.speed), small, p.accent2);
                    }
                }
                SortKey::Eta => {
                    if let Some(eta) = d.eta {
                        painter.text(right, Align2::RIGHT_CENTER, format_duration(eta), small, p.text_dim);
                    }
                }
                SortKey::Status => {
                    let (label, color) = status_label(d, &p);
                    widgets::dot(&painter, cell.left_center() + vec2(14.0, 0.0), color);
                    painter.with_clip_rect(cell).text(
                        cell.left_center() + vec2(24.0, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        small,
                        color,
                    );
                }
                SortKey::Added => {
                    let now = stewardget_core::util::now_unix();
                    painter.text(
                        cell.left_center() + vec2(4.0, 0.0),
                        Align2::LEFT_CENTER,
                        stewardget_core::util::format_relative(d.added_at, now),
                        small,
                        p.text_dim,
                    );
                    if response.hovered() && ui.rect_contains_pointer(cell) {
                        response.clone().on_hover_text(format_timestamp(d.added_at));
                    }
                }
                SortKey::Name | SortKey::Queue => {}
            }
            x += col.width;
        }

        // Interaction
        if response.clicked() || response.secondary_clicked() && !selected {
            self.cursor = Some(d.id);
            let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
            if shift && let Some(anchor) = self.anchor {
                let a = order.iter().position(|&id| id == anchor).unwrap_or(index);
                let (lo, hi) = (a.min(index), a.max(index));
                if !ctrl {
                    self.selection.clear();
                }
                self.selection.extend(order[lo..=hi].iter().copied());
            } else if ctrl {
                if !self.selection.remove(&d.id) {
                    self.selection.insert(d.id);
                }
                self.anchor = Some(d.id);
            } else {
                self.selection.clear();
                self.selection.insert(d.id);
                self.anchor = Some(d.id);
            }
        }
        if response.double_clicked() {
            // Like IDM: open the finished file, or watch the download in its own window.
            if d.status == Status::Completed {
                let _ = stewardget_core::util::open_with_system(&d.path);
            } else {
                self.open_progress(d.id);
            }
        }
        let d = d.clone();
        response.context_menu(|ui| self.context_menu(ui, &d));
    }

    fn context_menu(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        ui.set_min_width(230.0);
        let many = self.selection.len() > 1;
        let exists = file_exists(&d.path);
        if !many {
            if menu_item(ui, &p, Icon::External, "Open", d.status == Status::Completed && exists) {
                let _ = stewardget_core::util::open_with_system(&d.path);
                ui.close();
            }
            if menu_item(ui, &p, Icon::FolderOpen, "Show in folder", true) {
                self.reveal(&d.path);
                ui.close();
            }
            ui.separator();
        }
        let sel = self.selected_views();
        let any_resume = sel.iter().any(|s| s.status.can_resume());
        let any_pause = sel.iter().any(|s| s.status.can_pause());
        if menu_item(ui, &p, Icon::Play, "Resume  Space", any_resume) {
            self.resume_selected();
            ui.close();
        }
        if menu_item(ui, &p, Icon::Pause, "Pause  Space", any_pause) {
            self.pause_selected();
            ui.close();
        }
        if !many
            && d.status == Status::Failed
            && problem_of(d) == Some(Problem::Changed)
            && menu_item(ui, &p, Icon::Play, "Resume anyway", true)
        {
            let r = self.engine.resume_anyway(d.id);
            self.report(r);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Retry, "Start over…", true) {
            self.restart_selected();
            ui.close();
        }
        if !many {
            if menu_item(ui, &p, Icon::Link, "Refresh link…", d.status != Status::Completed) {
                self.open_refresh(d);
                ui.close();
            }
            if d.status != Status::Completed && menu_item(ui, &p, Icon::Eye, "Progress window", true) {
                self.open_progress(d.id);
                ui.close();
            }
            if menu_item(ui, &p, Icon::Copy, "Copy link", true) {
                ui.ctx().copy_text(d.url.clone());
                ui.close();
            }
            if menu_item(ui, &p, Icon::Copy, "Copy file path", true) {
                ui.ctx().copy_text(d.path.display().to_string());
                ui.close();
            }
            ui.separator();
            if menu_item(ui, &p, Icon::ArrowUp, "Move up in queue", true) {
                let r = self.engine.reorder(d.id, -1);
                self.report(r);
                ui.close();
            }
            if menu_item(ui, &p, Icon::ArrowDown, "Move down in queue", true) {
                let r = self.engine.reorder(d.id, 1);
                self.report(r);
                ui.close();
            }
            if menu_item(ui, &p, Icon::CaretUp, "Move to top of queue", true) {
                let r = self.engine.move_to_end(d.id, true);
                self.report(r);
                ui.close();
            }
            if menu_item(ui, &p, Icon::CaretDown, "Move to bottom of queue", true) {
                let r = self.engine.move_to_end(d.id, false);
                self.report(r);
                ui.close();
            }
        }
        self.move_to_queue_menu(ui, d);
        if !many && menu_item(ui, &p, Icon::Sliders, "Properties…  F2", true) {
            self.open_properties(d);
            ui.close();
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Trash, "Remove from list  Del", true) {
            self.remove_selected(false);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Trash, "Remove and delete files", true) {
            self.remove_selected(true);
            ui.close();
        }
    }

    /// "Move to queue" submenu for the selection.
    fn move_to_queue_menu(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        let ids: Vec<_> =
            if self.selection.contains(&d.id) { self.selection.iter().copied().collect() } else { vec![d.id] };
        let config = self.engine.config();
        ui.menu_button("     Move to queue", |ui| {
            ui.set_min_width(200.0);
            for q in &config.queues {
                let here = ids.iter().all(|id| self.snap.downloads.iter().any(|v| v.id == *id && v.queue == q.id));
                if ui.selectable_label(here, &q.name).clicked() {
                    let r = self.engine.set_queue(&ids, q.id);
                    self.report(r);
                    ui.close();
                }
            }
            ui.separator();
            if menu_item(ui, &p, Icon::Plus, "New queue…", true) {
                self.queue_dialog = Some(crate::queues::QueueDialog::create(ids.clone()));
                ui.close();
            }
        });
    }

    fn empty_state(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            widgets::icon(ui, Icon::Download, 40.0, p.accent.gamma_multiply(0.8));
            ui.add_space(6.0);
            let (title, sub) = if self.snap.downloads.is_empty() {
                (
                    "No downloads yet",
                    "Click Add URL, paste a link (Ctrl+V), copy a file link anywhere, or download from your browser with the StewardGet extension.",
                )
            } else if !self.search.is_empty() {
                ("No matches", "Try a different search.")
            } else if matches!(self.filter, ListFilter::Queue(_)) {
                (
                    "This queue is empty",
                    "Drag downloads onto it in the sidebar, choose Move to queue in their menu, or pick it when you add a download.",
                )
            } else if self.filter == ListFilter::Failed {
                ("Nothing has failed", "Downloads that stop with an error show up here, with a way to fix them.")
            } else if self.filter == ListFilter::Completed {
                ("Nothing finished yet", "Finished downloads stay here until you clear them. History keeps a permanent log.")
            } else if self.filter == ListFilter::Active {
                ("Nothing is downloading", "Queued and paused downloads are under Queued & paused.")
            } else {
                ("Nothing here", "Downloads in this view will appear here.")
            };
            ui.label(RichText::new(title).size(16.0).family(crate::theme::semi()).color(p.text));
            ui.add(egui::Label::new(RichText::new(sub).color(p.text_dim)).wrap());
            ui.add_space(6.0);
            if self.snap.downloads.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - 420.0) / 2.0).max(0.0));
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Plus), "Add URL", true, &p).clicked() {
                        self.open_add(None);
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Link), "Connect browser", true, &p).clicked() {
                        self.open_settings(crate::settings::Tab::Browser);
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Import links", true, &p).clicked() {
                        self.import_links();
                    }
                });
            } else if !self.search.is_empty() {
                if widgets::button(ui, ButtonKind::Ghost, None, "Clear search", true, &p).clicked() {
                    self.search.clear();
                }
            } else if self.filter != ListFilter::All
                && widgets::button(ui, ButtonKind::Ghost, None, "Show all downloads", true, &p).clicked()
            {
                self.filter = ListFilter::All;
            }
        });
    }
}

/// Whether a file exists, re-checked at most every few seconds. Rows and the
/// details pane ask every frame, and on a network drive each check can stall.
pub fn file_exists(path: &std::path::Path) -> bool {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    thread_local! {
        static SEEN: RefCell<HashMap<std::path::PathBuf, (Instant, bool)>> = RefCell::new(HashMap::new());
    }
    SEEN.with_borrow_mut(|seen| {
        if let Some(&(at, exists)) = seen.get(path)
            && at.elapsed() < Duration::from_secs(3)
        {
            return exists;
        }
        if seen.len() > 4096 {
            seen.clear();
        }
        let exists = path.exists();
        seen.insert(path.to_path_buf(), (Instant::now(), exists));
        exists
    })
}

/// Free space for a folder field, re-read at most every two seconds per path
/// (dialogs ask every frame while they are open).
pub fn free_space(path: &std::path::Path) -> Option<u64> {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};
    thread_local! {
        static LAST: RefCell<Option<(std::path::PathBuf, Instant, Option<u64>)>> = const { RefCell::new(None) };
    }
    LAST.with_borrow_mut(|last| {
        if let Some((p, at, free)) = last.as_ref()
            && p == path
            && at.elapsed() < Duration::from_secs(2)
        {
            return *free;
        }
        let free = stewardget_core::util::free_space(path);
        *last = Some((path.to_path_buf(), Instant::now(), free));
        free
    })
}

pub fn status_label(d: &DownloadView, p: &Palette) -> (String, Color32) {
    if d.awaiting_link {
        return ("Waiting for link".into(), p.yellow);
    }
    if let Some(secs) = d.retry_in {
        return (format!("Retry in {}", format_duration(secs)), p.yellow);
    }
    if d.status == Status::Downloading && d.connections > 0 && d.reconnecting >= d.connections {
        return ("Reconnecting…".into(), p.yellow);
    }
    let color = p.status(d.status);
    let label = match d.status {
        Status::Downloading if d.connections > 0 => format!("Downloading \u{00d7}{}", d.connections),
        Status::Completed if !file_exists(&d.path) => return ("File missing".into(), p.yellow),
        Status::Failed => match problem_of(d) {
            Some(Problem::Expired) => "Link expired?".into(),
            Some(Problem::NotFound) => "Not found".into(),
            Some(Problem::Denied) => "Access denied".into(),
            Some(Problem::Changed | Problem::SizeChanged) => "File changed".into(),
            Some(Problem::DiskFull) => "Disk full".into(),
            Some(Problem::NoResume) => "Can't resume".into(),
            Some(Problem::Checksum) => "Checksum failed".into(),
            _ => "Failed".into(),
        },
        s => s.label().into(),
    };
    (label, color)
}

/// What kind of failure an error message describes, to offer the right fix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    /// Usually fixed by refreshing the link.
    Expired,
    /// 404/410 before any data arrived: probably a wrong link.
    NotFound,
    /// 401/403 before any data arrived: the site wants a login or cookies.
    Denied,
    /// Same size, different validators: resume anyway or start over.
    Changed,
    SizeChanged,
    NoResume,
    DiskFull,
    Checksum,
    Other,
}

/// [`problem`], refined with the download's progress.
pub fn problem_of(d: &DownloadView) -> Option<Problem> {
    let error = d.error.as_deref()?;
    Some(match problem(error) {
        Problem::Expired if d.downloaded == 0 && (error.contains("HTTP 401") || error.contains("HTTP 403")) => {
            Problem::Denied
        }
        Problem::Expired if d.downloaded == 0 => Problem::NotFound,
        p => p,
    })
}

/// One plain sentence on what to do about a failure; empty when there is nothing useful to add.
pub fn problem_hint(kind: Problem, error: &str) -> &'static str {
    match kind {
        Problem::Expired => "The link has probably expired. Refresh it to continue from where it stopped.",
        Problem::NotFound => "Check that the link is right, or open the page and start the download again.",
        Problem::Denied => {
            "The site wants you to be logged in. Start the download from your browser so StewardGet gets its cookies, or refresh the link."
        }
        Problem::Changed => {
            "Your partial data is kept. If it is still the same file (e.g. a re-signed link), resume anyway; otherwise start over."
        }
        Problem::SizeChanged => "This looks like a different file now. Refresh the link, or start over.",
        Problem::NoResume => "The partial data can't be continued from this server. Try a fresh link, or start over.",
        Problem::DiskFull => "Free up space or move the download to another drive, then retry.",
        Problem::Checksum => "The file is corrupt or the checksum is wrong. Start over, or edit the checksum.",
        Problem::Other => {
            if error.contains("timed out") || error.contains("network error") {
                "Check your internet connection (or proxy settings), then press Resume."
            } else if error.contains("server is busy") {
                "The server asked for a pause. StewardGet will try again by itself."
            } else if error.contains("disk error") {
                "Check that the folder exists, is writable and the drive is connected."
            } else if error.contains("invalid URL") {
                "The link looks incomplete. Edit it in Properties (F2)."
            } else {
                ""
            }
        }
    }
}

pub fn problem(error: &str) -> Problem {
    if error.contains("remote file changed") {
        Problem::Changed
    } else if error.contains("different file size") {
        Problem::SizeChanged
    } else if error.contains("no longer supports resuming") || error.contains("ignored the byte-range") {
        Problem::NoResume
    } else if error.contains("disk space") {
        Problem::DiskFull
    } else if error.contains("checksum") {
        Problem::Checksum
    } else if ["HTTP 401", "HTTP 403", "HTTP 404", "HTTP 410"].iter().any(|e| error.contains(e)) {
        Problem::Expired
    } else {
        Problem::Other
    }
}
