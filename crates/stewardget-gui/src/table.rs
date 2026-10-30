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

/// Row height of the list view.
const ROW_H_COMPACT: f32 = 34.0;
const HEADER_H: f32 = 34.0;
/// Card view: card height, the gap below each card and the side margin.
const CARD_H: f32 = 90.0;
const CARD_GAP: f32 = 10.0;
const CARD_X: f32 = 20.0;
/// Rows are drawn as rounded bands this far in from the canvas edge.
const ROW_INSET: f32 = 10.0;
const PAD_X: f32 = 14.0 + ROW_INSET;

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
    let status = Column { key: SortKey::Status, title: "Status", width: 150.0, right: false };
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
        let cards = !self.cfg.compact_rows;
        let row_h = if cards { CARD_H + CARD_GAP } else { ROW_H_COMPACT };

        // Page header: title and summary, and on the right what can be done
        // with the selection (or with everything in view when nothing is selected).
        let (strip, _) = ui.allocate_exact_size(vec2(full.width(), 76.0), Sense::hover());
        let title = self.filter.title(&self.cfg);
        let summary = {
            let total: u64 = rows.iter().filter_map(|&i| self.snap.downloads[i].total).sum();
            let noun = if rows.len() == 1 { "download" } else { "downloads" };
            let active = rows.iter().filter(|&&i| self.snap.downloads[i].status.is_active()).count();
            let mut s = format!("{} {noun} \u{00b7} {}", rows.len(), format_bytes(total));
            if active > 0 {
                s = format!("{s} \u{00b7} {active} active");
            }
            s
        };
        let compact = full.width() < 860.0;
        let title_w = {
            let painter = ui.painter();
            let left = strip.left() + CARD_X + 4.0;
            let t = painter.text(
                egui::pos2(left, strip.center().y - 10.0),
                Align2::LEFT_CENTER,
                title,
                FontId::new(crate::theme::size::XL + 2.0, crate::theme::semi()),
                p.text,
            );
            let s = painter.text(
                egui::pos2(left, strip.center().y + 14.0),
                Align2::LEFT_CENTER,
                summary,
                FontId::proportional(crate::theme::size::SM),
                p.text_dim,
            );
            t.width().max(s.width()) + PAD_X + 30.0
        };
        let actions = egui::Rect::from_min_max(
            egui::pos2(strip.left() + title_w, strip.top()),
            egui::pos2(strip.right() - CARD_X, strip.bottom()),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(actions).layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                self.view_toggle(ui);
                ui.add_space(6.0);
                self.selection_actions(ui, compact);
            },
        );

        // Live figures for everything in the list; skipped when the window is short.
        if cards && full.height() > 600.0 {
            self.kpi_strip(ui, full.width());
            ui.add_space(14.0);
        }

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

        if cards {
            self.sort_bar(ui, rows.len());
        } else {
            let (header, header_r) = ui.allocate_exact_size(vec2(full.width(), HEADER_H), Sense::click());
            ui.painter().line_segment(
                [header.left_bottom() + vec2(ROW_INSET, 0.0), header.right_bottom() - vec2(ROW_INSET, 0.0)],
                Stroke::new(1.0, p.border),
            );
            let mut x = header.left() + PAD_X;
            let name_col = Column { key: SortKey::Name, title: "Name", width: name_w, right: false };
            for col in std::iter::once(name_col).chain(cols.iter().copied()) {
                let cell = Rect::from_min_size(pos2(x, header.top()), vec2(col.width, HEADER_H));
                self.header_cell(ui, cell, col);
                x += col.width;
            }
            header_r.context_menu(|ui| self.columns_menu(ui));
        }

        if rows.is_empty() {
            self.empty_state(ui);
            return;
        }

        // Rows touch; their bands and hairlines do the separating.
        ui.spacing_mut().item_spacing.y = 0.0;
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
                if cards {
                    self.card_row(ui, &order, i, &d);
                } else {
                    self.row(ui, &order, i, &d, name_w, &cols, row_h);
                }
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
        let galley = widgets::caps_galley(ui, &col.title.to_uppercase(), 10.5, color);
        let text_rect = anchor.anchor_size(pos, galley.size());
        ui.painter().galley(text_rect.min, galley, color);
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

    /// Right-click menu of the header: which optional columns to show, and cards or list.
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
        changed |= ui.checkbox(&mut config.compact_rows, "List view").changed();
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
        let band = rect.shrink2(vec2(ROW_INSET, 1.0));
        let corner = CornerRadius::same(crate::theme::radius::MD);
        let is_cursor = self.cursor == Some(d.id);
        if selected {
            painter.rect_filled(band, corner, p.selected_fill());
            if is_cursor && self.selection.len() > 1 {
                painter.rect_stroke(
                    band,
                    corner,
                    Stroke::new(1.0, p.accent.gamma_multiply(0.6)),
                    egui::StrokeKind::Inside,
                );
            }
        } else if response.hovered() {
            painter.rect_filled(band, corner, p.card_hover);
        } else if index > 0 && !self.selection.contains(&order[index - 1]) {
            // Hairline between unselected rows, kept short of the edges.
            painter.line_segment(
                [band.left_top() + vec2(PAD_X - ROW_INSET + 40.0, -1.0), band.right_top() - vec2(12.0, 1.0)],
                Stroke::new(1.0, p.border.gamma_multiply(0.55)),
            );
        }

        // Name cell
        let mut x = rect.left() + PAD_X;
        let tile = 22.0;
        let icon_rect = Rect::from_min_size(pos2(x, rect.center().y - tile / 2.0), vec2(tile, tile));
        widgets::paint_icon_tile(&painter, icon_rect, Icon::for_category(d.category), p.category(d.category), &p);
        let text_x = x + tile + 12.0;
        let name_rect = Rect::from_min_max(pos2(text_x, rect.top()), pos2(x + name_w - 12.0, rect.bottom()));
        let name_font = FontId::new(
            crate::theme::size::MD,
            if selected { crate::theme::semi() } else { egui::FontFamily::Proportional },
        );
        painter.with_clip_rect(name_rect).text(
            name_rect.left_center(),
            Align2::LEFT_CENTER,
            &d.file_name,
            name_font,
            p.text,
        );
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
                        Rect::from_min_size(pos2(cell.left(), cell.center().y - 3.0), vec2(cell.width() - 56.0, 6.0));
                    widgets::paint_progress(ui, bar, d, &p);
                    let pct = d.fraction().map_or("—".into(), |f| {
                        if f >= 1.0 { "100%".into() } else { format!("{:.1}%", (f * 1000.0).floor() / 10.0) }
                    });
                    let pct_color = if d.status == Status::Completed { p.text_dim } else { p.text };
                    painter.text(
                        cell.right_center() - vec2(4.0, 0.0),
                        Align2::RIGHT_CENTER,
                        pct,
                        FontId::new(12.5, crate::theme::semi()),
                        pct_color,
                    );
                }
                SortKey::Speed => {
                    if d.status == Status::Downloading {
                        painter.text(right, Align2::RIGHT_CENTER, format_speed(d.speed), small, p.text);
                    }
                }
                SortKey::Eta => {
                    if let Some(eta) = d.eta {
                        painter.text(right, Align2::RIGHT_CENTER, format_duration(eta), small, p.text_dim);
                    }
                }
                SortKey::Status => {
                    let (label, color) = status_label(d, &p);
                    widgets::paint_pill(
                        &painter,
                        cell.left_center() + vec2(4.0, 0.0),
                        &label,
                        color,
                        cell.width() - 10.0,
                        &p,
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

        self.row_interaction(ui, &response, order, index, d, selected);
    }

    /// Click, Ctrl/Shift-click, double-click and context menu of a row or card.
    fn row_interaction(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        order: &[DownloadId],
        index: usize,
        d: &DownloadView,
        selected: bool,
    ) {
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
                self.open_path(&d.path);
            } else {
                self.open_progress(d.id);
            }
        }
        let d = d.clone();
        response.context_menu(|ui| self.context_menu(ui, &d));
    }

    /// Cards or a dense list; remembered in the configuration.
    fn view_toggle(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let mut compact = self.cfg.compact_rows;
        let r = widgets::segmented(ui, &mut compact, &[(false, "Cards"), (true, "List")], &p)
            .on_hover_text("Rich cards, or a dense sortable list with columns");
        if r.changed() || compact != self.cfg.compact_rows {
            let mut config = self.engine.config();
            config.compact_rows = compact;
            if let Some(draft) = &mut self.settings_draft {
                draft.compact_rows = compact;
            }
            let r = self.engine.update_config(config);
            self.report(r);
        }
    }

    /// Four live figures above the cards: speed, what is in progress, what
    /// arrived today and the space left where downloads are saved.
    fn kpi_strip(&mut self, ui: &mut Ui, width: f32) {
        let p = self.palette;
        let s = &self.snap;
        let now = stewardget_core::util::now_unix();
        let unfinished: Vec<&DownloadView> = s.downloads.iter().filter(|d| d.status != Status::Completed).collect();
        let active = s.count(Status::is_active);
        let waiting = s.count(|x| matches!(x, Status::Queued | Status::Paused));
        let left: u64 = unfinished.iter().filter_map(|d| d.total.map(|t| t.saturating_sub(d.downloaded))).sum();
        let (got, of): (u64, u64) = unfinished
            .iter()
            .filter_map(|d| d.total.map(|t| (d.downloaded, t)))
            .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        let connections: usize = s.downloads.iter().map(|d| d.connections).sum();
        let peak = s.history.iter().copied().fold(0.0f32, f32::max);
        let finished_today =
            s.downloads.iter().filter(|d| d.finished_at.is_some_and(|t| now.saturating_sub(t) < 86_400)).count();
        let dir = self.cfg.download_dir.clone();
        let free = free_space(&dir);

        let gap = 12.0;
        let inner = width - CARD_X * 2.0;
        let n = if inner > 760.0 { 4 } else { 2 };
        let tile_w = (inner - gap * (n - 1) as f32) / n as f32;
        let history = s.history.clone();
        let speed = s.speed;
        let today = s.stats.today(now);
        let week: Vec<f32> = s.stats.recent_days(14, now).iter().map(|d| d.bytes as f32).collect();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.add_space(CARD_X - gap);
            let speed_sub = if active > 0 {
                format!("peak {} \u{00b7} {connections} connections", format_speed(peak as f64))
            } else {
                "Nothing downloading".to_owned()
            };
            let r = widgets::kpi_tile(
                ui,
                tile_w,
                Icon::Bolt,
                p.accent2,
                "Speed",
                &format_speed(speed),
                &speed_sub,
                widgets::KpiExtra::Spark(&history[history.len().saturating_sub(60)..]),
                &p,
            );
            if r.on_hover_text("Speed over the last minute").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.filter = ListFilter::Active;
            }
            let eta = if speed > 1.0 && left > 0 {
                format!(" \u{00b7} ~{}", format_duration((left as f64 / speed) as u64))
            } else {
                String::new()
            };
            let r = widgets::kpi_tile(
                ui,
                tile_w,
                Icon::Download,
                p.accent,
                "In progress",
                &format!("{active} active"),
                &format!("{waiting} waiting \u{00b7} {} left{eta}", format_bytes(left)),
                widgets::KpiExtra::Meter(if of > 0 { got as f32 / of as f32 } else { 0.0 }),
                &p,
            );
            if r.on_hover_text("Show what is downloading").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.filter = ListFilter::Active;
            }
            if n == 4 {
                let r = widgets::kpi_tile(
                    ui,
                    tile_w,
                    Icon::CheckCircle,
                    p.green,
                    "Today",
                    &format_bytes(today),
                    &format!("{finished_today} finished in the last 24 hours"),
                    widgets::KpiExtra::Spark(&week),
                    &p,
                );
                if r.on_hover_text("Open the history").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.page = crate::app::Page::History;
                }
                let (value, sub, color) = match free {
                    Some(f) if left > f => {
                        (format_bytes(f), format!("{} short for what's left", format_bytes(left - f)), p.yellow)
                    }
                    Some(f) => (format_bytes(f), format!("free in {}", short_dir(&dir)), p.accent2),
                    None => ("—".to_owned(), short_dir(&dir), p.text_dim),
                };
                let r = widgets::kpi_tile(
                    ui,
                    tile_w,
                    Icon::Drive,
                    color,
                    "Disk space",
                    &value,
                    &sub,
                    widgets::KpiExtra::Meter(match free {
                        Some(f) if f > 0 => (left as f32 / f as f32).min(1.0),
                        _ => 0.0,
                    }),
                    &p,
                );
                if r.on_hover_text(format!(
                    "{}\nThe bar is how much of it the unfinished downloads still need.\nClick to open the folder.",
                    dir.display()
                ))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                {
                    self.open_path(&dir);
                }
            }
        });
    }

    /// Above the cards: how many are shown and the sort order.
    fn sort_bar(&mut self, ui: &mut Ui, shown: usize) {
        let p = self.palette;
        let width = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
        let noun = if shown == 1 { "download" } else { "downloads" };
        ui.painter().galley(
            pos2(bar.left() + CARD_X + 4.0, bar.center().y - 6.0),
            widgets::caps_galley(ui, &format!("{shown} {noun}").to_uppercase(), 10.5, p.text_dim),
            p.text_dim,
        );
        let right = Rect::from_min_max(pos2(bar.center().x, bar.top()), pos2(bar.right() - CARD_X, bar.bottom()));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                const KEYS: [(SortKey, &str); 8] = [
                    (SortKey::Queue, "Queue order"),
                    (SortKey::Added, "Date added"),
                    (SortKey::Name, "Name"),
                    (SortKey::Size, "Size"),
                    (SortKey::Progress, "Progress"),
                    (SortKey::Speed, "Speed"),
                    (SortKey::Eta, "Time left"),
                    (SortKey::Status, "Status"),
                ];
                let dir_icon = if self.table.ascending { Icon::ArrowUp } else { Icon::ArrowDown };
                if self.table.sort != SortKey::Queue
                    && widgets::icon_button(
                        ui,
                        dir_icon,
                        if self.table.ascending { "Ascending" } else { "Descending" },
                        &p,
                    )
                    .clicked()
                {
                    self.table.ascending = !self.table.ascending;
                }
                let current = KEYS.iter().find(|(k, _)| *k == self.table.sort).map_or("Queue order", |(_, t)| t);
                let mut chosen = self.table.sort;
                egui::ComboBox::from_id_salt("card-sort")
                    .width(130.0)
                    .selected_text(RichText::new(current).size(12.5))
                    .show_ui(ui, |ui| {
                        for (k, t) in KEYS {
                            ui.selectable_value(&mut chosen, k, t);
                        }
                    });
                if chosen != self.table.sort {
                    self.table.sort = chosen;
                    self.table.ascending = !matches!(chosen, SortKey::Size | SortKey::Speed | SortKey::Added);
                }
                ui.label(RichText::new("Sort by").size(12.5).color(p.text_dim));
            },
        );
        ui.add_space(6.0);
    }

    /// One download as a card: file mark, name and source, status, a progress
    /// bar with the figures that matter right now, and its main actions.
    fn card_row(&mut self, ui: &mut Ui, order: &[DownloadId], index: usize, d: &DownloadView) {
        let p = self.palette;
        let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_H + CARD_GAP), Sense::hover());
        let card = Rect::from_min_size(slot.min + vec2(CARD_X, 0.0), vec2(slot.width() - CARD_X * 2.0, CARD_H));
        let response = ui.interact(card, egui::Id::unique(("stewardget-card", d.id)), Sense::click_and_drag());
        if response.drag_started() {
            let ids =
                if self.selection.contains(&d.id) { self.selection.iter().copied().collect() } else { vec![d.id] };
            egui::DragAndDrop::set_payload(ui.ctx(), DraggedDownloads(ids));
        }
        let selected = self.selection.contains(&d.id);
        let hovered = response.hovered();
        let painter = ui.painter().clone();
        let corner = CornerRadius::same(crate::theme::radius::LG + 2);
        if !p.dark {
            painter.add(p.shadow(if hovered { 4 } else { 1 }, if hovered { 14 } else { 6 }).as_shape(card, corner));
        }
        let (fill, stroke) = if selected {
            (
                widgets::lerp_color(p.card, p.accent, if p.dark { 0.10 } else { 0.06 }),
                Stroke::new(1.5, p.accent.gamma_multiply(0.85)),
            )
        } else if hovered {
            (p.card_hover, Stroke::new(1.0, p.control_border))
        } else {
            (p.card, Stroke::new(1.0, p.border))
        };
        painter.rect(card, corner, fill, stroke, egui::StrokeKind::Inside);
        // Live downloads carry a thin accent edge on the left.
        if d.status.is_active() {
            let edge = Rect::from_min_size(card.left_top() + vec2(0.0, 18.0), vec2(3.0, card.height() - 36.0));
            let (a, b) = widgets::status_gradient(d.status, &p);
            widgets::paint_gradient_rect(&painter, edge, 1.5, a, b, true);
        }

        let tile = Rect::from_center_size(card.left_center() + vec2(16.0 + 26.0, 0.0), vec2(52.0, 52.0));
        widgets::paint_file_tile(
            &painter,
            tile,
            Icon::for_category(d.category),
            widgets::extension(&d.file_name),
            p.category(d.category),
            &p,
        );

        let actions_w = 3.0 * crate::theme::CONTROL_H + 2.0 * 6.0;
        let x0 = tile.right() + 16.0;
        let x1 = card.right() - 16.0 - actions_w - 18.0;
        let top = card.top();

        // Line 1: name and status.
        let (label, color) = status_label(d, &p);
        let pill_w =
            painter.layout_no_wrap(label.clone(), FontId::new(12.0, crate::theme::semi()), color).size().x + 26.0;
        let pill_w = pill_w.min((x1 - x0) * 0.45);
        widgets::paint_pill(&painter, pos2(x1 - pill_w, top + 21.0), &label, color, pill_w, &p);
        let name_clip = Rect::from_min_max(pos2(x0, top), pos2(x1 - pill_w - 12.0, card.bottom()));
        painter.with_clip_rect(name_clip.intersect(painter.clip_rect())).text(
            pos2(x0, top + 21.0),
            Align2::LEFT_CENTER,
            &d.file_name,
            FontId::new(crate::theme::size::MD + 0.5, crate::theme::semi()),
            p.text,
        );

        // Line 2: where it comes from.
        let host = crate::capture::host_of(&d.url);
        let now = stewardget_core::util::now_unix();
        let mut meta = vec![self.cfg.category_label(d.category)];
        if !host.is_empty() {
            meta.push(host);
        }
        meta.push(format!("added {}", stewardget_core::util::format_relative(d.added_at, now)));
        if self.engine.config().queues.len() > 1 {
            meta.push(self.engine.config().queue(d.queue).name.clone());
        }
        painter
            .with_clip_rect(Rect::from_min_max(pos2(x0, top), pos2(x1, card.bottom())).intersect(painter.clip_rect()))
            .text(
                pos2(x0, top + 41.0),
                Align2::LEFT_CENTER,
                meta.join("  \u{00b7}  "),
                FontId::proportional(crate::theme::size::SM),
                p.text_dim,
            );

        // Line 3: progress.
        let bar = Rect::from_min_max(pos2(x0, top + 55.0), pos2(x1, top + 61.0));
        widgets::paint_progress(ui, bar, d, &p);

        // Line 4: figures.
        let small = FontId::proportional(12.5);
        let strong = FontId::new(12.5, crate::theme::semi());
        let figures_y = top + 75.0;
        let pct = d
            .fraction()
            .map(|f| if f >= 1.0 { "100%".to_owned() } else { format!("{:.1}%", (f * 1000.0).floor() / 10.0) });
        let amount = match (d.total, d.status) {
            (Some(t), Status::Completed) => format_bytes(t),
            (Some(t), _) => format!("{} of {}", format_bytes(d.downloaded), format_bytes(t)),
            (None, _) if d.downloaded > 0 => format!("{} so far", format_bytes(d.downloaded)),
            (None, _) => "Size not known yet".to_owned(),
        };
        let left_text = match &pct {
            Some(pct) if d.status != Status::Completed => format!("{amount}  \u{00b7}  {pct}"),
            _ => amount,
        };
        let lg = painter.text(pos2(x0, figures_y), Align2::LEFT_CENTER, left_text, small.clone(), p.text_dim);
        let right_text = match d.status {
            Status::Downloading => {
                let mut parts = vec![format_speed(d.speed)];
                if let Some(eta) = d.eta {
                    parts.push(format!("{} left", format_duration(eta)));
                }
                if d.connections > 0 {
                    parts.push(format!("\u{00d7}{}", d.connections));
                }
                Some((parts.join("  \u{00b7}  "), p.text, strong))
            }
            Status::Completed => d.finished_at.map(|t| {
                let secs = if d.active_secs > 0 { d.active_secs } else { t.saturating_sub(d.added_at) }.max(1);
                (
                    format!(
                        "Finished {}  \u{00b7}  avg {}",
                        stewardget_core::util::format_relative(t, now),
                        format_speed(d.downloaded as f64 / secs as f64)
                    ),
                    p.text_dim,
                    small,
                )
            }),
            Status::Failed => d.error.as_ref().map(|e| (crate::app::capitalize(e), p.red, small)),
            _ => None,
        };
        if let Some((text, color, font)) = right_text {
            let clip = Rect::from_min_max(pos2(lg.right() + 16.0, top), pos2(x1, card.bottom()));
            let g = painter.layout_no_wrap(text, font, color);
            let pos = pos2((x1 - g.size().x).max(clip.left()), figures_y - g.size().y / 2.0);
            painter.with_clip_rect(clip.intersect(painter.clip_rect())).galley(pos, g, color);
        }

        // Actions: the main one first, filled; then the folder and a menu.
        let actions = Rect::from_min_max(
            pos2(card.right() - 16.0 - actions_w, card.top()),
            pos2(card.right() - 16.0, card.bottom()),
        );
        let mut open_menu = None;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(actions).layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let more = widgets::round_button(ui, Icon::More, "More actions", false, &p);
                if more.clicked() && !selected {
                    self.selection.clear();
                    self.selection.insert(d.id);
                    self.cursor = Some(d.id);
                    self.anchor = Some(d.id);
                }
                open_menu = Some(more);
                if widgets::round_button(ui, Icon::FolderOpen, "Show in folder", false, &p).clicked() {
                    self.reveal(&d.path);
                }
                if d.status == Status::Completed {
                    let exists = file_exists(&d.path);
                    if exists && widgets::round_button(ui, Icon::External, "Open", true, &p).clicked() {
                        self.open_path(&d.path);
                    } else if !exists && widgets::round_button(ui, Icon::Retry, "Download again", true, &p).clicked() {
                        let r = self.engine.restart(d.id);
                        self.report(r);
                    }
                } else if d.status.can_pause() {
                    if widgets::round_button(ui, Icon::Pause, "Pause", false, &p).clicked() {
                        let r = self.engine.pause(d.id);
                        self.report(r);
                    }
                } else if d.status.can_resume() && problem_of(d) != Some(Problem::Changed) {
                    // A changed file needs a decision; the details pane offers it.
                    let (icon, tip) =
                        if d.status == Status::Failed { (Icon::Retry, "Retry") } else { (Icon::Play, "Resume") };
                    if widgets::round_button(ui, icon, tip, true, &p).clicked() {
                        let r = self.engine.resume(d.id);
                        self.report(r);
                    }
                }
            },
        );
        if let Some(more) = open_menu {
            let dd = d.clone();
            egui::Popup::menu(&more).show(|ui| self.context_menu(ui, &dd));
        }

        self.row_interaction(ui, &response, order, index, d, selected);
    }

    fn context_menu(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        ui.set_min_width(230.0);
        let many = self.selection.len() > 1;
        let exists = file_exists(&d.path);
        if !many {
            if menu_item(ui, &p, Icon::External, "Open", d.status == Status::Completed && exists) {
                self.open_path(&d.path);
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
            widgets::icon_tile(ui, Icon::Download, 64.0, p.accent, &p);
            ui.add_space(12.0);
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
            ui.label(RichText::new(title).size(crate::theme::size::XL).family(crate::theme::semi()).color(p.text));
            ui.add_space(2.0);
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
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    thread_local! {
        static SEEN: RefCell<HashMap<std::path::PathBuf, (Instant, Option<u64>)>> = RefCell::new(HashMap::new());
    }
    SEEN.with_borrow_mut(|seen| {
        if let Some(&(at, free)) = seen.get(path)
            && at.elapsed() < Duration::from_secs(2)
        {
            return free;
        }
        if seen.len() > 64 {
            seen.clear();
        }
        let free = stewardget_core::util::free_space(path);
        seen.insert(path.to_path_buf(), (Instant::now(), free));
        free
    })
}

/// The last folder name of a path, for tight spaces ("Downloads").
fn short_dir(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
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
