//! Custom-painted widgets: buttons, progress bars, the connection map and charts.

use egui::epaint::{Mesh, PathStroke};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use stewardget_core::util::{format_bytes, format_speed};
use stewardget_core::{DayTotal, DownloadView, Status};

use crate::icons::{self, Icon};
use crate::theme::{CONTROL_H, Palette};

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Fills the `[a, b]` slice of the pill-shaped `rect` with a gradient that
/// spans the whole pill, so adjacent slices join seamlessly and ends stay round.
fn pill_slice(rect: Rect, a: f32, b: f32, left: Color32, right: Color32) -> Shape {
    let (a, b) = (a.max(rect.left()), b.min(rect.right()));
    let r = rect.height() / 2.0;
    let cy = rect.center().y;
    let half_height = |x: f32| {
        let d = (rect.left() + r - x).max(x - (rect.right() - r)).max(0.0);
        (r * r - d * d).max(0.0).sqrt()
    };
    let mut xs = vec![a, b];
    for (lo, hi) in [(rect.left(), rect.left() + r), (rect.right() - r, rect.right())] {
        let mut x = lo.max(a);
        while x < hi.min(b) {
            xs.push(x);
            x += 0.75;
        }
    }
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    let mut mesh = Mesh::default();
    for (i, &x) in xs.iter().enumerate() {
        let color = lerp_color(left, right, (x - rect.left()) / rect.width().max(1.0));
        let h = half_height(x);
        mesh.colored_vertex(pos2(x, cy - h), color);
        mesh.colored_vertex(pos2(x, cy + h), color);
        if i > 0 {
            let v = (i as u32) * 2;
            mesh.add_triangle(v - 2, v - 1, v);
            mesh.add_triangle(v - 1, v + 1, v);
        }
    }
    Shape::mesh(mesh)
}

/// Gradient colours for a download's state.
pub fn status_gradient(status: Status, p: &Palette) -> (Color32, Color32) {
    match status {
        Status::Completed => (p.green, p.green),
        Status::Failed => (p.red.gamma_multiply(0.8), p.red),
        Status::Paused => (p.yellow.gamma_multiply(0.85), p.yellow),
        Status::Queued => (p.text_dim, p.text_dim),
        _ => (p.accent, p.accent2),
    }
}

/// A slim progress bar painted into `rect` (no layout allocation).
pub fn paint_progress(ui: &Ui, rect: Rect, d: &DownloadView, p: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), p.track);
    let (from, to) = status_gradient(d.status, p);
    match d.fraction() {
        Some(frac) if frac > 0.0 => {
            painter.add(pill_slice(rect, rect.left(), rect.left() + rect.width() * frac, from, to));
        }
        None if d.status.is_active() => {
            let t = ui.input(|i| i.time) as f32;
            let w = rect.width() * 0.3;
            let x = rect.left() + (t * 0.7).fract() * (rect.width() + w) - w;
            painter.add(pill_slice(rect, x, x + w, from, to));
            ui.ctx().request_repaint();
        }
        _ => {}
    }
}

/// IDM-style connection map: the file as a grid of blocks, each shaded by how
/// much of it is downloaded, with the blocks where connections are currently
/// writing highlighted.
pub fn connection_map(ui: &mut Ui, d: &DownloadView, p: &Palette, height: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    let cell = 9.0;
    let gap = 2.0;
    let cols = ((rect.width() + gap) / (cell + gap)).floor().max(1.0) as usize;
    let rows = ((rect.height() + gap) / (cell + gap)).floor().max(1.0) as usize;
    let n = cols * rows;
    let total = d.total.unwrap_or(0);
    let done_all = d.status == Status::Completed;
    let (from, to) = status_gradient(d.status, p);

    for i in 0..n {
        let (cx, cy) = (i % cols, i / cols);
        let min = rect.min + vec2(cx as f32 * (cell + gap), cy as f32 * (cell + gap));
        let block = Rect::from_min_size(min, Vec2::splat(cell));
        let (a, b) = if total > 0 {
            ((total as f64 * i as f64 / n as f64) as u64, (total as f64 * (i + 1) as f64 / n as f64) as u64)
        } else {
            (0, 0)
        };
        let (ratio, active) = if done_all {
            (1.0, false)
        } else if total == 0 || d.segments.is_empty() {
            let f = d.fraction().unwrap_or(0.0);
            (if (i as f32) < f * n as f32 { 1.0 } else { 0.0 }, false)
        } else {
            let mut covered = 0u64;
            let mut active = false;
            for s in &d.segments {
                let head = s.start + s.done;
                let (lo, hi) = (a.max(s.start), b.min(head));
                if hi > lo {
                    covered += hi - lo;
                }
                active |= s.active && head >= a && head < b && head < s.end;
            }
            (covered as f32 / (b - a).max(1) as f32, active)
        };
        let base = lerp_color(from, to, i as f32 / n as f32);
        let color = if active {
            // Connection heads: brightest block in dark themes, darkest in light ones.
            p.text
        } else if ratio <= 0.0 {
            p.track
        } else {
            lerp_color(p.track, base, 0.35 + 0.65 * ratio)
        };
        painter.rect_filled(block, CornerRadius::same(2), color);
    }
    response
}

