//! Custom-painted widgets: buttons, progress bars, the connection map and charts.

use egui::epaint::{Mesh, PathStroke};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shadow, Shape, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};
use stewardget_core::util::{format_bytes, format_speed};
use stewardget_core::{DayTotal, DownloadView, Status};

use crate::icons::{self, Icon};
use crate::theme::{CAPS_TRACKING, CONTROL_H, Palette, radius};

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

/// Outline of a rounded rectangle, clockwise from the top-left corner, with the
/// outward normal of each point (used to feather the edge of gradient meshes).
fn rounded_outline(rect: Rect, r: f32) -> Vec<(Pos2, Vec2)> {
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0).max(0.0);
    let corners = [
        (pos2(rect.left() + r, rect.top() + r), std::f32::consts::PI),
        (pos2(rect.right() - r, rect.top() + r), std::f32::consts::PI * 1.5),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), std::f32::consts::PI * 0.5),
    ];
    let steps = if r < 1.0 { 1 } else { (r * 0.6).ceil().clamp(3.0, 12.0) as usize };
    let mut out = Vec::with_capacity(4 * (steps + 1));
    for (c, start) in corners {
        for i in 0..=steps {
            let a = start + std::f32::consts::FRAC_PI_2 * i as f32 / steps as f32;
            let n = vec2(a.cos(), a.sin());
            out.push((c + n * r, n));
        }
    }
    out
}

/// A rounded rectangle filled with a gradient from `a` to `b`, left to right
/// (or top to bottom when `vertical`), with an anti-aliased edge.
pub fn paint_gradient_rect(painter: &egui::Painter, rect: Rect, r: f32, a: Color32, b: Color32, vertical: bool) {
    let outline = rounded_outline(rect, r);
    let color_at = |pt: Pos2| {
        let t = if vertical {
            (pt.y - rect.top()) / rect.height().max(1.0)
        } else {
            (pt.x - rect.left()) / rect.width().max(1.0)
        };
        lerp_color(a, b, t)
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    let n = outline.len() as u32;
    for &(pt, normal) in &outline {
        let inner = pt - normal * 0.5;
        mesh.colored_vertex(inner, color_at(inner));
        mesh.colored_vertex(pt + normal * 0.5, Color32::TRANSPARENT);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (ii, io, ji, jo) = (1 + i * 2, 2 + i * 2, 1 + j * 2, 2 + j * 2);
        mesh.add_triangle(0, ii, ji);
        mesh.add_triangle(ii, io, jo);
        mesh.add_triangle(ii, jo, ji);
    }
    painter.add(Shape::mesh(mesh));
}

/// A soft round light, for the ambient glows behind hero areas.
pub fn paint_glow(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color);
    let steps = 48;
    for i in 0..=steps {
        let a = std::f32::consts::TAU * i as f32 / steps as f32;
        mesh.colored_vertex(center + vec2(a.cos(), a.sin()) * radius * 0.45, color.gamma_multiply(0.55));
        mesh.colored_vertex(center + vec2(a.cos(), a.sin()) * radius, Color32::TRANSPARENT);
    }
    for i in 0..steps as u32 {
        let (m, o, m2, o2) = (1 + i * 2, 2 + i * 2, 3 + i * 2, 4 + i * 2);
        mesh.add_triangle(0, m, m2);
        mesh.add_triangle(m, o, o2);
        mesh.add_triangle(m, o2, m2);
    }
    painter.add(Shape::mesh(mesh));
}

