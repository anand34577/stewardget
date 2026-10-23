//! Rendering for the terminal dashboard.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState, Tabs, Wrap};
use stewardget_core::util::{format_bytes, format_duration, format_speed, format_timestamp};
use stewardget_core::{DownloadView, Status};

use super::{App, Filter, InputKind, Mode};

const ACCENT: Color = Color::Rgb(139, 92, 246);
const CYAN: Color = Color::Rgb(34, 211, 238);
const GREEN: Color = Color::Rgb(52, 211, 153);
const YELLOW: Color = Color::Rgb(251, 191, 36);
const RED: Color = Color::Rgb(248, 113, 113);
const DIM: Color = Color::Rgb(113, 113, 135);
const SURFACE: Color = Color::Rgb(39, 39, 58);

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let details_height = if area.height >= 24 { 11 } else { 0 };
    let [header, tabs, body, details, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(details_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);
    draw_tabs(f, app, tabs);
    draw_table(f, app, body);
    if details_height > 0 {
        draw_details(f, app, details);
    }
    draw_footer(f, app, footer);

    match &app.mode {
        Mode::Normal => {}
        Mode::Help => draw_help(f, area),
        Mode::Input { kind, buffer } => draw_input(f, area, kind, buffer),
        Mode::Confirm { name, delete_file, .. } => draw_confirm(f, area, name, *delete_file),
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .border_set(border::ROUNDED)
        .border_style(Style::new().fg(SURFACE))
        .title(Span::styled(format!(" {title} "), Style::new().fg(DIM).bold()))
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.snap;
    let active = s.count(Status::is_active);
    let waiting = s.count(|st| st == Status::Queued);
    let done = s.count(|st| st == Status::Completed);
    let failed = s.count(|st| st == Status::Failed);
    let limit = if s.speed_limit == 0 { "∞".to_owned() } else { format_speed(s.speed_limit as f64) };
    let sep = || Span::styled("  │  ", Style::new().fg(SURFACE));
    let line = Line::from(vec![
        Span::styled("↓ ", Style::new().fg(CYAN)),
        Span::styled(format_speed(s.speed), Style::new().fg(CYAN).bold()),
        sep(),
        Span::styled(format!("● {active} active"), Style::new().fg(ACCENT)),
        Span::raw("  "),
        Span::styled(format!("◌ {waiting} queued"), Style::new().fg(DIM)),
        Span::raw("  "),
        Span::styled(format!("✓ {done} done"), Style::new().fg(GREEN)),
        Span::raw("  "),
        Span::styled(format!("✗ {failed} failed"), Style::new().fg(if failed > 0 { RED } else { DIM })),
        sep(),
        Span::styled("limit ", Style::new().fg(DIM)),
        Span::raw(limit),
        sep(),
        Span::styled("today ", Style::new().fg(DIM)),
        Span::raw(format_bytes(s.stats.today(stewardget_core::util::now_unix()))),
        sep(),
        match &s.api {
            stewardget_core::ApiState::Listening(_) => Span::styled("● browser", Style::new().fg(GREEN)),
            stewardget_core::ApiState::Disabled => Span::styled("○ browser off", Style::new().fg(DIM)),
            stewardget_core::ApiState::Failed(_) => Span::styled("● browser error", Style::new().fg(RED)),
        },
    ]);
    let block =
        Block::bordered().border_set(border::ROUNDED).border_style(Style::new().fg(ACCENT)).title(Line::from(vec![
            Span::styled(" STEWARDGET ", Style::new().fg(Color::White).bg(ACCENT).bold()),
            Span::styled(concat!(" v", env!("CARGO_PKG_VERSION"), " "), Style::new().fg(DIM)),
        ]));
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Filter::ALL
        .iter()
        .map(|flt| {
            let n = app.snap.count(|s| flt.matches(s));
            Line::from(format!(" {} {n} ", flt.label()))
        })
        .collect();
    let selected = Filter::ALL.iter().position(|x| *x == app.filter);
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::new().fg(DIM))
        .highlight_style(Style::new().fg(Color::White).bg(SURFACE).bold())
        .divider(Span::styled("·", Style::new().fg(SURFACE)))
        .padding("", "");
    f.render_widget(tabs, area);
}