/// Area chart of `values` with a fading gradient fill and hover read-out.
pub fn area_chart(ui: &mut Ui, values: &[f32], size: Vec2, color: Color32, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    for i in 1..4 {
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, p.border.gamma_multiply(0.6)),
        );
    }
    if values.len() < 2 {
        painter.text(rect.center(), Align2::CENTER_CENTER, "no data yet", FontId::proportional(11.5), p.text_dim);
        return response;
    }
    let max = values.iter().copied().fold(1.0f32, f32::max) * 1.15;
    let n = stewardget_core::HISTORY_LEN.max(values.len());
    let offset = n - values.len();
    let point = |i: usize, v: f32| {
        pos2(
            rect.left() + rect.width() * (i + offset) as f32 / (n - 1) as f32,
            rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0),
        )
    };
    let points: Vec<Pos2> = values.iter().enumerate().map(|(i, &v)| point(i, v)).collect();
    let mut mesh = Mesh::default();
    let (top, bottom) = (color.gamma_multiply(0.32), color.gamma_multiply(0.0));
    for w in points.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], top);
        mesh.colored_vertex(w[1], top);
        mesh.colored_vertex(pos2(w[1].x, rect.bottom()), bottom);
        mesh.colored_vertex(pos2(w[0].x, rect.bottom()), bottom);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(points.clone(), PathStroke::new(1.8, color)));
    if let Some(&last) = points.last() {
        painter.circle_filled(last, 3.0, color);
    }
    painter.text(
        rect.left_top() + vec2(4.0, 2.0),
        Align2::LEFT_TOP,
        format_speed(max as f64 / 1.15),
        FontId::proportional(10.5),
        p.text_dim,
    );

    if let Some(hover) = response.hover_pos() {
        let i = (((hover.x - rect.left()) / rect.width() * (n - 1) as f32).round() as usize).saturating_sub(offset);
        if let Some(&v) = values.get(i) {
            let pt = point(i, v);
            painter.line_segment(
                [pos2(pt.x, rect.top()), pos2(pt.x, rect.bottom())],
                Stroke::new(1.0, p.text_dim.gamma_multiply(0.5)),
            );
            painter.circle_filled(pt, 3.5, color);
            let ago = (values.len() - 1 - i) as f32 * stewardget_core::SAMPLE_INTERVAL_MS as f32 / 1000.0;
            let align = if pt.x > rect.center().x { Align2::RIGHT_BOTTOM } else { Align2::LEFT_BOTTOM };
            painter.text(
                pt + vec2(0.0, -8.0),
                align,
                format!("{} · {ago:.0}s ago", format_speed(v as f64)),
                FontId::proportional(11.0),
                p.text,
            );
        }
    }
    response
}

