//! Statistics page.

use std::collections::HashMap;

use egui::{Align, Layout, Margin, RichText, Ui, vec2};
use stewardget_core::util::{format_bytes, format_duration, format_speed, format_timestamp, now_unix};
use stewardget_core::{Category, Status};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, card};

impl StewardGetApp {
    pub fn stats_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let s = self.snap.stats.clone();
        let now = now_unix();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Statistics").size(18.0).family(crate::theme::semi()).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Stored only on this device — never sent anywhere.")
                                .size(12.0)
                                .color(p.text_dim),
                        );
                        widgets::icon(ui, Icon::Lock, 13.0, p.green);
                    });
                });
                ui.add_space(10.0);

                let tiles = [
                    ("Total downloaded", format_bytes(s.total_bytes), String::new(), p.accent),
                    ("Today", format_bytes(s.today(now)), String::new(), p.accent2),
                    ("Completed", s.completed.to_string(), format!("{} failed", s.failed), p.green),
                    (
                        "Success rate",
                        s.success_rate().map_or("—".into(), |r| format!("{:.1}%", r * 100.0)),
                        String::new(),
                        p.green,
                    ),
                    ("Peak speed", format_speed(s.peak_speed), String::new(), p.accent2),
                    ("Average speed", format_speed(s.average_speed()), "while downloading".into(), p.accent),
                    ("Time downloading", format_duration(s.active_secs as u64), String::new(), p.yellow),
                    (
                        "Since",
                        if s.first_seen > 0 { format_timestamp(s.first_seen)[..10].to_owned() } else { "—".into() },
                        String::new(),
                        p.text,
                    ),
                ];
                let per_row = ((ui.available_width() / 210.0).floor() as usize).clamp(2, 8);
                for chunk in tiles.chunks(per_row) {
                    ui.columns(per_row, |cols| {
                        for (col, (label, value, caption, color)) in cols.iter_mut().zip(chunk) {
                            widgets::stat_tile(col, &p, label, value, caption, *color);
                        }
                    });
                    ui.add_space(8.0);
                }

                let wide = ui.available_width() > 900.0;
                let days_chart = |ui: &mut Ui| {
                    card(&p).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        widgets::caption(ui, "LAST 30 DAYS", &p);
                        widgets::day_bars(ui, &s.recent_days(30, now), &p, 170.0);
                    });
                };
                let history = self.snap.history.clone();
                let speed = self.snap.speed;
                let live_chart = |ui: &mut Ui| {
                    card(&p).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::caption(ui, "LIVE THROUGHPUT · 60 s", &p);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format_speed(speed)).family(crate::theme::semi()).color(p.accent2),
                                );
                            });
                        });
                        widgets::area_chart(ui, &history, vec2(ui.available_width(), 156.0), p.accent2, &p);
                    });
                };
                if wide {
                    ui.columns_const(|[a, b]: &mut [Ui; 2]| {
                        days_chart(a);
                        live_chart(b);
                    });
                } else {
                    days_chart(ui);
                    ui.add_space(8.0);
                    live_chart(ui);
                }
                ui.add_space(8.0);

                let categories = self.category_rows();
                let hosts = self.host_rows();
                let breakdown = |ui: &mut Ui, title: &str, rows: &[(String, Option<Category>, usize, u64)]| {
                    card(&p).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        widgets::caption(ui, title, &p);
                        ui.add_space(4.0);
                        if rows.is_empty() {
                            ui.label(RichText::new("Nothing yet").color(p.text_dim));
                        }
                        let max = rows.iter().map(|r| r.3).max().unwrap_or(1).max(1);
                        for (name, cat, n, bytes) in rows {
                            let color = cat.map_or(p.accent, |c| p.category(c));
                            ui.horizontal(|ui| {
                                if let Some(c) = cat {
                                    widgets::icon(ui, Icon::for_category(*c), 14.0, color);
                                }
                                ui.add(egui::Label::new(RichText::new(name).color(p.text)).truncate());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(format!("{n} · {}", format_bytes(*bytes)))
                                            .size(12.0)
                                            .color(p.text_dim),
                                    );
                                });
                            });
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
                    ui.add_space(8.0);
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
