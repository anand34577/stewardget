//! StewardGet's own window frame: the header doubles as the title bar, with
//! window buttons, dragging, double-click to maximise and resizable edges.
//! Used unless the system title bar is chosen in Settings.

use egui::viewport::ResizeDirection;
use egui::{Color32, CornerRadius, Id, Order, Rect, Response, Sense, Stroke, Ui, ViewportCommand, pos2, vec2};

use crate::theme::Palette;

/// Whether StewardGet draws the window buttons itself (macOS keeps its traffic lights).
pub const OWN_BUTTONS: bool = !cfg!(target_os = "macos");

/// Width kept free for the macOS traffic lights at the top left.
pub const TRAFFIC_LIGHTS_W: f32 = if cfg!(target_os = "macos") { 72.0 } else { 0.0 };

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// Makes the empty parts of `rect` move the window, and a double-click
/// maximise or restore it. Call before adding the widgets on top of it.
pub fn drag_area(ui: &Ui, rect: Rect, id: &str) {
    let r = ui.interact(rect, Id::unique(("stewardget-titlebar", id)), Sense::click_and_drag());
    if r.double_clicked() {
        let max = maximized(ui.ctx());
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    } else if r.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Minimise, maximise/restore and close, laid out right to left.
pub fn window_buttons(ui: &mut Ui, p: &Palette) {
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.x = 2.0;
    if caption_button(ui, Caption::Close, "Close", p).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let max = maximized(&ctx);
    let (kind, tip) = if max { (Caption::Restore, "Restore") } else { (Caption::Maximize, "Maximise") };
    if caption_button(ui, kind, tip, p).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if caption_button(ui, Caption::Minimize, "Minimise", p).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

fn caption_button(ui: &mut Ui, kind: Caption, tip: &str, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(42.0, 34.0), Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let close = kind == Caption::Close;
    if hovered {
        let fill = match (close, pressed) {
            (true, false) => Color32::from_rgb(220, 38, 38),
            (true, true) => Color32::from_rgb(185, 28, 28),
            (false, false) => p.card_hover,
            (false, true) => p.track,
        };
        painter.rect_filled(rect, CornerRadius::same(crate::theme::radius::MD), fill);
    }
    let color = if close && hovered {
        Color32::WHITE
    } else if hovered {
        p.text
    } else {
        p.text_dim
    };
    let stroke = Stroke::new(1.3, color);
    let c = rect.center();
    let s = 5.0;
    match kind {
        Caption::Minimize => {
            painter.line_segment([c + vec2(-s, 0.0), c + vec2(s, 0.0)], stroke);
        }
        Caption::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(2.0 * s, 2.0 * s)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Caption::Restore => {
            let front = Rect::from_center_size(c + vec2(-1.5, 1.5), vec2(2.0 * s - 2.0, 2.0 * s - 2.0));
            painter.rect_stroke(front, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
            // The window behind: only its top and right edges show.
            let (tl, tr, br) = (
                pos2(front.left() + 3.0, front.top() - 3.0),
                front.right_top() + vec2(3.0, -3.0),
                pos2(front.right() + 3.0, front.bottom() - 3.0),
            );
            painter.line_segment([tl, tr], stroke);
            painter.line_segment([tr, br], stroke);
        }
        Caption::Close => {
            painter.line_segment([c + vec2(-s, -s), c + vec2(s, s)], stroke);
            painter.line_segment([c + vec2(-s, s), c + vec2(s, -s)], stroke);
        }
    }
    response.on_hover_text(tip)
}

/// Invisible handles along the window's edges that start an OS resize.
pub fn resize_edges(ctx: &egui::Context) {
    if !OWN_BUTTONS || maximized(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    let (m, c) = (5.0, 10.0);
    let (l, r, t, b) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    let edges = [
        (Rect::from_min_max(pos2(l, t), pos2(l + c, t + c)), ResizeDirection::NorthWest),
        (Rect::from_min_max(pos2(r - c, t), pos2(r, t + c)), ResizeDirection::NorthEast),
        (Rect::from_min_max(pos2(l, b - c), pos2(l + c, b)), ResizeDirection::SouthWest),
        (Rect::from_min_max(pos2(r - c, b - c), pos2(r, b)), ResizeDirection::SouthEast),
        (Rect::from_min_max(pos2(l + c, t), pos2(r - c, t + m)), ResizeDirection::North),
        (Rect::from_min_max(pos2(l + c, b - m), pos2(r - c, b)), ResizeDirection::South),
        (Rect::from_min_max(pos2(l, t + c), pos2(l + m, b - c)), ResizeDirection::West),
        (Rect::from_min_max(pos2(r - m, t + c), pos2(r, b - c)), ResizeDirection::East),
    ];
    for (i, (rect, dir)) in edges.into_iter().enumerate() {
        // Corners are small squares, edges thin strips; all above everything else.
        egui::Area::new(Id::unique(("stewardget-resize", i))).order(Order::Foreground).fixed_pos(rect.min).show(
            ctx,
            |ui| {
                let (_, r) = ui.allocate_exact_size(rect.size(), Sense::drag());
                if r.hovered() || r.dragged() {
                    ctx.set_cursor_icon(cursor(dir));
                }
                if r.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
                    ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
                }
            },
        );
    }
}

fn cursor(dir: ResizeDirection) -> egui::CursorIcon {
    use egui::CursorIcon as C;
    match dir {
        ResizeDirection::North => C::ResizeNorth,
        ResizeDirection::South => C::ResizeSouth,
        ResizeDirection::East => C::ResizeEast,
        ResizeDirection::West => C::ResizeWest,
        ResizeDirection::NorthEast => C::ResizeNorthEast,
        ResizeDirection::NorthWest => C::ResizeNorthWest,
        ResizeDirection::SouthEast => C::ResizeSouthEast,
        ResizeDirection::SouthWest => C::ResizeSouthWest,
    }
}

/// A hairline around the window where it has no system border of its own.
/// Windows keeps one (with rounded corners and a shadow) for frameless
/// windows; a second line there would not meet it at the corners.
pub fn outline(ctx: &egui::Context, p: &Palette) {
    if !OWN_BUTTONS || cfg!(windows) || maximized(ctx) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("stewardget-outline")));
    painter.rect_stroke(
        ctx.content_rect(),
        CornerRadius::ZERO,
        Stroke::new(1.0, if p.dark { p.border } else { p.control_border }),
        egui::StrokeKind::Inside,
    );
}