/// Daily totals as vertical bars with hover read-outs.
pub fn day_bars(ui: &mut Ui, days: &[DayTotal], p: &Palette, height: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect.expand(4.0));
    let chart = Rect::from_min_max(rect.min + vec2(16.0, 14.0), pos2(rect.max.x - 16.0, rect.max.y - 16.0));
    let max = days.iter().map(|d| d.bytes).max().unwrap_or(0).max(1) as f32;
    let slot = chart.width() / days.len().max(1) as f32;
    let bar_w = (slot * 0.62).max(2.0);
    let hovered = response.hover_pos().map(|h| ((h.x - chart.left()) / slot).floor() as usize);
    for (i, d) in days.iter().enumerate() {
        let x = chart.left() + slot * (i as f32 + 0.5);
        let h = (d.bytes as f32 / max * chart.height()).max(if d.bytes > 0 { 3.0 } else { 1.0 });
        let bar = Rect::from_center_size(pos2(x, chart.bottom() - h / 2.0), vec2(bar_w, h));
        let top = if hovered == Some(i) { p.accent2 } else { p.accent };
        let mut mesh = Mesh::default();
        mesh.colored_vertex(bar.left_top(), top);
        mesh.colored_vertex(bar.right_top(), top);
        mesh.colored_vertex(bar.right_bottom(), top.gamma_multiply(0.45));
        mesh.colored_vertex(bar.left_bottom(), top.gamma_multiply(0.45));
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
        // Counted from the newest day, so the last label never crowds the one before it.
        if (days.len() - 1 - i).is_multiple_of((days.len() / 8).max(1)) {
            painter.text(
                pos2(x, rect.bottom()),
                Align2::CENTER_BOTTOM,
                d.label(),
                FontId::proportional(10.5),
                p.text_dim,
            );
        }
        if hovered == Some(i) {
            painter.text(
                pos2(x, bar.top() - 3.0),
                Align2::CENTER_BOTTOM,
                format_bytes(d.bytes),
                FontId::proportional(11.0),
                p.text,
            );
        }
    }
}

/// Counts per hour of the day as bars; the busiest hour is highlighted.
pub fn hour_bars(ui: &mut Ui, hours: &[u32; 24], p: &Palette, height: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect.expand(4.0));
    let chart = Rect::from_min_max(rect.min + vec2(4.0, 14.0), pos2(rect.max.x - 4.0, rect.max.y - 14.0));
    let max = (*hours.iter().max().unwrap_or(&0)).max(1) as f32;
    let slot = chart.width() / 24.0;
    let hovered = response.hover_pos().map(|h| ((h.x - chart.left()) / slot).floor() as usize);
    let busiest = hours.iter().enumerate().max_by_key(|(_, n)| **n).map(|(h, _)| h);
    for (h, &n) in hours.iter().enumerate() {
        let x = chart.left() + slot * (h as f32 + 0.5);
        let bar_h = (n as f32 / max * chart.height()).max(if n > 0 { 3.0 } else { 1.0 });
        let bar = Rect::from_center_size(pos2(x, chart.bottom() - bar_h / 2.0), vec2((slot * 0.7).max(2.0), bar_h));
        let color =
            if hovered == Some(h) || (busiest == Some(h) && n > 0) { p.accent2 } else { p.accent.gamma_multiply(0.7) };
        painter.rect_filled(bar, CornerRadius::same(2), if n == 0 { p.track } else { color });
        if h % 6 == 0 {
            painter.text(
                pos2(x, rect.bottom()),
                Align2::CENTER_BOTTOM,
                format!("{h:02}"),
                FontId::proportional(10.5),
                p.text_dim,
            );
        }
        if hovered == Some(h) {
            painter.text(
                pos2(x, bar.top() - 3.0),
                Align2::CENTER_BOTTOM,
                format!("{n} · {h:02}:00"),
                FontId::proportional(11.0),
                p.text,
            );
        }
    }
}

