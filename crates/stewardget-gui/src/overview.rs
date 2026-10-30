//! Overview page: what is happening now, what finished lately, and what the
//! history says about the user's downloading habits.

use egui::{Align, Layout, Margin, RichText, Sense, Ui, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_relative, format_speed, now_unix};
use stewardget_core::{DownloadView, Status};

use crate::app::{ListFilter, Page, StewardGetApp};
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind, card};

impl StewardGetApp {
    pub fn overview_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        self.history.refresh(&self.engine);
        let now = now_unix();
        let snap = self.snap.clone();

        let active: Vec<DownloadView> = snap.downloads.iter().filter(|d| d.status.is_active()).cloned().collect();
        let waiting = snap.count(|s| matches!(s, Status::Queued | Status::Paused));
        let left: u64 = snap
            .downloads
            .iter()
            .filter(|d| d.status.is_active() || d.status == Status::Queued)
            .filter_map(|d| d.total.map(|t| t.saturating_sub(d.downloaded)))
            .sum();
        let eta = (snap.speed > 1.0 && left > 0).then(|| (left as f64 / snap.speed) as u64);
        let week: u64 = snap.stats.recent_days(7, now).iter().map(|d| d.bytes).sum();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 20 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 6.0);
                ui.label(
                    RichText::new("Overview")
                        .size(crate::theme::size::XL + 2.0)
                        .family(crate::theme::semi())
                        .color(p.text),
                );
                ui.label(RichText::new(greeting(&snap, waiting)).size(crate::theme::size::SM).color(p.text_dim));
                ui.add_space(12.0);
                // Hero: the live speed over its own graph, lit by a soft glow of the accent.
                card(&p).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let glow = ui.painter().add(egui::Shape::Noop);
                    let top_left = ui.min_rect().min;
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            widgets::caption(ui, "Right now", &p);
                            let live = snap.speed > 1.0;
                            let value = if live { format_speed(snap.speed) } else { "Idle".to_owned() };
                            let color = if live { p.text } else { p.text_dim };
                            ui.label(
                                RichText::new(value)
                                    .size(crate::theme::size::DISPLAY)
                                    .family(crate::theme::semi())
                                    .color(color),
                            );
                            let line = match (active.len(), eta) {
                                (0, _) if waiting > 0 => format!("{waiting} waiting to start"),
                                (0, _) => "Nothing is downloading. Paste a link to begin.".to_owned(),
                                (n, Some(eta)) => {
                                    format!(
                                        "{n} downloading · {} left · about {}",
                                        format_bytes(left),
                                        format_duration(eta)
                                    )
                                }
                                (n, None) => format!("{n} downloading"),
                            };
                            ui.label(RichText::new(line).color(p.text_dim));
                            ui.add_space(5.0);
                            ui.horizontal_wrapped(|ui| {
                                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Plus), "Add download", true, &p)
                                    .clicked()
                                {
                                    self.open_add(None);
                                }
                                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Paste), "Paste links", true, &p)
                                    .clicked()
                                {
                                    self.paste_links();
                                }
                            });
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let w = (ui.available_width() - 20.0).clamp(120.0, 560.0);
                            widgets::area_chart(ui, &snap.history, vec2(w, 112.0), p.accent2, &p);
                        });
                    });
                    let card_rect = ui.min_rect();
                    let glow_rect = egui::Rect::from_min_size(
                        top_left + vec2(40.0, 30.0),
                        vec2(card_rect.width() * 0.35, (card_rect.height() - 60.0).max(10.0)),
                    );
                    let strength = if snap.speed > 1.0 { 0.32 } else { 0.16 };
                    ui.painter().set(
                        glow,
                        egui::Shadow {
                            offset: [0, 0],
                            blur: 160,
                            spread: 0,
                            color: p.accent.gamma_multiply(if p.dark { strength * 0.5 } else { strength * 0.3 }),
                        }
                        .as_shape(glow_rect, egui::CornerRadius::same(20)),
                    );
                });
                ui.add_space(12.0);

                let per_row = if ui.available_width() > 760.0 { 4 } else { 2 };
                let tile_w = (ui.available_width() - 12.0 * (per_row - 1) as f32) / per_row as f32;
                let days: Vec<f32> = snap.stats.recent_days(14, now).iter().map(|d| d.bytes as f32).collect();
                let queued = snap.count(|s| s == Status::Queued);
                let success = snap.stats.success_rate();
                let shown = snap.downloads.len().max(1) as f32;
                let tiles = [
                    (
                        Icon::Download,
                        "Today",
                        format_bytes(snap.stats.today(now)),
                        "Downloaded since midnight".to_owned(),
                        p.accent,
                        None,
                    ),
                    (
                        Icon::Calendar,
                        "Last 7 days",
                        format_bytes(week),
                        format!("{} this month", format_bytes(snap.stats.month_total(now))),
                        p.accent2,
                        None,
                    ),
                    (
                        Icon::Clock,
                        "Waiting",
                        waiting.to_string(),
                        format!("{queued} queued \u{00b7} {} paused", waiting.saturating_sub(queued)),
                        p.yellow,
                        Some(waiting as f32 / shown),
                    ),
                    (
                        Icon::CheckCircle,
                        "Finished",
                        snap.stats.completed.to_string(),
                        success.map_or("All time".to_owned(), |r| format!("{:.0}% of downloads succeed", r * 100.0)),
                        p.green,
                        Some(success.unwrap_or(0.0) as f32),
                    ),
                ];
                for (i, chunk) in tiles.chunks(per_row).enumerate() {
                    ui.horizontal(|ui| {
                        for (j, (icon, label, value, sub, color, meter)) in chunk.iter().enumerate() {
                            // The two volume tiles show the last two weeks, or the last one.
                            let extra = match meter {
                                Some(f) => widgets::KpiExtra::Meter(*f),
                                None if i * per_row + j == 0 => widgets::KpiExtra::Spark(&days),
                                None => widgets::KpiExtra::Spark(&days[days.len().saturating_sub(7)..]),
                            };
                            widgets::kpi_tile(ui, tile_w, *icon, *color, label, value, sub, extra, &p);
                        }
                    });
                    ui.add_space(12.0);
                }

                self.data_cap_card(ui);
                self.links_card(ui);
                let wide = ui.available_width() > 880.0;
                let now_block = |this: &mut Self, ui: &mut Ui| this.overview_active(ui, &active);
                let recent_block = |this: &mut Self, ui: &mut Ui| this.overview_recent(ui, now);
                if wide {
                    ui.columns_const(|[a, b]: &mut [Ui; 2]| {
                        now_block(self, a);
                        recent_block(self, b);
                    });
                } else {
                    now_block(self, ui);
                    ui.add_space(12.0);
                    recent_block(self, ui);
                }
                ui.add_space(12.0);
                self.overview_habits(ui);
            });
        });
    }

    /// Opens the add window with the links on the clipboard.
    fn paste_links(&mut self) {
        let text = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok());
        self.open_add(text.filter(|t| t.contains("://")));
    }

    fn overview_active(&mut self, ui: &mut Ui, active: &[DownloadView]) {
        let p = self.palette;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if widgets::card_header(ui, "Downloading", Some("See all"), &p) {
                self.page = Page::Downloads;
                self.filter = ListFilter::All;
            }
            if active.is_empty() {
                empty_line(ui, &p, Icon::Download, "Nothing is downloading right now.");
            }
            for d in active.iter().take(6) {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
                let painter = ui.painter();
                if response.hovered() {
                    painter.rect_filled(
                        rect.expand2(vec2(6.0, 0.0)),
                        egui::CornerRadius::same(crate::theme::radius::MD),
                        p.card_hover,
                    );
                }
                let color = p.category(d.category);
                widgets::paint_file_tile(
                    painter,
                    egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 6.0), vec2(36.0, 36.0)),
                    Icon::for_category(d.category),
                    widgets::extension(&d.file_name),
                    color,
                    &p,
                );
                let text_x = 46.0;
                let name_w = rect.width() - 170.0;
                let name = widgets::fit_text(
                    painter,
                    &d.file_name,
                    &egui::FontId::new(13.5, crate::theme::semi()),
                    name_w - text_x,
                );
                painter.text(
                    rect.left_top() + vec2(text_x, 14.0),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::new(13.5, crate::theme::semi()),
                    p.text,
                );
                let right =
                    format!("{}  \u{00b7}  {}", format_speed(d.speed), d.eta.map_or("—".into(), format_duration));
                painter.text(
                    rect.right_top() + vec2(0.0, 14.0),
                    egui::Align2::RIGHT_CENTER,
                    right,
                    egui::FontId::proportional(12.5),
                    p.text_dim,
                );
                widgets::paint_progress(
                    ui,
                    egui::Rect::from_min_size(rect.left_top() + vec2(text_x, 29.0), vec2(rect.width() - text_x, 6.0)),
                    d,
                    &p,
                );
                if let Some(insight) = self.insight_for(d) {
                    let c = match insight.tone {
                        stewardget_core::InsightTone::Good => p.green,
                        stewardget_core::InsightTone::Info => p.text_dim,
                        stewardget_core::InsightTone::Warn => p.yellow,
                    };
                    ui.painter().text(
                        rect.left_top() + vec2(text_x, 47.0),
                        egui::Align2::LEFT_CENTER,
                        insight.headline,
                        egui::FontId::proportional(12.0),
                        c,
                    );
                }
                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.page = Page::Downloads;
                    self.selection.clear();
                    self.selection.insert(d.id);
                    self.cursor = Some(d.id);
                    self.anchor = Some(d.id);
                    self.scroll_to = Some(d.id);
                }
            }
        });
    }

    fn overview_recent(&mut self, ui: &mut Ui, now: u64) {
        let p = self.palette;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if widgets::card_header(ui, "Recently finished", Some("History"), &p) {
                self.page = Page::History;
            }
            let recent: Vec<_> = self.history.entries().iter().take(6).cloned().collect();
            if recent.is_empty() {
                empty_line(ui, &p, Icon::CheckCircle, "Finished downloads will show up here.");
            }
            for e in recent {
                let ok = e.status == Status::Completed;
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
                let painter = ui.painter();
                if response.hovered() {
                    painter.rect_filled(
                        rect.expand2(vec2(6.0, 0.0)),
                        egui::CornerRadius::same(crate::theme::radius::MD),
                        p.card_hover,
                    );
                }
                let color = if ok { p.category(e.category) } else { p.red };
                widgets::paint_file_tile(
                    painter,
                    egui::Rect::from_min_size(rect.left_center() - vec2(0.0, 16.0), vec2(32.0, 32.0)),
                    if ok { Icon::for_category(e.category) } else { Icon::Alert },
                    "",
                    color,
                    &p,
                );
                painter.text(
                    rect.left_center() + vec2(42.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    widgets::fit_text(painter, &e.file_name, &egui::FontId::proportional(13.5), rect.width() - 190.0),
                    egui::FontId::proportional(13.5),
                    p.text,
                );
                let meta = format!(
                    "{}  \u{00b7}  {}",
                    e.size.map_or("—".into(), format_bytes),
                    format_relative(e.finished_at, now)
                );
                painter.text(
                    rect.right_center(),
                    egui::Align2::RIGHT_CENTER,
                    meta,
                    egui::FontId::proportional(12.0),
                    p.text_dim,
                );
                if response.on_hover_text("Open").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && ok {
                    self.open_path(&e.path);
                }
            }
        });
    }

    /// Patterns in the history: busiest time of day, fastest and slowest sources.
    fn overview_habits(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let hours = self.history.hours();
        let hosts = self.history.fastest_hosts(4);
        if hours.iter().all(|h| *h == 0) {
            return;
        }
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            widgets::caption(ui, "WHEN YOU DOWNLOAD", &p);
            ui.add_space(4.0);
            widgets::hour_bars(ui, &hours, &p, 72.0);
            let busiest = hours.iter().enumerate().max_by_key(|(_, n)| **n).map_or(0, |(h, _)| h);
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "Most downloads finish around {busiest:02}:00. Schedule big ones for the quiet hours with a queue."
                ))
                .size(12.0)
                .color(p.text_dim),
            );
            self.overnight_suggestion(ui, &hours);
            if !hosts.is_empty() {
                ui.add_space(5.0);
                widgets::caption(ui, "FASTEST SOURCES", &p);
                let max = hosts.iter().map(|h| h.average_speed()).fold(1.0, f64::max);
                for h in &hosts {
                    widgets::split_row(
                        ui,
                        |ui| {
                            ui.add(egui::Label::new(RichText::new(&h.host).color(p.text)).truncate());
                        },
                        |ui| {
                            ui.label(RichText::new(format_speed(h.average_speed())).size(12.0).color(p.text_dim));
                        },
                    );
                    widgets::share_bar(ui, (h.average_speed() / max) as f32, p.accent2, &p);
                    ui.add_space(2.0);
                }
            }
        });
    }
}

