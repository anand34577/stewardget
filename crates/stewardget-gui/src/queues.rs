//! Queue management: the queue editor, deleting a queue, and what the close
//! button does.

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};
use stewardget_core::{CloseAction, DownloadId, MAIN_QUEUE, Queue, QueueId, Schedule};

use crate::app::{ListFilter, StewardGetApp, capitalize, hhmm};
use crate::icons::Icon;
use crate::theme::{self, Palette};
use crate::widgets::{self, ButtonKind};

const LABEL_W: f32 = 132.0;
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Creates or edits a queue.
pub struct QueueDialog {
    /// `None` for a new queue.
    id: Option<QueueId>,
    name: String,
    max_concurrent: usize,
    speed_limit: u64,
    schedule: Schedule,
    /// Downloads to move into the new queue once it exists.
    move_ids: Vec<DownloadId>,
    error: Option<String>,
    focus: bool,
}

impl QueueDialog {
    pub fn edit(queue: &Queue) -> Self {
        Self {
            id: Some(queue.id),
            name: queue.name.clone(),
            max_concurrent: queue.max_concurrent,
            speed_limit: queue.speed_limit,
            schedule: queue.schedule.clone(),
            move_ids: Vec::new(),
            error: None,
            focus: false,
        }
    }

    pub fn create(move_ids: Vec<DownloadId>) -> Self {
        Self {
            id: None,
            name: String::new(),
            max_concurrent: 0,
            speed_limit: 0,
            schedule: Schedule::default(),
            move_ids,
            error: None,
            focus: true,
        }
    }
}

/// One line describing a queue's limit and schedule, e.g. "2 at a time · 02:00–07:00, Mon–Fri".
pub fn summary(q: &Queue) -> String {
    let mut limit = match q.max_concurrent {
        0 => "Global limit".to_owned(),
        1 => "1 at a time".to_owned(),
        n => format!("{n} at a time"),
    };
    if q.speed_limit > 0 {
        limit.push_str(&format!(" · {}", stewardget_core::util::format_speed(q.speed_limit as f64)));
    }
    if !q.schedule.enabled {
        return format!("{limit} · no schedule");
    }
    let stop = q.schedule.stop_minute.map_or(String::new(), |m| format!("–{}", hhmm(m)));
    format!("{limit} · {}{stop}, {}", hhmm(q.schedule.start_minute), days_text(&q.schedule.days))
}

fn days_text(days: &[bool; 7]) -> String {
    match days {
        [true, true, true, true, true, true, true] => "every day".into(),
        [true, true, true, true, true, false, false] => "Mon–Fri".into(),
        [false, false, false, false, false, true, true] => "weekends".into(),
        _ => {
            let on: Vec<&str> = DAYS.iter().zip(days).filter(|(_, on)| **on).map(|(d, _)| *d).collect();
            if on.is_empty() { "no days".into() } else { on.join(", ") }
        }
    }
}

impl StewardGetApp {
    pub fn open_queue_dialog(&mut self, id: Option<QueueId>) {
        let config = self.engine.config();
        self.queue_dialog = Some(match id.and_then(|id| config.queues.iter().find(|q| q.id == id)) {
            Some(q) => QueueDialog::edit(q),
            None => QueueDialog::create(Vec::new()),
        });
    }

    /// Writes queue changes and keeps an open settings draft in step.
    fn save_queues(&mut self, edit: impl FnOnce(&mut Vec<Queue>)) -> Result<(), String> {
        let mut config = self.engine.config();
        edit(&mut config.queues);
        self.engine.update_config(config).map_err(|e| capitalize(&e.to_string()))?;
        if let Some(draft) = &mut self.settings_draft {
            draft.queues = self.engine.config().queues;
        }
        Ok(())
    }