/// GitHub-style calendar: one column per week, one cell per day, shaded by
/// bytes. `days` is oldest first and should end today.
pub fn heatmap(ui: &mut Ui, days: &[DayTotal], p: &Palette) {
    const GAP: f32 = 3.0;
    // Weeks fill the card; cells stay square between 9 and 20 points.
    let weeks = days.len().div_ceil(7) + 1;
    let cell_size = ((ui.available_width() - 4.0) / weeks as f32 - GAP).clamp(9.0, 20.0);
    let size = vec2(weeks as f32 * (cell_size + GAP), 7.0 * (cell_size + GAP));
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width().max(size.x), size.y), Sense::hover());
    let painter = ui.painter_at(rect);
    let max = days.iter().map(|d| d.bytes).max().unwrap_or(0).max(1) as f32;
    // Weekday of the first day: day 0 of the epoch was a Thursday.
    let first_dow = days.first().map_or(0, |d| (d.day + 3).rem_euclid(7)) as usize;
    let hover = response.hover_pos();
    let mut tip = None;
    for (i, d) in days.iter().enumerate() {
        let slot = first_dow + i;
        let (col, row) = (slot / 7, slot % 7);
        let cell = Rect::from_min_size(
            rect.min + vec2(col as f32 * (cell_size + GAP), row as f32 * (cell_size + GAP)),
            Vec2::splat(cell_size),
        );
        let color = if d.bytes == 0 {
            p.track
        } else {
            lerp_color(p.accent.gamma_multiply(0.35), p.accent2, (d.bytes as f32 / max).sqrt())
        };
        painter.rect_filled(cell, CornerRadius::same(3), color);
        if hover.is_some_and(|h| cell.expand(GAP / 2.0).contains(h)) {
            painter.rect_stroke(cell, CornerRadius::same(3), Stroke::new(1.5, p.text), StrokeKind::Outside);
            tip = Some(format!("{}  ·  {}", d.label(), format_bytes(d.bytes)));
        }
    }
    if let Some(tip) = tip {
        response.on_hover_text(tip);
    }
}

/// Horizontal share bar.
pub fn share_bar(ui: &mut Ui, fraction: f32, color: Color32, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), p.track);
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(fill, CornerRadius::same(3), color);
}

/// Paints an icon at a fixed size into the layout.
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    icons::paint(ui.painter(), rect, icon, color);
    response
}

/// Square, borderless icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CONTROL_H), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(7), p.card_hover);
    }
    let color = if response.hovered() { p.text } else { p.text_dim };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(16.0)), icon, color);
    response.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Ghost,
    Danger,
    /// Flat toolbar button: no background until hovered.
    Tool,
}