/// Circular progress: a track ring and an arc from twelve o'clock, shaded
/// from `from` to `to` along its length.
#[allow(clippy::too_many_arguments)]
pub fn paint_ring(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    width: f32,
    fraction: f32,
    from: Color32,
    to: Color32,
    track: Color32,
) {
    painter.circle_stroke(center, radius, Stroke::new(width, track));
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.0 {
        return;
    }
    let steps = ((96.0 * fraction).ceil() as usize).max(2);
    let start = -std::f32::consts::FRAC_PI_2;
    let (ri, ro) = (radius - width / 2.0, radius + width / 2.0);
    let mut mesh = Mesh::default();
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let a = start + std::f32::consts::TAU * fraction * t;
        let n = vec2(a.cos(), a.sin());
        let c = lerp_color(from, to, t);
        mesh.colored_vertex(center + n * ri, c);
        mesh.colored_vertex(center + n * ro, c);
        if i > 0 {
            let v = i as u32 * 2;
            mesh.add_triangle(v - 2, v - 1, v);
            mesh.add_triangle(v - 1, v + 1, v);
        }
    }
    painter.add(Shape::mesh(mesh));
    // Round caps.
    let end = start + std::f32::consts::TAU * fraction;
    painter.circle_filled(center + vec2(start.cos(), start.sin()) * radius, width / 2.0, from);
    painter.circle_filled(center + vec2(end.cos(), end.sin()) * radius, width / 2.0, to);
}

/// The file-type mark used for downloads: a tile shaded in the category colour
/// with its icon, and (when there is room) the file extension in a tag below.
pub fn paint_file_tile(painter: &egui::Painter, rect: Rect, icon: Icon, ext: &str, color: Color32, p: &Palette) {
    let r = (rect.height() * 0.28).round().clamp(5.0, 14.0);
    let (top, bottom) = if p.dark {
        (color.gamma_multiply(0.30), color.gamma_multiply(0.10))
    } else {
        (color.gamma_multiply(0.20), color.gamma_multiply(0.07))
    };
    paint_gradient_rect(painter, rect, r, top, bottom, true);
    painter.rect_stroke(
        rect,
        CornerRadius::same(r as u8),
        Stroke::new(1.0, color.gamma_multiply(0.22)),
        StrokeKind::Inside,
    );
    let big = rect.height() >= 40.0 && !ext.is_empty();
    let glyph = (rect.height() * if big { 0.42 } else { 0.5 }).round();
    let icon_center = if big { rect.center() - vec2(0.0, rect.height() * 0.1) } else { rect.center() };
    icons::paint(painter, Rect::from_center_size(icon_center, Vec2::splat(glyph)), icon, color);
    if big {
        let text: String = ext.chars().take(4).collect::<String>().to_uppercase();
        let size = (rect.height() * 0.2).clamp(8.5, 11.0);
        let galley = painter.layout_no_wrap(text, FontId::new(size, crate::theme::semi()), p.on_tile(color));
        let tag = Rect::from_center_size(
            pos2(rect.center().x, rect.bottom() - size * 0.95),
            vec2(galley.size().x + 8.0, size + 4.0),
        );
        painter.rect_filled(tag, CornerRadius::same(4), color);
        painter.galley(tag.center() - galley.size() / 2.0, galley, p.on_tile(color));
    }
}

/// Lower-case extension of a file name ("" when it has none).
pub fn extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => ext,
        _ => "",
    }
}

/// Small line chart filling `rect` (no axes), with a soft fill under it.
pub fn paint_sparkline(painter: &egui::Painter, rect: Rect, values: &[f32], color: Color32) {
    if values.len() < 2 {
        painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.5, color.gamma_multiply(0.5)));
        return;
    }
    let max = values.iter().copied().fold(1.0f32, f32::max) * 1.1;
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                rect.left() + rect.width() * i as f32 / (values.len() - 1) as f32,
                rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0),
            )
        })
        .collect();
    let mut mesh = Mesh::default();
    let (top, bottom) = (color.gamma_multiply(0.28), color.gamma_multiply(0.0));
    for w in pts.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], top);
        mesh.colored_vertex(w[1], top);
        mesh.colored_vertex(pos2(w[1].x, rect.bottom()), bottom);
        mesh.colored_vertex(pos2(w[0].x, rect.bottom()), bottom);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(pts, PathStroke::new(1.6, color)));
}

/// What a KPI tile shows below its value.
pub enum KpiExtra<'a> {
    /// Recent samples, drawn as a sparkline.
    Spark(&'a [f32]),
    /// A share in `0..=1`, drawn as a bar.
    Meter(f32),
}