impl StewardGetApp {
    /// This month's data against the limit, when there is one.
    fn data_cap_card(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let (used, cap, hit) = self.engine.month_usage();
        if cap == 0 {
            return;
        }
        let color = if hit {
            p.red
        } else if used * 10 >= cap * 8 {
            p.yellow
        } else {
            p.accent2
        };
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::caption(ui, "DATA THIS MONTH", &p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if hit && widgets::button(ui, ButtonKind::Tool, Some(Icon::Gear), "Change the limit", true, &p).clicked() {
                        self.open_settings(crate::settings::Tab::Connection);
                    }
                    ui.label(RichText::new(format!("{} of {}", format_bytes(used), format_bytes(cap))).color(color));
                });
            });
            widgets::share_bar(ui, (used as f64 / cap as f64) as f32, color, &p);
            if hit {
                ui.label(
                    RichText::new("The monthly limit is used up, so downloads are paused. They continue next month, or when you raise the limit.")
                        .size(12.0)
                        .color(p.red),
                );
            }
        });
        ui.add_space(5.0);
    }

    /// Offers a queue that runs in the hours the connection is least used.
    fn overnight_suggestion(&mut self, ui: &mut Ui, hours: &[u32; 24]) {
        let p = self.palette;
        let config = self.engine.config();
        if config.queues.iter().any(|q| q.schedule.enabled) {
            return;
        }
        let Some((start, _)) = stewardget_core::quietest_in(hours, 5) else { return };
        let end = (start + 5) % 24;
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            widgets::icon(ui, Icon::Clock, 15.0, p.accent2);
            ui.label(
                RichText::new(format!(
                    "Your quietest hours are {start:02}:00 to {end:02}:00. Put big downloads in a queue that runs then."
                ))
                .size(12.0)
                .color(p.text_dim),
            );
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Plus), "Create that queue", true, &p).clicked() {
                let mut next = config.clone();
                let id = next.queues.iter().map(|q| q.id).max().unwrap_or(0) + 1;
                next.queues.push(stewardget_core::Queue {
                    id,
                    name: "Quiet hours".into(),
                    schedule: stewardget_core::Schedule {
                        enabled: true,
                        start_minute: u16::from(start) * 60,
                        stop_minute: Some(u16::from(end) * 60),
                        days: [true; 7],
                    },
                    ..Default::default()
                });
                let green = p.green;
                match self.engine.update_config(next) {
                    Ok(()) => {
                        self.toast("Queue “Quiet hours” created. Add downloads to it from the Add window.", green)
                    }
                    Err(e) => {
                        let red = p.red;
                        self.toast(e.to_string(), red);
                    }
                }
            }
        });
    }
}

/// One line under the page title saying how things stand.
fn greeting(snap: &stewardget_core::EngineSnapshot, waiting: usize) -> String {
    let active = snap.count(Status::is_active);
    let failed = snap.count(|s| s == Status::Failed);
    let mut parts = Vec::new();
    match active {
        0 => parts.push("Nothing is downloading".to_owned()),
        1 => parts.push("1 download in progress".to_owned()),
        n => parts.push(format!("{n} downloads in progress")),
    }
    if waiting > 0 {
        parts.push(format!("{waiting} waiting"));
    }
    if failed > 0 {
        parts.push(format!("{failed} need attention"));
    }
    parts.join(" \u{00b7} ")
}

/// A quiet placeholder line inside a card with nothing to list.
fn empty_line(ui: &mut Ui, p: &crate::theme::Palette, icon: Icon, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        widgets::icon_tile(ui, icon, 30.0, p.text_dim, p);
        ui.add_space(4.0);
        ui.label(RichText::new(text).color(p.text_dim));
    });
    ui.add_space(6.0);
}