/// A fully painted button with an optional leading icon. Every kind has the
/// same height as text fields ([`CONTROL_H`]).
pub fn button(ui: &mut Ui, kind: ButtonKind, icon: Option<Icon>, text: &str, enabled: bool, p: &Palette) -> Response {
    let (fill, hover, fg, stroke) = match kind {
        ButtonKind::Primary => (p.accent, lerp_color(p.accent, Color32::WHITE, 0.14), p.on_accent, Stroke::NONE),
        ButtonKind::Danger => (p.red, lerp_color(p.red, Color32::WHITE, 0.12), Color32::WHITE, Stroke::NONE),
        ButtonKind::Ghost => (p.card, p.card_hover, p.text, Stroke::new(1.0, p.control_border)),
        ButtonKind::Tool => (Color32::TRANSPARENT, p.card_hover, p.text, Stroke::NONE),
    };
    // Disabled buttons use neutral colours in every theme so the label stays readable.
    let (fill, hover, fg, stroke) = if enabled {
        (fill, hover, fg, stroke)
    } else {
        let muted = lerp_color(p.text_dim, p.bg, 0.35);
        match kind {
            ButtonKind::Primary | ButtonKind::Danger => (p.track, p.track, muted, Stroke::NONE),
            ButtonKind::Ghost => (p.card, p.card, muted, Stroke::new(1.0, p.border)),
            ButtonKind::Tool => (Color32::TRANSPARENT, Color32::TRANSPARENT, muted, Stroke::NONE),
        }
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(13.5), fg);
    let icon_w = match icon {
        Some(_) if text.is_empty() => 16.0,
        Some(_) => 23.0,
        None => 0.0,
    };
    let pad = if text.is_empty() { 7.0 } else { 13.0 };
    let size = vec2((galley.size().x + icon_w + pad * 2.0).max(CONTROL_H), CONTROL_H);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    let fill = if enabled && response.hovered() { hover } else { fill };
    painter.rect(rect, CornerRadius::same(8), fill, stroke, StrokeKind::Inside);
    if enabled && response.has_focus() {
        painter.rect_stroke(rect.expand(2.0), CornerRadius::same(10), Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    let content_w = galley.size().x + icon_w;
    let mut x = rect.center().x - content_w / 2.0;
    if let Some(icon) = icon {
        let color = if kind == ButtonKind::Tool && enabled { p.accent } else { fg };
        icons::paint(painter, Rect::from_min_size(pos2(x, rect.center().y - 8.0), Vec2::splat(16.0)), icon, color);
        x += icon_w;
    }
    painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

/// Single-line text field with the standard height, padding and outline.
pub fn input(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .margin(egui::Margin::symmetric(10, 7))
        .min_size(vec2(0.0, CONTROL_H))
        .align(egui::Align2::LEFT_CENTER)
}

/// A speed field ("2M", "500k"; empty = unlimited) bound to bytes/second.
/// What is typed is kept while the field has focus, so "2" doesn't turn into
/// "2 B" before the "M" can follow; it is shown formatted otherwise.
pub fn speed_input(ui: &mut Ui, id: &str, value: &mut u64, width: f32) -> Response {
    let id = egui::Id::unique(("stewardget-speed-input", id));
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| if *value == 0 { String::new() } else { format_bytes(*value) });
    let r = ui.add(input(&mut text).hint_text("unlimited").desired_width(width));
    if r.changed() {
        let typed = text.trim();
        if typed.is_empty() {
            *value = 0;
        } else if let Some(v) = stewardget_core::util::parse_size(typed) {
            *value = v;
        }
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    r
}

/// Multi-line text field with the standard padding and outline.
pub fn input_multiline(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text).margin(egui::Margin::symmetric(10, 8))
}

/// Text field plus a trailing button; together they fill the row.
pub fn input_with_button(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    icon: Icon,
    label: &str,
    p: &Palette,
) -> (Response, bool) {
    ui.horizontal(|ui| {
        let label_w = ui.painter().layout_no_wrap(label.to_owned(), FontId::proportional(13.5), p.text).size().x;
        let button_w = if label.is_empty() { CONTROL_H } else { label_w + 49.0 };
        let field_w = (ui.available_width() - button_w - ui.spacing().item_spacing.x).max(80.0);
        let field = ui.add(input(text).hint_text(hint).desired_width(field_w));
        let clicked = button(ui, ButtonKind::Ghost, Some(icon), label, true, p).clicked();
        (field, clicked)
    })
    .inner
}

/// Folder text field with Browse and a menu of recently used folders.
/// Returns whether the user changed the folder.
pub fn folder_field(ui: &mut Ui, dir: &mut String, recent: &[std::path::PathBuf], p: &Palette) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let extra = if recent.is_empty() { 0.0 } else { CONTROL_H + ui.spacing().item_spacing.x };
        let label_w = ui.painter().layout_no_wrap("Browse".to_owned(), FontId::proportional(13.5), p.text).size().x;
        let field_w = (ui.available_width() - label_w - 49.0 - extra - ui.spacing().item_spacing.x).max(80.0);
        changed |= ui.add(input(dir).desired_width(field_w)).changed();
        if button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Browse", true, p).clicked() {
            let start = std::path::Path::new(dir.trim());
            let picker =
                if start.is_dir() { rfd::FileDialog::new().set_directory(start) } else { rfd::FileDialog::new() };
            if let Some(folder) = picker.pick_folder() {
                *dir = folder.display().to_string();
                changed = true;
            }
        }
        if !recent.is_empty() {
            let r = button(ui, ButtonKind::Ghost, Some(Icon::History), "", true, p).on_hover_text("Recent folders");
            egui::Popup::menu(&r).show(|ui| {
                ui.set_min_width(300.0);
                ui.label(egui::RichText::new("Recent folders").size(11.5).color(p.text_dim));
                for folder in recent {
                    let text = folder.display().to_string();
                    if ui.selectable_label(*dir == text, &text).clicked() {
                        *dir = text;
                        changed = true;
                        ui.close();
                    }
                }
            });
        }
    });
    changed
}