fn status_span(d: &DownloadView) -> Span<'static> {
    if d.awaiting_link {
        return Span::styled("… Waiting link", Style::new().fg(YELLOW));
    }
    if let Some(secs) = d.retry_in {
        return Span::styled(format!("⟳ Retry in {secs}s"), Style::new().fg(YELLOW));
    }
    let (icon, color) = match d.status {
        Status::Queued => ("◌", DIM),
        Status::Connecting => ("⇄", CYAN),
        Status::Downloading => ("↓", CYAN),
        Status::Verifying => ("✱", ACCENT),
        Status::Paused => ("‖", YELLOW),
        Status::Completed => ("✓", GREEN),
        Status::Failed => ("✗", RED),
    };
    Span::styled(format!("{icon} {}", d.status.label()), Style::new().fg(color))
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Completed => GREEN,
        Status::Failed => RED,
        Status::Paused => YELLOW,
        Status::Queued => DIM,
        _ => ACCENT,
    }
}

fn progress_line(d: &DownloadView, width: usize) -> Line<'static> {
    let color = status_color(d.status);
    match d.fraction() {
        Some(frac) => {
            let filled = ((frac * width as f32).round() as usize).min(width);
            Line::from(vec![
                Span::styled("━".repeat(filled), Style::new().fg(color)),
                Span::styled("─".repeat(width - filled), Style::new().fg(SURFACE)),
                Span::styled(format!(" {:>3.0}%", frac * 100.0), Style::new().fg(Color::White)),
            ])
        }
        None => Line::from(Span::styled(format!("{:─<width$}  ?", ""), Style::new().fg(SURFACE))),
    }
}

fn draw_table(f: &mut Frame, app: &App, area: Rect) {
    let visible = app.visible();
    let block = panel("Downloads");
    if visible.is_empty() {
        let msg = if app.snap.downloads.is_empty() {
            "No downloads yet.

Press  a  to add a URL — or just paste one."
        } else {
            "Nothing here. Press Tab to switch view."
        };
        let inner = block.inner(area);
        f.render_widget(block, area);
        let height = inner.height.min(3);
        let centered = Rect { y: inner.y + (inner.height - height) / 2, height, ..inner };
        f.render_widget(Paragraph::new(msg).alignment(Alignment::Center).style(Style::new().fg(DIM)), centered);
        return;
    }
    // Drop optional columns as the terminal narrows; Name always keeps room.
    let inner = area.width.saturating_sub(4);
    let show_eta = inner >= 92;
    let show_speed = inner >= 76;
    let show_size = inner >= 60;
    let bar_width: usize = if inner >= 76 { 16 } else { 8 };
    let rows = visible.iter().map(|d| {
        let size = match d.total {
            Some(t) => format_bytes(t),
            None if d.downloaded > 0 => format!("{}+", format_bytes(d.downloaded)),
            None => "—".into(),
        };
        let speed = if d.status == Status::Downloading { format_speed(d.speed) } else { String::new() };
        let eta = d.eta.map(format_duration).unwrap_or_default();
        let mut cells = vec![Cell::from(Span::styled(d.file_name.clone(), Style::new().fg(Color::White)))];
        if show_size {
            cells.push(Cell::from(Span::styled(size, Style::new().fg(DIM))));
        }
        cells.push(Cell::from(progress_line(d, bar_width)));
        if show_speed {
            cells.push(Cell::from(Span::styled(speed, Style::new().fg(CYAN))));
        }
        if show_eta {
            cells.push(Cell::from(Span::styled(eta, Style::new().fg(DIM))));
        }
        cells.push(Cell::from(status_span(d)));
        Row::new(cells)
    });
    let mut widths = vec![Constraint::Fill(1)];
    let mut titles = vec!["Name"];
    if show_size {
        widths.push(Constraint::Length(10));
        titles.push("Size");
    }
    widths.push(Constraint::Length(bar_width as u16 + 5));
    titles.push("Progress");
    if show_speed {
        widths.push(Constraint::Length(12));
        titles.push("Speed");
    }
    if show_eta {
        widths.push(Constraint::Length(8));
        titles.push("ETA");
    }
    widths.push(Constraint::Length(14));
    titles.push("Status");
    let header = Row::new(titles).style(Style::new().fg(DIM).add_modifier(Modifier::BOLD)).bottom_margin(0);
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SURFACE))
        .highlight_symbol(Span::styled("▌", Style::new().fg(ACCENT)));
    let mut state = TableState::default().with_selected(app.selected_index());
    f.render_stateful_widget(table, area, &mut state);
}