/// A summary tile: tinted icon and label, a large value, a supporting line and
/// a sparkline or meter along the bottom.
#[allow(clippy::too_many_arguments)]
pub fn kpi_tile(
    ui: &mut Ui,
    width: f32,
    icon: Icon,
    color: Color32,
    label: &str,
    value: &str,
    sub: &str,
    extra: KpiExtra<'_>,
    p: &Palette,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 104.0), Sense::click());
    let painter = ui.painter();
    let corner = CornerRadius::same(radius::LG + 2);
    if !p.dark {
        painter.add(p.shadow(2, 10).as_shape(rect, corner));
    }
    let fill = if response.hovered() { p.card_hover } else { p.card };
    painter.rect(rect, corner, fill, Stroke::new(1.0, p.border), StrokeKind::Inside);
    // A wash of the tile's colour from the top-left corner.
    let wash = Rect::from_min_size(rect.min, vec2(rect.width(), rect.height() * 0.65));
    paint_gradient_rect(
        &painter.with_clip_rect(rect.shrink(1.0)),
        wash.shrink(1.0),
        (radius::LG + 1) as f32,
        color.gamma_multiply(if p.dark { 0.10 } else { 0.07 }),
        Color32::TRANSPARENT,
        false,
    );
    let inner = rect.shrink2(vec2(14.0, 12.0));
    let tile = Rect::from_min_size(inner.min, Vec2::splat(26.0));
    paint_icon_tile(painter, tile, icon, color, p);
    let label_galley = caps_galley(ui, &label.to_uppercase(), 10.5, p.text_dim);
    painter.galley(pos2(tile.right() + 9.0, tile.center().y - label_galley.size().y / 2.0), label_galley, p.text_dim);
    let value_pos = pos2(inner.left(), tile.bottom() + 8.0);
    let vg = painter.text(
        value_pos,
        Align2::LEFT_TOP,
        value,
        FontId::new(crate::theme::size::XL + 2.0, crate::theme::semi()),
        p.text,
    );
    painter
        .with_clip_rect(Rect::from_min_max(pos2(inner.left(), rect.top()), pos2(inner.right(), rect.bottom())))
        .text(
            pos2(inner.left(), vg.bottom() + 3.0),
            Align2::LEFT_TOP,
            sub,
            FontId::proportional(crate::theme::size::SM),
            p.text_dim,
        );
    match extra {
        KpiExtra::Spark(values) => {
            let spark = Rect::from_min_max(
                pos2(inner.right() - inner.width() * 0.42, inner.top() + 30.0),
                pos2(inner.right(), vg.bottom()),
            );
            paint_sparkline(painter, spark, values, color);
        }
        KpiExtra::Meter(f) => {
            let bar = Rect::from_min_max(
                pos2(inner.right() - inner.width() * 0.38, vg.center().y - 3.0),
                pos2(inner.right(), vg.center().y + 3.0),
            );
            painter.rect_filled(bar, CornerRadius::same(3), p.track);
            let fill = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f.clamp(0.0, 1.0), bar.bottom()));
            if fill.width() > 0.5 {
                paint_gradient_rect(painter, fill, 3.0, color.gamma_multiply(0.75), color, false);
            }
        }
    }
    response
}

/// A round icon button for card rows; `primary` fills it with the accent.
pub fn round_button(ui: &mut Ui, icon: Icon, tooltip: &str, primary: bool, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CONTROL_H), Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    if primary {
        let (a, b) = p.gradient();
        let (a, b) =
            if hovered { (lerp_color(a, Color32::WHITE, 0.1), lerp_color(b, Color32::WHITE, 0.1)) } else { (a, b) };
        painter.add(p.glow(a, if hovered { 14 } else { 8 }).as_shape(rect, CornerRadius::same(255)));
        paint_gradient_rect(painter, rect, rect.height() / 2.0, a, b, false);
    } else {
        let fill = if pressed {
            p.track
        } else if hovered {
            p.card_hover
        } else {
            Color32::TRANSPARENT
        };
        painter.rect(
            rect,
            CornerRadius::same(255),
            fill,
            Stroke::new(1.0, if hovered { p.control_border } else { p.border }),
            StrokeKind::Inside,
        );
    }
    if response.has_focus() {
        painter.circle_stroke(rect.center(), rect.width() / 2.0 + 2.5, Stroke::new(2.0, p.accent.gamma_multiply(0.85)));
    }
    let color = if primary {
        p.on_accent
    } else if hovered {
        p.text
    } else {
        p.text_dim
    };
    icons::paint(painter, Rect::from_center_size(rect.center(), Vec2::splat(15.0)), icon, color);
    response.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Gradient colours for a download's state.