    pub fn queue_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.queue_dialog.take() else { return };
        let p = self.palette;
        let mut close = false;
        let mut save = false;
        let mut delete = false;
        let modal = crate::dialogs::modal("stewardget-queue", &p).show(ctx, |ui| {
            ui.set_width(520.0);
            let title = if d.id.is_some() { "Edit queue" } else { "New queue" };
            crate::dialogs::title(ui, &p, Icon::Queue, title);
            widgets::form_row(ui, LABEL_W, "Name", &p, |ui| {
                let r = ui.add(
                    widgets::input(&mut d.name).hint_text("e.g. Night, Videos, Work").desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
            });
            widgets::form_row(ui, LABEL_W, "At the same time", &p, |ui| {
                ui.add(
                    egui::Slider::new(&mut d.max_concurrent, 0..=16)
                        .clamping(egui::SliderClamping::Edits)
                        .custom_formatter(|v, _| if v == 0.0 { "Global limit".into() } else { format!("{v}") }),
                );
            });
            ui.horizontal(|ui| {
                ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
                let global = self.engine.config().max_concurrent;
                crate::dialogs::note(
                    ui,
                    format!("How many of this queue's downloads run at once, within the global limit of {global}."),
                    p.text_dim,
                );
            });
            widgets::form_row(ui, LABEL_W, "Speed limit", &p, |ui| {
                widgets::speed_input(ui, "queue", &mut d.speed_limit, 150.0);
                ui.label(RichText::new("shared by the queue, e.g. 2M").size(12.0).color(p.text_dim));
            });
            ui.add_space(6.0);
            widgets::caption(ui, "SCHEDULE", &p);
            ui.add_space(4.0);
            schedule_editor(ui, &p, "queue", &mut d.schedule);
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                crate::dialogs::note(ui, e.as_str(), p.red);
            }
            crate::dialogs::footer(ui, |ui| {
                let label = if d.id.is_some() { "Save" } else { "Create queue" };
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Check), label, true, &p).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    save = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    close = true;
                }
                if d.id.is_some_and(|id| id != MAIN_QUEUE)
                    && widgets::button(ui, ButtonKind::Tool, Some(Icon::Trash), "Delete queue", true, &p).clicked()
                {
                    delete = true;
                }
            });
        });
        close |= modal.should_close();
        if delete && let Some(id) = d.id {
            self.delete_queue = Some(id);
            return;
        }
        if save {
            let name = d.name.trim().to_owned();
            let config = self.engine.config();
            if name.is_empty() {
                d.error = Some("Give the queue a name.".into());
            } else if config.queues.iter().any(|q| Some(q.id) != d.id && q.name.eq_ignore_ascii_case(&name)) {
                d.error = Some(format!("There is already a queue called \u{201c}{name}\u{201d}."));
            } else {
                let id = d.id.unwrap_or_else(|| config.next_queue_id());
                let queue = Queue {
                    id,
                    name: name.clone(),
                    max_concurrent: d.max_concurrent,
                    speed_limit: d.speed_limit,
                    schedule: d.schedule.clone(),
                };
                match self.save_queues(|queues| match queues.iter_mut().find(|q| q.id == id) {
                    Some(q) => *q = queue,
                    None => queues.push(queue),
                }) {
                    Ok(()) => {
                        if !d.move_ids.is_empty() {
                            let r = self.engine.set_queue(&d.move_ids, id);
                            self.report(r);
                        }
                        if d.id.is_none() {
                            self.filter = ListFilter::Queue(id);
                            self.page = crate::app::Page::Downloads;
                        }
                        let green = p.green;
                        self.toast(
                            if d.id.is_some() { format!("Saved {name}") } else { format!("Created {name}") },
                            green,
                        );
                        close = true;
                    }
                    Err(e) => d.error = Some(e),
                }
            }
        }
        if !close {
            self.queue_dialog = Some(d);
        }
    }

    /// Confirms deleting a queue; its downloads join the main queue.
    pub fn delete_queue_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.delete_queue else { return };
        let p = self.palette;
        let config = self.engine.config();
        let Some(queue) = config.queues.iter().find(|q| q.id == id && q.id != MAIN_QUEUE).cloned() else {
            self.delete_queue = None;
            return;
        };
        let count = self.snap.downloads.iter().filter(|d| d.queue == id).count();
        let mut done = false;
        let modal = crate::dialogs::modal("stewardget-delete-queue", &p).show(ctx, |ui| {
            ui.set_width(420.0);
            crate::dialogs::title(ui, &p, Icon::Trash, &format!("Delete \u{201c}{}\u{201d}?", queue.name));
            let main = config.queue(MAIN_QUEUE).name.clone();
            let text = match count {
                0 => "The queue is empty.".to_owned(),
                1 => format!("Its download moves to {main}. Nothing is deleted from disk."),
                n => format!("Its {n} downloads move to {main}. Nothing is deleted from disk."),
            };
            crate::dialogs::note(ui, text, p.text_dim);
            crate::dialogs::footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Danger, Some(Icon::Trash), "Delete queue", true, &p).clicked() {
                    match self.save_queues(|queues| queues.retain(|q| q.id != id)) {
                        Ok(()) => {
                            if self.filter == ListFilter::Queue(id) {
                                self.filter = ListFilter::Queue(MAIN_QUEUE);
                            }
                            let green = p.green;
                            self.toast(format!("Deleted {}", queue.name), green);
                        }
                        Err(e) => {
                            let red = p.red;
                            self.toast(e, red);
                        }
                    }
                    done = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    done = true;
                }
            });
        });
        if done || modal.should_close() {
            self.delete_queue = None;
        }
    }

    /// Context menu of a queue in the sidebar.
    pub fn queue_menu(&mut self, ui: &mut Ui, id: QueueId) {
        let p = self.palette;
        ui.set_min_width(220.0);
        let in_queue: Vec<_> = self.snap.downloads.iter().filter(|d| d.queue == id).collect();
        let can_start = in_queue.iter().any(|d| d.status.can_resume() && !d.awaiting_link);
        let can_stop = in_queue.iter().any(|d| d.status.can_pause());
        if crate::app::menu_item(ui, &p, Icon::Play, "Start queue", can_start) {
            self.engine.start_queue(id);
            ui.close();
        }
        if crate::app::menu_item(ui, &p, Icon::Pause, "Stop queue", can_stop) {
            self.engine.stop_queue(id);
            ui.close();
        }
        ui.separator();
        if crate::app::menu_item(ui, &p, Icon::Edit, "Edit queue\u{2026}", true) {
            self.open_queue_dialog(Some(id));
            ui.close();
        }
        if crate::app::menu_item(ui, &p, Icon::Plus, "New queue\u{2026}", true) {
            self.open_queue_dialog(None);
            ui.close();
        }
        if id != MAIN_QUEUE && crate::app::menu_item(ui, &p, Icon::Trash, "Delete queue\u{2026}", true) {
            self.delete_queue = Some(id);
            ui.close();
        }
    }

    /// Asks what the close button should do, the first time it is used.
    pub fn close_prompt(&mut self, ctx: &egui::Context) {
        let Some(remember) = self.close_prompt.as_mut() else { return };
        let p = self.palette;
        let mut choice = None;
        let modal = crate::dialogs::modal("stewardget-close", &p).show(ctx, |ui| {
            ui.set_width(470.0);
            crate::dialogs::title(ui, &p, Icon::Power, "Keep StewardGet running?");
            crate::dialogs::note(
                ui,
                "StewardGet can keep running in the background, minimised. Downloads continue, and your browser keeps handing downloads to it. Quitting stops both until you open StewardGet again.",
                p.text,
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                widgets::toggle(ui, remember, &p);
                ui.label(RichText::new("Remember my choice (change it in Settings › General)").size(12.5).color(p.text_dim));
            });
            crate::dialogs::footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Download), "Keep running", true, &p).clicked() {
                    choice = Some(CloseAction::Background);
                }
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Power), "Quit StewardGet", true, &p).clicked() {
                    choice = Some(CloseAction::Quit);
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    choice = Some(CloseAction::Ask);
                }
            });
        });
        if modal.should_close() {
            choice = Some(CloseAction::Ask);
        }
        let Some(choice) = choice else { return };
        let remember = *remember;
        self.close_prompt = None;
        if remember && choice != CloseAction::Ask {
            let mut config = self.engine.config();
            config.close_action = choice;
            let r = self.engine.update_config(config);
            self.report(r);
            if let Some(draft) = &mut self.settings_draft {
                draft.close_action = choice;
            }
        }
        match choice {
            CloseAction::Background => self.park(ctx),
            CloseAction::Quit => self.quit(ctx),
            CloseAction::Ask => {}
        }
    }
}