/// Renders the byte ranges as a heat strip: full blocks for downloaded parts.
fn segment_line(d: &DownloadView, width: usize) -> Line<'static> {
    let Some(total) = d.total.filter(|&t| t > 0) else {
        let text = if d.status.is_active() { "size unknown — single stream" } else { "—" };
        return Line::from(Span::styled(text, Style::new().fg(DIM)));
    };
    if d.segments.is_empty() {
        let done = d.status == Status::Completed;
        let frac = if done { 1.0 } else { d.downloaded as f64 / total as f64 };
        let filled = (frac * width as f64).round() as usize;
        return Line::from(vec![
            Span::styled("█".repeat(filled.min(width)), Style::new().fg(status_color(d.status))),
            Span::styled("░".repeat(width.saturating_sub(filled)), Style::new().fg(SURFACE)),
        ]);
    }
    let cell = total as f64 / width as f64;
    let spans = (0..width).map(|i| {
        let (a, b) = ((i as f64 * cell) as u64, (((i + 1) as f64) * cell) as u64);
        let mut covered = 0u64;
        let mut active = false;
        for s in &d.segments {
            let done_end = s.start + s.done;
            let lo = a.max(s.start);
            let hi = b.min(done_end);
            if hi > lo {
                covered += hi - lo;
            }
            active |= s.active && done_end >= a && done_end <= b && done_end < s.end;
        }
        let ratio = covered as f64 / (b - a).max(1) as f64;
        let (ch, color) = match ratio {
            _ if active => ("▶", CYAN),
            r if r >= 0.95 => ("█", ACCENT),
            r if r >= 0.5 => ("▓", ACCENT),
            r if r > 0.0 => ("▒", ACCENT),
            _ => ("░", SURFACE),
        };
        Span::styled(ch, Style::new().fg(color))
    });
    Line::from(spans.collect::<Vec<_>>())
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);
    let label = |s: &'static str| Span::styled(format!("{s:<11}"), Style::new().fg(DIM));

    if let Some(d) = app.selected_view() {
        let inner_width = left.width.saturating_sub(2 + 11) as usize;
        let size = match d.total {
            Some(t) => format!("{} of {}", format_bytes(d.downloaded), format_bytes(t)),
            None => format_bytes(d.downloaded),
        };
        let mut lines = vec![
            Line::from(vec![label("Name"), Span::styled(d.file_name.clone(), Style::new().fg(Color::White).bold())]),
            Line::from(vec![label("URL"), Span::styled(d.url.clone(), Style::new().fg(DIM))]),
            Line::from(vec![label("Saved to"), Span::raw(d.path.display().to_string())]),
            Line::from(vec![
                label("Progress"),
                Span::raw(size),
                Span::styled("   connections ", Style::new().fg(DIM)),
                Span::raw(d.connections.to_string()),
                Span::styled("   resumable ", Style::new().fg(DIM)),
                Span::raw(if d.resumable { "yes" } else { "no" }),
            ]),
            Line::from([vec![label("Segments")], segment_line(d, inner_width.saturating_sub(1)).spans].concat()),
            Line::from(vec![
                label("Added"),
                Span::raw(format_timestamp(d.added_at)),
                Span::styled(
                    if d.options.speed_limit > 0 {
                        format!("   limit {}", format_speed(d.options.speed_limit as f64))
                    } else {
                        String::new()
                    },
                    Style::new().fg(YELLOW),
                ),
                Span::styled(if d.checksum_verified { "   checksum ✓" } else { "" }, Style::new().fg(GREEN)),
            ]),
        ];
        if let Some(err) = &d.error {
            lines.push(Line::from(vec![label("Error"), Span::styled(err.clone(), Style::new().fg(RED))]));
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("Details")), left);
    } else {
        f.render_widget(panel("Details"), left);
    }

    let (title, history, current) = match app.selected_view().filter(|d| !d.history.is_empty()) {
        Some(d) => ("Speed · selected", d.history.clone(), d.speed),
        None => ("Speed · all", app.snap.history.clone(), app.snap.speed),
    };
    let width = right.width.saturating_sub(2) as usize;
    let data: Vec<u64> = history.iter().rev().take(width).rev().map(|&v| v as u64).collect();
    let peak = history.iter().copied().fold(0.0f32, f32::max);
    let block = panel(title).title_bottom(
        Line::from(vec![
            Span::styled(format!(" now {} ", format_speed(current)), Style::new().fg(CYAN)),
            Span::styled(format!("peak {} ", format_speed(peak as f64)), Style::new().fg(DIM)),
        ])
        .right_aligned(),
    );
    f.render_widget(Sparkline::default().data(data).style(Style::new().fg(CYAN)).block(block), right);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Some((msg, error, _)) = &app.toast {
        let color = if *error { RED } else { GREEN };
        f.render_widget(Paragraph::new(Span::styled(format!(" {msg}"), Style::new().fg(color).bold())), area);
        return;
    }
    let keys = [
        ("a", "add"),
        ("␣", "pause/resume"),
        ("x", "remove"),
        ("R", "restart"),
        ("u", "refresh link"),
        ("l/L", "limit"),
        ("o", "open folder"),
        ("P/S", "pause/start all"),
        ("Tab", "view"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let spans: Vec<Span> = keys
        .iter()
        .flat_map(|(k, v)| {
            [
                Span::styled(format!(" {k} "), Style::new().fg(Color::White).bg(SURFACE).bold()),
                Span::styled(format!(" {v}  "), Style::new().fg(DIM)),
            ]
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    area.centered(Constraint::Length(width.min(area.width)), Constraint::Length(height.min(area.height)))
}

fn draw_input(f: &mut Frame, area: Rect, kind: &InputKind, buffer: &str) {
    let (title, hint) = match kind {
        InputKind::AddUrls => ("Add downloads", "Paste or type one or more URLs separated by spaces. Enter to add."),
        InputKind::GlobalLimit => ("Global speed limit", "e.g. 500k, 2M, 1.5MiB — empty or 0 for unlimited."),
        InputKind::DownloadLimit(_) => ("Download speed limit", "e.g. 500k, 2M — empty or 0 for unlimited."),
        InputKind::NewUrl(_) => {
            ("Refresh link", "Paste the new link. Downloaded data is kept and the download continues.")
        }
    };
    let rect = popup(area, 76, 7);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_set(border::ROUNDED)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(format!(" {title} "), Style::new().fg(Color::White).bold()));
    let text = Text::from(vec![
        Line::from(Span::styled(hint, Style::new().fg(DIM))),
        Line::default(),
        Line::from(vec![
            Span::styled("› ", Style::new().fg(ACCENT).bold()),
            Span::raw(buffer.to_owned()),
            Span::styled("▏", Style::new().fg(ACCENT).add_modifier(Modifier::SLOW_BLINK)),
        ]),
    ]);
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(block), rect);
}

fn draw_confirm(f: &mut Frame, area: Rect, name: &str, delete_file: bool) {
    let rect = popup(area, 64, 6);
    f.render_widget(Clear, rect);
    let what = if delete_file {
        "Remove and DELETE its files (partial data too)"
    } else {
        "Remove from the list (files and partial data are kept)"
    };
    let block = Block::bordered()
        .border_set(border::ROUNDED)
        .border_style(Style::new().fg(RED))
        .title(Span::styled(" Confirm ", Style::new().fg(Color::White).bold()));
    let text = Text::from(vec![
        Line::from(format!("{what}:")),
        Line::from(Span::styled(name.to_owned(), Style::new().bold())),
        Line::from(vec![
            Span::styled(" y ", Style::new().bg(RED).fg(Color::White).bold()),
            Span::raw(" yes   "),
            Span::styled(" n ", Style::new().bg(SURFACE).bold()),
            Span::raw(" no"),
        ]),
    ]);
    f.render_widget(Paragraph::new(text).alignment(Alignment::Center).block(block), rect);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rows = [
        ("a / Insert", "Add URLs; img[1-50].jpg adds a series"),
        ("Space / p", "Pause or resume the selected download"),
        ("r", "Retry a failed download"),
        ("R", "Restart from scratch"),
        ("u", "Refresh link: paste a new URL"),
        ("w", "Refresh link: wait for browser"),
        ("f", "Resume anyway after the server's file changed"),
        ("x / Delete", "Remove from list (files and partial data kept)"),
        ("X", "Remove and delete files"),
        ("l", "Speed limit for the selected download"),
        ("L", "Global speed limit"),
        ("K / J", "Move up / down in the queue"),
        ("o / Enter", "Open folder / open completed file"),
        ("P / S", "Pause all / start all"),
        ("c", "Clear completed from the list"),
        ("Tab / 1-5", "Switch view"),
        ("↑↓ j k g G", "Navigate"),
        ("q / Esc", "Quit (transfers resume next time)"),
    ];
    let rect = popup(area, 62, rows.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![Span::styled(format!("  {k:<13}"), Style::new().fg(ACCENT).bold()), Span::raw(*v)])
        })
        .collect();
    let block = Block::bordered()
        .border_set(border::ROUNDED)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(" Keyboard shortcuts ", Style::new().fg(Color::White).bold()))
        .title_bottom(Line::from(Span::styled(" any key to close ", Style::new().fg(DIM))).centered());
    f.render_widget(Paragraph::new(lines).block(block.padding(ratatui::widgets::Padding::vertical(1))), rect);
}
