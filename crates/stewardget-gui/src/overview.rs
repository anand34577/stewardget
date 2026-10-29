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
            egui::Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                // Hero: the live speed over its own graph.
                card(&p).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            widgets::caption(ui, "RIGHT NOW", &p);
                            let (value, color) = if snap.speed > 1.0 {
                                (format_speed(snap.speed), p.accent2)
                            } else {
                                ("Idle".to_owned(), p.text_dim)
                            };
                            ui.label(RichText::new(value).size(28.0).family(crate::theme::semi()).color(color));
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
                            ui.horizontal(|ui| {
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
                            let w = (ui.available_width() - 20.0).clamp(120.0, 520.0);
                            widgets::area_chart(ui, &snap.history, vec2(w, 84.0), p.accent2, &p);
                        });
                    });
                });
                ui.add_space(5.0);

                let per_row = if ui.available_width() > 480.0 { 4 } else { 2 };
                let tiles = [
                    ("Today", format_bytes(snap.stats.today(now)), p.accent),
                    ("Last 7 days", format_bytes(week), p.accent2),
                    ("Waiting", waiting.to_string(), p.yellow),
                    ("Finished all-time", snap.stats.completed.to_string(), p.green),
                ];
                for chunk in tiles.chunks(per_row) {
                    ui.columns(per_row, |cols| {
                        for (col, (label, value, color)) in cols.iter_mut().zip(chunk) {
                            widgets::stat_tile(col, &p, label, value, "", *color);
                        }
                    });
                    ui.add_space(6.0);
                }
                ui.add_space(4.0);

                self.links_card(ui);
                ui.add_space(5.0);
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
                    ui.add_space(5.0);
                    recent_block(self, ui);
                }
                ui.add_space(5.0);
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
            ui.horizontal(|ui| {
                widgets::caption(ui, "DOWNLOADING", &p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(RichText::new("See all").size(12.0).color(p.accent)).clicked() {
                        self.page = Page::Downloads;
                        self.filter = ListFilter::All;
                    }
                });
            });
            ui.add_space(4.0);
            if active.is_empty() {
                ui.label(RichText::new("No active downloads.").color(p.text_dim));
            }
            for d in active.iter().take(6) {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                let painter = ui.painter();
                if response.hovered() {
                    painter.rect_filled(rect, egui::CornerRadius::same(8), p.card_hover);
                }
                let color = p.category(d.category);
                crate::icons::paint(
                    painter,
                    egui::Rect::from_center_size(rect.left_center() + vec2(16.0, -6.0), vec2(16.0, 16.0)),
                    Icon::for_category(d.category),
                    color,
                );
                let name_w = rect.width() - 150.0;
                let name = shorten(&d.file_name, (name_w / 7.0) as usize);
                painter.text(
                    rect.left_top() + vec2(34.0, 12.0),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::new(13.0, crate::theme::semi()),
                    p.text,
                );
                let right = format!("{}  ·  {}", format_speed(d.speed), d.eta.map_or("—".into(), format_duration));
                painter.text(
                    rect.right_top() + vec2(-8.0, 12.0),
                    egui::Align2::RIGHT_CENTER,
                    right,
                    egui::FontId::proportional(12.0),
                    p.text_dim,
                );
                widgets::paint_progress(
                    ui,
                    egui::Rect::from_min_size(rect.left_top() + vec2(34.0, 28.0), vec2(rect.width() - 42.0, 6.0)),
                    d,
                    &p,
                );
                if let Some(insight) = self.insight_for(d) {
                    let c = match insight.tone {
                        stewardget_core::InsightTone::Good => p.green,
                        stewardget_core::InsightTone::Info => p.accent2,
                        stewardget_core::InsightTone::Warn => p.yellow,
                    };
                    ui.painter().text(
                        rect.left_top() + vec2(34.0, 41.0),
                        egui::Align2::LEFT_CENTER,
                        insight.headline,
                        egui::FontId::proportional(11.0),
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
            ui.horizontal(|ui| {
                widgets::caption(ui, "RECENTLY FINISHED", &p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(RichText::new("History").size(12.0).color(p.accent)).clicked() {
                        self.page = Page::History;
                    }
                });
            });
            ui.add_space(4.0);
            let recent: Vec<_> = self.history.entries().iter().take(6).cloned().collect();
            if recent.is_empty() {
                ui.label(RichText::new("Finished downloads will show up here.").color(p.text_dim));
            }
            for e in recent {
                let ok = e.status == Status::Completed;
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                let painter = ui.painter();
                if response.hovered() {
                    painter.rect_filled(rect, egui::CornerRadius::same(8), p.card_hover);
                }
                let color = if ok { p.category(e.category) } else { p.red };
                crate::icons::paint(
                    painter,
                    egui::Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(16.0, 16.0)),
                    if ok { Icon::for_category(e.category) } else { Icon::Alert },
                    color,
                );
                painter.text(
                    rect.left_center() + vec2(34.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    shorten(&e.file_name, ((rect.width() - 170.0) / 7.0) as usize),
                    egui::FontId::proportional(13.0),
                    p.text,
                );
                let meta =
                    format!("{}  ·  {}", e.size.map_or("—".into(), format_bytes), format_relative(e.finished_at, now));
                painter.text(
                    rect.right_center() - vec2(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    meta,
                    egui::FontId::proportional(12.0),
                    p.text_dim,
                );
                if response.on_hover_text("Open").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && ok {
                    let _ = stewardget_core::util::open_with_system(&e.path);
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
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(&h.host).color(p.text)).truncate());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(format_speed(h.average_speed())).size(12.0).color(p.text_dim));
                        });
                    });
                    widgets::share_bar(ui, (h.average_speed() / max) as f32, p.accent2, &p);
                    ui.add_space(2.0);
                }
            }
        });
    }
}

impl StewardGetApp {
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

fn shorten(s: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(8);
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        s.chars().take(max_chars - 1).collect::<String>() + "…"
    }
}