/// Enable switch, start and stop times and weekdays of a schedule.
pub fn schedule_editor(ui: &mut Ui, p: &Palette, id: &str, s: &mut Schedule) {
    ui.push_id(id, |ui| {
        widgets::form_row(ui, LABEL_W, "Run on a schedule", p, |ui| {
            widgets::toggle(ui, &mut s.enabled, p);
        });
        ui.add_enabled_ui(s.enabled, |ui| {
            widgets::form_row(ui, LABEL_W, "Start at", p, |ui| {
                crate::settings::time_picker(ui, "start", &mut s.start_minute);
                ui.label(RichText::new("paused downloads of the queue start").size(12.0).color(p.text_dim));
            });
            widgets::form_row(ui, LABEL_W, "Pause at", p, |ui| {
                let mut on = s.stop_minute.is_some();
                widgets::toggle(ui, &mut on, p);
                let mut minute = s.stop_minute.unwrap_or(7 * 60);
                ui.add_enabled_ui(on, |ui| crate::settings::time_picker(ui, "stop", &mut minute));
                s.stop_minute = on.then_some(minute);
            });
            widgets::form_row(ui, LABEL_W, "Days", p, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, day) in DAYS.iter().enumerate() {
                    if crate::settings::day_chip(ui, day, s.days[i], p).clicked() {
                        s.days[i] = !s.days[i];
                    }
                }
            });
        });
    });
}