pub fn status_gradient(status: Status, p: &Palette) -> (Color32, Color32) {
    match status {
        Status::Completed => (p.green, p.green),
        // Opaque shades: a translucent end would show the track through it.
        Status::Failed => (lerp_color(p.red, p.card, 0.3), p.red),
        Status::Paused => (lerp_color(p.yellow, p.card, 0.3), p.yellow),
        Status::Queued => (p.text_dim, p.text_dim),
        _ => (p.accent, p.accent2),
    }
}

/// A slim progress bar painted into `rect` (no layout allocation). Live
/// downloads get a soft glow and a highlight that sweeps along the filled part.
pub fn paint_progress(ui: &Ui, rect: Rect, d: &DownloadView, p: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), p.track);
    let (from, to) = status_gradient(d.status, p);
    match d.fraction() {
        Some(frac) if frac > 0.0 => {
            let end = rect.left() + rect.width() * frac;
            let live = d.status == Status::Downloading;
            if live {
                let filled = Rect::from_min_max(rect.min, pos2(end.max(rect.left() + rect.height()), rect.max.y));
                painter.add(p.glow(to, 10).as_shape(filled, CornerRadius::same((rect.height() / 2.0) as u8)));
            }
            let filled = Rect::from_min_max(rect.min, pos2(end.max(rect.left() + rect.height()), rect.max.y));
            paint_gradient_rect(painter, filled, rect.height() / 2.0, from, lerp_color(from, to, frac), false);
            if live && end - rect.left() > 24.0 {
                // A short bright band travelling left to right, once every two seconds.
                let t = (ui.input(|i| i.time) as f32 * 0.5).fract();
                let w = 36.0f32.min((end - rect.left()) * 0.5);
                let x = rect.left() - w + t * (end - rect.left() + w);
                let shine = Color32::from_white_alpha(if p.dark { 60 } else { 80 });
                let r = rect.height() / 2.0;
                let clip = Rect::from_min_max(pos2(rect.left() + r, rect.top()), pos2(end - r, rect.bottom()));
                let mut mesh = Mesh::default();
                for (i, (bx, c)) in [(x, Color32::TRANSPARENT), (x + w / 2.0, shine), (x + w, Color32::TRANSPARENT)]
                    .into_iter()
                    .enumerate()
                {
                    mesh.colored_vertex(pos2(bx, rect.top()), c);
                    mesh.colored_vertex(pos2(bx, rect.bottom()), c);
                    if i > 0 {
                        let v = i as u32 * 2;
                        mesh.add_triangle(v - 2, v - 1, v);
                        mesh.add_triangle(v - 1, v + 1, v);
                    }
                }
                painter.with_clip_rect(clip.intersect(painter.clip_rect())).add(Shape::mesh(mesh));
                ui.ctx().request_repaint();
            }
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
        painter.text(rect.center(), Align2::CENTER_CENTER, "no data yet", FontId::proportional(12.0), p.text_dim);
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
                FontId::proportional(12.0),
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
                FontId::proportional(12.0),
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
                FontId::proportional(12.0),
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

/// Paints `icon` on a rounded square tinted with its own colour: the file-type
/// and section marks used in lists, cards and dialog titles.
pub fn paint_icon_tile(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32, p: &Palette) {
    let r = (rect.height() * 0.3).round().clamp(4.0, 12.0) as u8;
    painter.rect_filled(rect, CornerRadius::same(r), p.tint(color));
    let glyph = (rect.height() * 0.52).round();
    icons::paint(painter, Rect::from_center_size(rect.center(), Vec2::splat(glyph)), icon, color);
}

/// [`paint_icon_tile`] placed in the layout.
pub fn icon_tile(ui: &mut Ui, icon: Icon, size: f32, color: Color32, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon_tile(ui.painter(), rect, icon, color, p);
    response
}

/// A small rounded count, as next to navigation entries.
pub fn paint_badge(painter: &egui::Painter, right_center: Pos2, text: &str, fg: Color32, fill: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(11.5), fg);
    let size = vec2((galley.size().x + 12.0).max(22.0), 18.0);
    let rect = Rect::from_min_size(pos2(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    painter.rect_filled(rect, CornerRadius::same(9), fill);
    painter.galley(rect.center() - galley.size() / 2.0, galley, fg);
    rect
}

/// Square, borderless icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CONTROL_H), Sense::click());
    if response.hovered() {
        let fill = if response.is_pointer_button_down_on() { p.track } else { p.card_hover };
        ui.painter().rect_filled(rect, CornerRadius::same(radius::MD), fill);
    }
    if response.has_focus() {
        focus_ring(ui, rect, radius::MD, p);
    }
    let color = if response.hovered() { p.text } else { p.text_dim };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(17.0)), icon, color);
    response.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Keyboard focus outline, drawn just outside `rect`.
fn focus_ring(ui: &Ui, rect: Rect, r: u8, p: &Palette) {
    ui.painter().rect_stroke(
        rect.expand(2.5),
        CornerRadius::same(r + 3),
        Stroke::new(2.0, p.accent.gamma_multiply(0.85)),
        StrokeKind::Middle,
    );
}

/// Fills `rect` with a top-to-bottom gradient.
pub fn paint_vertical_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
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
        ButtonKind::Primary => (p.accent, lerp_color(p.accent, Color32::WHITE, 0.12), p.on_accent, Stroke::NONE),
        ButtonKind::Danger => (p.red, lerp_color(p.red, Color32::WHITE, 0.12), Color32::WHITE, Stroke::NONE),
        ButtonKind::Ghost => (p.card, p.card_hover, p.text, Stroke::new(1.0, p.control_border)),
        ButtonKind::Tool => (Color32::TRANSPARENT, p.card_hover, p.text, Stroke::NONE),
    };
    let solid = matches!(kind, ButtonKind::Primary | ButtonKind::Danger);
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
    let family = if solid { crate::theme::semi() } else { egui::FontFamily::Proportional };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(13.5, family), fg);
    let icon_w = match icon {
        Some(_) if text.is_empty() => 16.0,
        Some(_) => 24.0,
        None => 0.0,
    };
    let pad = if text.is_empty() { 8.0 } else { 14.0 };
    let size = vec2((galley.size().x + icon_w + pad * 2.0).max(CONTROL_H), CONTROL_H);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    let pressed = enabled && response.is_pointer_button_down_on();
    let fill = if pressed {
        if solid { lerp_color(fill, Color32::BLACK, 0.12) } else { p.track }
    } else if enabled && response.hovered() {
        hover
    } else {
        fill
    };
    let corner = CornerRadius::same(radius::MD);
    if solid && enabled {
        painter.add(p.glow(fill, if response.hovered() { 18 } else { 12 }).as_shape(rect, corner));
    } else if kind == ButtonKind::Ghost && enabled && !p.dark {
        painter.add(p.shadow(1, 3).as_shape(rect, corner));
    }
    let stroke = if kind == ButtonKind::Ghost && enabled && response.hovered() {
        Stroke::new(1.0, lerp_color(p.control_border, p.text_dim, 0.4))
    } else {
        stroke
    };
    if kind == ButtonKind::Primary && enabled {
        // The accent runs into its partner colour across the button.
        let (a, b) = p.gradient();
        let shade = |c: Color32| {
            if pressed {
                lerp_color(c, Color32::BLACK, 0.12)
            } else if response.hovered() {
                lerp_color(c, Color32::WHITE, 0.1)
            } else {
                c
            }
        };
        paint_gradient_rect(painter, rect, radius::MD as f32, shade(a), shade(b), false);
    } else {
        painter.rect(rect, corner, fill, stroke, StrokeKind::Inside);
    }
    if solid && enabled {
        // A hairline of light along the top edge gives solid buttons some depth.
        let top = rect.shrink2(vec2(radius::MD as f32, 0.0));
        painter.line_segment(
            [top.left_top() + vec2(0.0, 0.5), top.right_top() + vec2(0.0, 0.5)],
            Stroke::new(1.0, Color32::from_white_alpha(if pressed { 0 } else { 55 })),
        );
    }
    if enabled && response.has_focus() {
        focus_ring(ui, rect, radius::MD, p);
    }
    let painter = ui.painter();
    let content_w = galley.size().x + icon_w;
    let mut x = rect.center().x - content_w / 2.0;
    if let Some(icon) = icon {
        let color = if kind == ButtonKind::Tool && enabled && !response.hovered() { p.text_dim } else { fg };
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
        let button_w = if label.is_empty() { CONTROL_H } else { label_w + 52.0 };
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
        let field_w = (ui.available_width() - label_w - 52.0 - extra - ui.spacing().item_spacing.x).max(80.0);
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
                ui.label(egui::RichText::new("Recent folders").size(12.0).color(p.text_dim));
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
    ui.painter().rect(rect, CornerRadius::same(radius::MD), p.track, Stroke::NONE, StrokeKind::Inside);
    let mut x = rect.left() + 3.0;
    for ((option, label), w) in options.iter().zip(widths) {
        let cell = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(w, rect.height() - 6.0));
        let r = ui.interact(cell, response.id.with(label), Sense::click());
        let selected = *value == *option;
        if selected {
            ui.painter().add(p.shadow(1, 4).as_shape(cell, CornerRadius::same(radius::SM)));
            ui.painter().rect(
                cell,
                CornerRadius::same(radius::SM),
                if p.dark { p.card_hover } else { p.card },
                Stroke::new(1.0, p.border),
                StrokeKind::Inside,
            );
        } else if r.hovered() {
            ui.painter().rect_filled(cell, CornerRadius::same(radius::SM), p.card_hover.gamma_multiply(0.6));
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
    let frame = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(radius::XL - 2))
        .inner_margin(egui::Margin::same(16));
    // Light themes lift cards off the canvas with a soft shadow; dark ones rely on the fill.
    if p.dark {
        frame
    } else {
        frame.shadow(Shadow { offset: [0, 1], blur: 6, spread: 0, color: Color32::from_black_alpha(10) })
    }
}

