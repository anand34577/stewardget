//! The "why is it this fast?" card shown for running downloads.

use egui::{CornerRadius, RichText, Stroke, Ui};
use stewardget_core::{DownloadView, Fix, Insight, InsightContext, InsightTone, explain_speed, limit_notices};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

impl StewardGetApp {
    /// What explains the current speed of `d`, if there is anything to say.
    /// The reading stays for a few seconds before it gives way to another, so
    /// the card doesn't flicker while the speed hovers between two causes.
    pub fn insight_for(&self, d: &DownloadView) -> Option<Insight> {
        let now = self.insight_context(d);
        let fresh = explain_speed(d, &now);
        let mut memo = self.insight_memo.borrow_mut();
        let Some(fresh) = fresh else {
            memo.remove(&d.id);
            return None;
        };
        match memo.get(&d.id) {
            Some((since, held))
                if held.kind != fresh.kind
                    && held.tone != InsightTone::Warn
                    && since.elapsed() < std::time::Duration::from_secs(8) =>
            {
                Some(held.clone())
            }
            Some((since, held)) if held.kind == fresh.kind => {
                let since = *since;
                memo.insert(d.id, (since, fresh.clone()));
                Some(fresh)
            }
            _ => {
                memo.insert(d.id, (std::time::Instant::now(), fresh.clone()));
                Some(fresh)
            }
        }
    }

    /// The speed limits in force for `d`, each as its own card.
    pub fn limit_notices_for(&self, d: &DownloadView) -> Vec<Insight> {
        limit_notices(d, &self.insight_context(d))
    }

    fn insight_context(&self, d: &DownloadView) -> InsightContext {
        InsightContext {
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
            limits_separate: true,
        }
    }

    /// One-line cards: the speed limits in force, each on its own, then the
    /// explanation of the speed. Each has its fix, when there is one.
    pub fn insight_card(&mut self, ui: &mut Ui, d: &DownloadView) {
        let mut cards = self.limit_notices_for(d);
        cards.extend(self.insight_for(d));
        let mut apply = None;
        for (i, insight) in cards.iter().enumerate() {
            if i > 0 {
                ui.add_space(4.0);
            }
            apply = self.one_insight(ui, insight).or(apply);
        }
        if let Some(fix) = apply {
            self.apply_fix(fix, d);
        }
    }

    fn one_insight(&self, ui: &mut Ui, insight: &Insight) -> Option<Fix> {
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
            .corner_radius(CornerRadius::same(crate::theme::radius::LG))
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
        apply
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
