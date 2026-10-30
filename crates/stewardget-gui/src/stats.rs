//! Statistics page.

use std::collections::HashMap;

use egui::{Align, Layout, Margin, RichText, Ui, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_speed, format_timestamp, now_unix};
use stewardget_core::{Category, Status};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, card};

/// Draws a card whose height matches the other cards of the same `key`, so
/// cards side by side end on the same line. Heights are learned one frame
/// behind: each card reports its natural height, the tallest one wins.
fn even_card(ui: &mut Ui, p: &crate::theme::Palette, key: &str, even: bool, add: impl FnOnce(&mut Ui)) {
    card(p).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let natural = ui.vertical(add).response.rect.height();
        if !even {
            return;
        }
        let ctx = ui.ctx().clone();
        let id = egui::Id::unique(("stewardget-even-card", key));
        let pass = ctx.cumulative_pass_nr();
        let (seen, now, before): (u64, f32, f32) = ctx.data(|d| d.get_temp(id)).unwrap_or((pass, 0.0, 0.0));
        let (now, before) = if seen == pass { (now, before) } else { (0.0, now) };
        let now = now.max(natural);
        ctx.data_mut(|d| d.insert_temp(id, (pass, now, before)));
        if (before - now).abs() > 0.5 && seen != pass {
            ctx.request_repaint();
        }
        let pad = before.max(natural) - natural - ui.spacing().item_spacing.y;
        if pad > 0.5 {
            ui.allocate_space(vec2(1.0, pad));
        }
    });
}

