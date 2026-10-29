//! The "why is it this fast?" card shown for running downloads.

use egui::{CornerRadius, RichText, Stroke, Ui};
use stewardget_core::{DownloadView, Fix, Insight, InsightContext, InsightTone, explain_speed};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

impl StewardGetApp {
    /// What explains the current speed of `d`, if there is anything to say.
    pub fn insight_for(&self, d: &DownloadView) -> Option<Insight> {
        let cx = InsightContext {
            global_limit: self.snap.speed_limit,
            scheduled_limit: self.snap.scheduled_limit,
            queue_limit: self.cfg.queue(d.queue).speed_limit,
            aggregate_speed: self.snap.speed,
            active_downloads: self
                .snap
                .downloads
                .iter()
                .filter(|x| x.status == stewardget_core::Status::Downloading)
                .count(),
            configured_connections: self.cfg.connections,
            host_average: self.history.host_average(&stewardget_core::host_of(&d.url)),
        };
        explain_speed(d, &cx)
    }

    /// A one-line card with the explanation and, when there is one, its fix.
    pub fn insight_card(&mut self, ui: &mut Ui, d: &DownloadView) {
        let Some(insight) = self.insight_for(d) else { return };
        let p = self.palette;
        let color = match insight.tone {
            InsightTone::Good => p.green,
            InsightTone::Info => p.accent2,
            InsightTone::Warn => p.yellow,
        };
        let icon = match insight.tone {
            InsightTone::Good => Icon::Bolt,
            InsightTone::Info => Icon::Gauge,
            InsightTone::Warn => Icon::Warning,
        };
        let mut apply = None;
        egui::Frame::new()
            .fill(color.gamma_multiply(if p.dark { 0.10 } else { 0.08 }))
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    widgets::icon(ui, icon, 16.0, color);
                    ui.label(RichText::new(&insight.headline).family(crate::theme::semi()).color(p.text));
                    ui.label(RichText::new(&insight.detail).size(12.5).color(p.text_dim));
                    if let Some(fix) = insight.fix
                        && widgets::button(ui, ButtonKind::Ghost, None, fix.label(), true, &p).clicked()
                    {
                        apply = Some(fix);
                    }
                });
            });
        if let Some(fix) = apply {
            self.apply_fix(fix, d);
        }
    }

    fn apply_fix(&mut self, fix: Fix, d: &DownloadView) {
        match fix {
            Fix::RemoveDownloadLimit => {
                let r = self.engine.set_download_speed_limit(d.id, 0);
                self.report(r);
            }
            Fix::RemoveGlobalLimit => self.engine.set_speed_limit(0),
            Fix::OpenProperties => self.open_properties(d),
            Fix::Reconnect => {
                let r = self.engine.pause(d.id).and_then(|()| self.engine.resume(d.id));
                self.report(r);
            }
        }
    }
}