/// Upper-case text in the caption style, for painting.
pub fn caps_galley(ui: &Ui, text: &str, size: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: FontId::new(size, crate::theme::semi()),
            extra_letter_spacing: CAPS_TRACKING * size / 11.0,
            color,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// Small upper-case section caption, letter-spaced.
pub fn caption(ui: &mut Ui, text: &str, p: &Palette) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .family(crate::theme::semi())
            .extra_letter_spacing(CAPS_TRACKING)
            .color(p.text_dim),
    );
}

/// A card title row: caption on the left and an optional action link on the
/// right. Returns whether the link was clicked.
pub fn card_header(ui: &mut Ui, title: &str, link: Option<&str>, p: &Palette) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        caption(ui, title, p);
        if let Some(link) = link {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let galley = ui.painter().layout_no_wrap(link.to_owned(), FontId::proportional(12.5), p.accent);
                let (rect, r) = ui.allocate_exact_size(galley.size() + vec2(18.0, 4.0), Sense::click());
                let color = if r.hovered() { p.text } else { p.text_dim };
                ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, link, FontId::proportional(12.5), color);
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(rect.right_center() - vec2(6.0, 0.0), Vec2::splat(12.0)),
                    Icon::External,
                    color,
                );
                clicked = r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            });
        }
    });
    ui.add_space(6.0);
    clicked
}

