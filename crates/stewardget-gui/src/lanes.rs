//! Where a download's bytes come from: network links and mirrors, drawn as
//! live bars, plus the same view for the whole app on the Overview.

use egui::{Align, Layout, RichText, Ui, vec2};
use stewardget_core::DownloadView;
use stewardget_core::util::{format_bytes, format_speed};

use crate::app::StewardGetApp;
use crate::widgets::{self, card};

impl StewardGetApp {
    /// One bar per lane (link × source) of a running download. Shown only
    /// when there is something to compare: several lanes, or the tuner at work.
    pub fn lanes_card(&mut self, ui: &mut Ui, d: &DownloadView) {
        let p = self.palette;
        let tuning = d.tuned_connections.filter(|&n| n < self.engine.config().connections);
        if d.lanes.len() < 2 && tuning.is_none() {
            return;
        }
        let switchable = d.status.is_active() && d.lanes.len() > 1;
        let mut switch: Option<(usize, bool)> = None;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::caption(ui, "Sources and links", &p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(n) = tuning {
                        ui.label(
                            RichText::new(format!("Tuning: {n} of {} connections", self.engine.config().connections))
                                .size(12.0)
                                .color(p.accent2),
                        );
                    }
                });
            });
            let top = d.lanes.iter().map(|l| l.speed).fold(1.0, f64::max);
            let total: u64 = d.lanes.iter().map(|l| l.bytes).sum::<u64>().max(1);
            let on_count = d.lanes.iter().filter(|l| !l.disabled && !l.failed).count();
            for (i, lane) in d.lanes.iter().enumerate() {
                let color = if lane.disabled {
                    p.text_dim
                } else if lane.failed {
                    p.red
                } else if lane.mirror {
                    p.accent
                } else {
                    p.accent2
                };
                // Name on one line, host and figures on the next: fits a narrow inspector.
                let name = if lane.mirror { format!("{} · {}", lane.link, lane.host) } else { lane.link.clone() };
                widgets::split_row(
                    ui,
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(name).color(p.text)).truncate());
                    },
                    |ui| {
                        if switchable {
                            let mut on = !lane.disabled;
                            // The last working lane can't be switched off.
                            let last = on && !lane.failed && on_count <= 1;
                            let tip = if last {
                                "At least one link has to stay on"
                            } else if on {
                                "Switch this link off for this download; its connections move to the others"
                            } else {
                                "Use this link again"
                            };
                            ui.add_enabled_ui(!last, |ui| {
                                if widgets::toggle(ui, &mut on, &p).on_hover_text(tip).changed() {
                                    switch = Some((i, on));
                                }
                            });
                        }
                        if lane.mirror {
                            widgets::pill(ui, "mirror", p.accent);
                        }
                    },
                );
                // The figures are placed first; the host gets what is left.
                widgets::split_row(
                    ui,
                    |ui| {
                        if !lane.mirror && !lane.host.is_empty() {
                            ui.add(egui::Label::new(RichText::new(&lane.host).size(12.0).color(p.text_dim)).truncate());
                        }
                    },
                    |ui| {
                        let text = if lane.disabled {
                            "off".to_owned()
                        } else if lane.failed {
                            "dropped".to_owned()
                        } else {
                            format!(
                                "{}  ·  {}%  ·  {}×",
                                format_speed(lane.speed),
                                lane.bytes * 100 / total,
                                lane.workers
                            )
                        };
                        ui.label(RichText::new(text).size(12.0).color(if lane.failed { p.red } else { p.text_dim }));
                    },
                );
                widgets::share_bar(ui, (lane.speed / top) as f32, color, &p);
                if let Some(why) = lane.reason.as_ref().filter(|_| lane.failed) {
                    ui.label(RichText::new(why).size(12.0).color(p.red));
                }
                ui.add_space(2.0);
            }
        });
        if let Some((lane, on)) = switch {
            let r = self.engine.set_lane_enabled(d.id, lane, on);
            self.report(r);
        }
    }

    /// Every network link in use, with its live speed. Only when more than
    /// one link is bonded.
    pub fn links_card(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let links = self.engine.links();
        if links.len() < 2 {
            return;
        }
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            widgets::caption(ui, "Network links", &p);
            ui.add_space(3.0);
            for (i, link) in links.iter().enumerate() {
                let color = if i % 2 == 0 { p.accent2 } else { p.accent };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&link.label).family(crate::theme::semi()).color(p.text));
                        ui.label(
                            RichText::new(format!(
                                "{}{}",
                                link.address.as_deref().unwrap_or(""),
                                if link.limit > 0 {
                                    format!("  ·  capped at {}", format_speed(link.limit as f64))
                                } else {
                                    String::new()
                                }
                            ))
                            .size(12.0)
                            .color(p.text_dim),
                        );
                        ui.label(
                            RichText::new(format!("{} so far", format_bytes(link.bytes))).size(12.0).color(p.text_dim),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let w = (ui.available_width() - 90.0).clamp(80.0, 260.0);
                        ui.label(
                            RichText::new(format_speed(link.speed))
                                .size(15.0)
                                .family(crate::theme::semi())
                                .color(color),
                        );
                        widgets::area_chart(ui, &link.history, vec2(w, 40.0), color, &p);
                    });
                });
                ui.add_space(3.0);
            }
        });
    }
}