/// Segmented control: one choice out of a few, shown side by side.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], p: &Palette) -> Response {
    let font = FontId::proportional(13.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), p.text).size().x + 26.0)
        .collect();
    let total = widths.iter().sum::<f32>() + 6.0;
    let (rect, mut response) = ui.allocate_exact_size(vec2(total, CONTROL_H), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(8), p.track, Stroke::NONE, StrokeKind::Inside);
    let mut x = rect.left() + 3.0;
    for ((option, label), w) in options.iter().zip(widths) {
        let cell = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(w, rect.height() - 6.0));
        let r = ui.interact(cell, response.id.with(label), Sense::click());
        let selected = *value == *option;
        if selected {
            ui.painter().rect(cell, CornerRadius::same(6), p.card, Stroke::new(1.0, p.border), StrokeKind::Inside);
        } else if r.hovered() {
            ui.painter().rect_filled(cell, CornerRadius::same(6), p.card_hover.gamma_multiply(0.6));
        }
        let color = if selected { p.text } else { p.text_dim };
        let family = if selected { crate::theme::semi() } else { egui::FontFamily::Proportional };
        ui.painter().text(cell.center(), Align2::CENTER_CENTER, *label, FontId::new(13.0, family), color);
        if r.clicked() && !selected {
            *value = *option;
            response.mark_changed();
        }
        r.on_hover_cursor(egui::CursorIcon::PointingHand);
        x += w;
    }
    response
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool, p: &Palette) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let track = lerp_color(p.control_border, p.accent, t);
    ui.painter().rect_filled(rect, CornerRadius::same(11), track);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    let knob = pos2(x, rect.center().y);
    ui.painter().circle_filled(knob + vec2(0.0, 0.8), 8.2, Color32::from_black_alpha(40));
    ui.painter().circle_filled(knob, 8.0, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A label + control row inside forms; labels share one column width.
pub fn form_row<R>(ui: &mut Ui, label_w: f32, label: &str, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(label_w, CONTROL_H), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, FontId::proportional(13.0), p.text_dim);
        add(ui)
    })
    .inner
}

/// The StewardGet logo, drawn from the bundled bitmap.
pub fn logo(ui: &mut Ui, size: f32) -> Response {
    let id = egui::Id::unique("stewardget-logo-texture");
    let texture = match ui.ctx().memory_mut(|m| m.data.get_temp::<egui::TextureHandle>(id)) {
        Some(t) => t,
        None => {
            let image = egui::ColorImage::from_rgba_unmultiplied([128, 128], include_bytes!("../assets/icon-128.rgba"));
            let handle = ui.ctx().load_texture("stewardget-logo", image, egui::TextureOptions::LINEAR);
            ui.ctx().memory_mut(|m| m.data.insert_temp(id, handle.clone()));
            handle
        }
    };
    ui.add(egui::Image::new((texture.id(), Vec2::splat(size))))
}

/// A rounded panel frame.
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
}

/// Small upper-case section caption.
pub fn caption(ui: &mut Ui, text: &str, p: &Palette) {
    ui.label(egui::RichText::new(text).size(10.5).family(crate::theme::semi()).color(p.text_dim));
}

/// A sidebar section title that folds its section away when clicked.
/// Returns whether the section is open; the choice is remembered.
pub fn section(ui: &mut Ui, text: &str, p: &Palette) -> bool {
    let id = egui::Id::unique(("stewardget-section", text));
    let open = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, true));
    let width = ui.available_width().min(150.0);
    let (rect, r) = ui.allocate_exact_size(vec2(width, 16.0), Sense::click());
    let color = if r.hovered() { p.text } else { p.text_dim };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::new(10.5, crate::theme::semi()), color);
    let text_w = galley.size().x;
    ui.painter().galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, color);
    let caret = if open { Icon::CaretDown } else { Icon::CaretUp };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(rect.left() + text_w + 10.0, rect.center().y), vec2(9.0, 9.0)),
        caret,
        color,
    );
    if r.on_hover_text(if open { "Fold away" } else { "Show" }).clicked() {
        ui.ctx().data_mut(|d| d.insert_persisted(id, !open));
        return !open;
    }
    open
}

/// Compact metric tile.
pub fn stat_tile(ui: &mut Ui, p: &Palette, label: &str, value: &str, caption_text: &str, color: Color32) {
    card(p).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(58.0);
        ui.label(egui::RichText::new(label).size(11.5).color(p.text_dim));
        ui.label(egui::RichText::new(value).size(19.0).family(crate::theme::semi()).color(color));
        if !caption_text.is_empty() {
            ui.label(egui::RichText::new(caption_text).size(11.0).color(p.text_dim));
        }
    });
}

/// Coloured status dot.
pub fn dot(painter: &egui::Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, 3.5, color);
}

/// Pill-shaped status badge.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.5), color);
    let size = galley.size() + vec2(14.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), color.gamma_multiply(0.15));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}