/// A card per queue for the settings page.
pub fn queue_card(ui: &mut Ui, p: &Palette, q: &Queue, count: usize) -> QueueCardAction {
    let mut action = QueueCardAction::None;
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                crate::widgets::icon(ui, Icon::Queue, 18.0, p.accent);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&q.name).family(theme::semi()).color(p.text));
                        let n = if count == 1 { "1 download".to_owned() } else { format!("{count} downloads") };
                        ui.label(RichText::new(n).size(12.0).color(p.text_dim));
                    });
                    ui.label(RichText::new(summary(q)).size(12.0).color(p.text_dim));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if q.id != MAIN_QUEUE
                        && widgets::button(ui, ButtonKind::Tool, Some(Icon::Trash), "", true, p)
                            .on_hover_text("Delete queue")
                            .clicked()
                    {
                        action = QueueCardAction::Delete;
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Edit), "Edit", true, p).clicked() {
                        action = QueueCardAction::Edit;
                    }
                });
            });
        });
    ui.add_space(6.0);
    action
}

pub enum QueueCardAction {
    None,
    Edit,
    Delete,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        let mut q = Queue::default();
        assert_eq!(summary(&q), "Global limit · no schedule");
        q.max_concurrent = 2;
        q.schedule = Schedule { enabled: true, start_minute: 120, stop_minute: Some(420), days: [true; 7] };
        assert_eq!(summary(&q), "2 at a time · 02:00–07:00, every day");
        q.schedule.days = [false, true, false, true, false, false, false];
        assert_eq!(days_text(&q.schedule.days), "Tue, Thu");
        q.speed_limit = 2 << 20;
        q.schedule.enabled = false;
        assert_eq!(summary(&q), "2 at a time · 2.00 MiB/s · no schedule");
    }
}