impl StewardGetApp {
    pub fn stats_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let s = self.snap.stats.clone();
        let now = now_unix();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 20 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Statistics")
                            .size(crate::theme::size::XL + 2.0)
                            .family(crate::theme::semi())
                            .color(p.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Stored only on this device, never sent anywhere")
                                .size(crate::theme::size::SM)
                                .color(p.text_dim),
                        );
                        widgets::icon(ui, Icon::Lock, 13.0, p.green);
                    });
                });
                let since = if s.first_seen > 0 {
                    format!("Counting since {}", &format_timestamp(s.first_seen)[..10])
                } else {
                    "Nothing downloaded yet".to_owned()
                };
                ui.label(RichText::new(since).size(crate::theme::size::SM).color(p.text_dim));
                ui.add_space(12.0);

                // Four tiles, one row when there is room: never squeezed until values wrap.
                let volume: Vec<f32> = s.recent_days(30, now).iter().map(|d| d.bytes as f32).collect();
                let success = s.success_rate();
                let tiles = [
                    (
                        Icon::Download,
                        "Total downloaded",
                        format_bytes(s.total_bytes),
                        format!("{} this month", format_bytes(s.month_total(now))),
                        p.accent,
                        widgets::KpiExtra::Spark(&volume),
                    ),
                    (
                        Icon::Calendar,
                        "Today",
                        format_bytes(s.today(now)),
                        "The graph shows the last 7 days".to_owned(),
                        p.accent2,
                        widgets::KpiExtra::Spark(&volume[volume.len().saturating_sub(7)..]),
                    ),
                    (
                        Icon::CheckCircle,
                        "Completed",
                        s.completed.to_string(),
                        match success {
                            Some(r) => format!("{:.1}% succeed \u{00b7} {} failed", r * 100.0, s.failed),
                            None => format!("{} failed", s.failed),
                        },
                        p.green,
                        widgets::KpiExtra::Meter(success.unwrap_or(0.0) as f32),
                    ),
                    (
                        Icon::Bolt,
                        "Peak speed",
                        format_speed(s.peak_speed),
                        format!(
                            "avg {} \u{00b7} {} active",
                            format_speed(s.average_speed()),
                            format_duration(s.active_secs as u64)
                        ),
                        p.yellow,
                        widgets::KpiExtra::None,
                    ),
                ];
                let per_row = if ui.available_width() > 860.0 { 4 } else { 2 };
                let tile_w = (ui.available_width() - 12.0 * (per_row - 1) as f32) / per_row as f32;
                let mut tiles = tiles.into_iter().peekable();
                while tiles.peek().is_some() {
                    ui.horizontal(|ui| {
                        for (icon, label, value, sub, color, extra) in tiles.by_ref().take(per_row) {
                            widgets::kpi_tile(ui, tile_w, icon, color, label, &value, &sub, extra, &p);
                        }
                    });
                    ui.add_space(12.0);
                }

                let wide = ui.available_width() > 900.0;
                let days_chart = |ui: &mut Ui| {
                    even_card(ui, &p, "stats-charts", wide, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_min_height(18.0);
                            widgets::caption(ui, "LAST 30 DAYS", &p);
                        });
                        widgets::day_bars(ui, &s.recent_days(30, now), &p, 160.0);
                    });
                };
                let history = self.snap.history.clone();
                let speed = self.snap.speed;
                let live_chart = |ui: &mut Ui| {
                    even_card(ui, &p, "stats-charts", wide, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_min_height(18.0);
                            widgets::caption(ui, "LIVE THROUGHPUT · 60 s", &p);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format_speed(speed)).family(crate::theme::semi()).color(p.accent2),
                                );
                            });
                        });
                        widgets::area_chart(ui, &history, vec2(ui.available_width(), 160.0), p.accent2, &p);
                    });
                };
                if wide {
                    ui.columns_const(|[a, b]: &mut [Ui; 2]| {
                        days_chart(a);
                        live_chart(b);
                    });
                } else {
                    days_chart(ui);
                    ui.add_space(12.0);
                    live_chart(ui);
                }
                ui.add_space(12.0);

                card(&p).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    widgets::caption(ui, "THE LAST 6 MONTHS", &p);
                    ui.add_space(4.0);
                    egui::ScrollArea::horizontal().id_salt("heatmap").show(ui, |ui| {
                        widgets::heatmap(ui, &s.recent_days(182, now), &p);
                    });
                });
                ui.add_space(12.0);

                let categories = self.category_rows();
                let hosts = self.host_rows();
                let breakdown = |ui: &mut Ui, title: &str, rows: &[(String, Option<Category>, usize, u64)]| {
                    even_card(ui, &p, "stats-breakdown", wide, |ui| {
                        widgets::caption(ui, title, &p);
                        ui.add_space(4.0);
                        if rows.is_empty() {
                            ui.label(RichText::new("Nothing yet").color(p.text_dim));
                        }
                        let max = rows.iter().map(|r| r.3).max().unwrap_or(1).max(1);
                        for (name, cat, n, bytes) in rows {
                            let color = cat.map_or(p.accent, |c| p.category(c));
                            widgets::split_row(
                                ui,
                                |ui| {
                                    if let Some(c) = cat {
                                        widgets::icon(ui, Icon::for_category(*c), 14.0, color);
                                    }
                                    ui.add(egui::Label::new(RichText::new(name).color(p.text)).truncate());
                                },
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("{n} · {}", format_bytes(*bytes)))
                                            .size(12.0)
                                            .color(p.text_dim),
                                    );
                                },
                            );
                            widgets::share_bar(ui, *bytes as f32 / max as f32, color, &p);
                            ui.add_space(3.0);
                        }
                    });
                };
                if wide {
                    ui.columns_const(|[a, b]: &mut [Ui; 2]| {
                        breakdown(a, "BY CATEGORY", &categories);
                        breakdown(b, "TOP SOURCES", &hosts);
                    });
                } else {
                    breakdown(ui, "BY CATEGORY", &categories);
                    ui.add_space(12.0);
                    breakdown(ui, "TOP SOURCES", &hosts);
                }
            });
        });
    }

    fn category_rows(&self) -> Vec<(String, Option<Category>, usize, u64)> {
        let mut rows: Vec<_> = self
            .cfg
            .categories()
            .into_iter()
            .map(|c| {
                let (n, bytes) = self
                    .snap
                    .downloads
                    .iter()
                    .filter(|d| d.category == c)
                    .fold((0, 0), |(n, b), d| (n + 1, b + d.downloaded));
                (self.cfg.category_label(c), Some(c), n, bytes)
            })
            .filter(|r| r.2 > 0)
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.3));
        rows
    }

    fn host_rows(&self) -> Vec<(String, Option<Category>, usize, u64)> {
        let mut map: HashMap<String, (usize, u64)> = HashMap::new();
        for d in self.snap.downloads.iter().filter(|d| d.status != Status::Failed) {
            let host = d.url.split("://").nth(1).and_then(|r| r.split(['/', '?', ':']).next()).unwrap_or("").to_owned();
            let e = map.entry(host).or_default();
            e.0 += 1;
            e.1 += d.downloaded;
        }
        let mut rows: Vec<_> = map.into_iter().map(|(h, (n, b))| (h, None, n, b)).collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.3));
        rows.truncate(6);
        rows
    }
}