/// A sidebar section title that folds its section away when clicked.
/// Returns whether the section is open; the choice is remembered.
pub fn section(ui: &mut Ui, text: &str, p: &Palette) -> bool {
    let id = egui::Id::unique(("stewardget-section", text));
    let open = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, true));
    let width = ui.available_width().min(150.0);
    let (rect, r) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click());
    let color = if r.hovered() { p.text } else { lerp_color(p.text_dim, p.sidebar, 0.15) };
    let galley = caps_galley(ui, &text.to_uppercase(), 10.5, color);
    let text_w = galley.size().x;
    let x = rect.left() + 10.0;
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
    if r.hovered() {
        let caret = if open { Icon::CaretDown } else { Icon::CaretUp };
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(x + text_w + 10.0, rect.center().y), vec2(10.0, 10.0)),
            caret,
            color,
        );
    }
    if r.on_hover_text(if open { "Fold away" } else { "Show" }).clicked() {
        ui.ctx().data_mut(|d| d.insert_persisted(id, !open));
        return !open;
    }
    open
}

/// Metric tile: icon, label, a large value and an optional caption.
pub fn stat_tile(ui: &mut Ui, p: &Palette, label: &str, value: &str, caption_text: &str, color: Color32) {
    stat_tile_with_icon(ui, p, None, label, value, caption_text, color);
}

pub fn stat_tile_with_icon(
    ui: &mut Ui,
    p: &Palette,
    icon: Option<Icon>,
    label: &str,
    value: &str,
    caption_text: &str,
    color: Color32,
) {
    card(p).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            if let Some(icon) = icon {
                icon_tile(ui, icon, 26.0, color, p);
                ui.add_space(2.0);
            }
            ui.label(egui::RichText::new(label).size(12.5).color(p.text_dim));
        });
        ui.add_space(4.0);
        ui.label(egui::RichText::new(value).size(crate::theme::size::XXL).family(crate::theme::semi()).color(p.text));
        // Always one line, so tiles side by side have the same height.
        let caption_text = if caption_text.is_empty() { "\u{a0}" } else { caption_text };
        ui.label(egui::RichText::new(caption_text).size(12.0).color(p.text_dim));
    });
}

/// Coloured status dot.
pub fn dot(painter: &egui::Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, 3.5, color);
}

/// Paints a status pill (dot and label on a tint of `color`) with its left
/// edge at `left_center`, clipped to `max_w`. Returns its rect.
pub fn paint_pill(
    painter: &egui::Painter,
    left_center: Pos2,
    text: &str,
    color: Color32,
    max_w: f32,
    p: &Palette,
) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::new(12.0, crate::theme::semi()), color);
    let w = (galley.size().x + 26.0).min(max_w);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 11.0), vec2(w, 22.0));
    painter.rect_filled(rect, CornerRadius::same(11), p.tint(color));
    painter.circle_filled(rect.left_center() + vec2(10.0, 0.0), 3.0, color);
    painter.with_clip_rect(rect.shrink(1.0)).galley(
        pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    rect
}

/// Pill-shaped status badge.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(12.0, crate::theme::semi()), color);
    let size = galley.size() + vec2(18.0, 6.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), color.gamma_multiply(0.15));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// A small toggle chip: tinted with the accent when on.
pub fn chip(ui: &mut Ui, label: &str, on: bool, p: &Palette) -> Response {
    let family = if on { crate::theme::semi() } else { egui::FontFamily::Proportional };
    let font = FontId::new(12.5, family);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), p.text);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(20.0, 10.0), Sense::click());
    let (fill, fg, stroke) = if on {
        (p.selected_fill(), p.accent, Stroke::new(1.0, p.accent.gamma_multiply(0.55)))
    } else if response.hovered() {
        (p.card_hover, p.text, Stroke::new(1.0, p.control_border))
    } else {
        (p.card, p.text_dim, Stroke::new(1.0, p.border))
    };
    ui.painter().rect(rect, CornerRadius::same(255), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
