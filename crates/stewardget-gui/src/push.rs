//! Settings for messages to other devices: any number of destinations
//! (Gotify, ntfy, email), each with its own events and queues.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};
use stewardget_core::Config;
use stewardget_core::notifiers::{self, Kind, Message, Security, Service, Target};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::settings::{row, section};
use crate::theme::CONTROL_H;
use crate::widgets::{self, ButtonKind, toggle};

/// Results of "Send test", per destination id: `None` while it is on its way.
pub type TestResults = HashMap<u32, Arc<Mutex<Option<Result<(), String>>>>>;

const FIELD_W: f32 = 300.0;

impl StewardGetApp {
    /// The phone and email destinations of the Notifications tab.
    pub(crate) fn push_sections(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        section(ui, &p, "Phone and email notifications", |ui| {
            row(
                ui,
                &p,
                "Destinations",
                "Send messages to Gotify, ntfy or email when downloads finish or fail, a queue runs out or everything is done, also while the window is closed. Add one per person, device or team; each picks its own events and queues.",
                |_| {},
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for kind in Kind::ALL {
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Plus), kind.name(), true, &p)
                        .on_hover_text(format!("Add a {} destination", kind.name()))
                        .clicked()
                    {
                        let id = draft.remote_notify.next_id();
                        draft.remote_notify.targets.push(Target::new(id, kind.service()));
                    }
                }
            });
        });

        let queues: Vec<(u32, String)> = draft.queues.iter().map(|q| (q.id, q.name.clone())).collect();
        let mut remove = None;
        for (i, target) in draft.remote_notify.targets.iter_mut().enumerate() {
            let title = if target.name.trim().is_empty() {
                target.service.kind().name().to_owned()
            } else {
                format!("{} \u{00b7} {}", target.name.trim(), target.service.kind().name())
            };
            section(ui, &p, &title, |ui| {
                target_fields(ui, target, &queues, true, &p);
            });
            ui.add_space(-10.0);
            // Remove is placed first, so the test result gets what is left of the line.
            widgets::split_row(
                ui,
                |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    self.test_button(ui, target);
                },
                |ui| {
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::Trash), "Remove", true, &p).clicked() {
                        remove = Some(i);
                    }
                },
            );
            if let Some(Some(Err(e))) = self.notify_tests.get(&target.id).map(|s| s.lock().unwrap().clone()) {
                ui.add_space(2.0);
                ui.add(egui::Label::new(RichText::new(crate::app::capitalize(&e)).size(12.0).color(p.red)).wrap());
            }
            ui.add_space(18.0);
        }
        if let Some(i) = remove {
            let id = draft.remote_notify.targets.remove(i).id;
            self.notify_tests.remove(&id);
        }
    }

    /// "Send test" for one destination, with the result next to it. Uses the
    /// settings as they are on screen, before Save.
    fn test_button(&mut self, ui: &mut Ui, target: &Target) {
        let p = self.palette;
        let slot = self.notify_tests.get(&target.id).cloned();
        let state = slot.as_ref().map(|s| s.lock().unwrap().clone());
        let sending = matches!(state, Some(None));
        if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Bolt), "Send test", !sending, &p).clicked() {
            let slot = Arc::new(Mutex::new(None));
            let (out, target, ctx) = (Arc::clone(&slot), target.clone(), ui.ctx().clone());
            let id = target.id;
            let message = Message {
                title: "StewardGet test".to_owned(),
                body: format!("Messages to \u{201c}{}\u{201d} work.", target.name.trim()),
                problem: false,
            };
            self.runtime.spawn(async move {
                let result = notifiers::send(&target, &message).await;
                *out.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
            self.notify_tests.insert(id, slot);
        }
        match state {
            Some(None) => {
                ui.add(egui::Spinner::new().size(14.0).color(p.accent2));
                ui.label(RichText::new("Sending…").size(12.5).color(p.text_dim));
            }
            Some(Some(Ok(()))) => {
                widgets::icon(ui, Icon::CheckCircle, 14.0, p.green);
                ui.label(RichText::new("Sent. Check that it arrived.").size(12.5).color(p.green));
            }
            Some(Some(Err(_))) => {
                // The reason is shown in full under the buttons.
                widgets::icon(ui, Icon::Alert, 14.0, p.red);
                ui.label(RichText::new("Couldn't send").size(12.5).color(p.red));
            }
            None => {}
        }
    }
}

/// Everything about one destination: name, switch, service settings, events
/// and queues. A queue's own window passes `full: false` and only edits who
/// and where; it picks the events and whether the queue counts itself.
pub(crate) fn target_fields(
    ui: &mut Ui,
    t: &mut Target,
    queues: &[(u32, String)],
    full: bool,
    p: &crate::theme::Palette,
) {
    if full {
        row(ui, p, "Send here", "Switch off to keep the settings without sending anything.", |ui| {
            toggle(ui, &mut t.enabled, p);
        });
    }
    row(ui, p, "Name", "Who or what this is for, e.g. My phone or Team mailbox.", |ui| {
        field(ui, &mut t.name, "name", false);
    });
    match &mut t.service {
        Service::Gotify(g) => {
            row(ui, p, "Server", "The address you open Gotify at, for example https://gotify.example.com.", |ui| {
                field(ui, &mut g.url, "https://", false);
            });
            row(ui, p, "Application token", "Create an application in Gotify and paste its token here.", |ui| {
                field(ui, &mut g.token, "token", true);
            });
            row(ui, p, "Priority", "Most Gotify apps make a sound from 5 up. Failures are sent 2 higher.", |ui| {
                widgets::slider(ui, &mut g.priority, 0..=10, "", None, p);
            });
        }
        Service::Ntfy(n) => {
            row(ui, p, "Server", "Leave as https://ntfy.sh unless you run your own.", |ui| {
                field(ui, &mut n.server, "https://ntfy.sh", false);
            });
            row(
                ui,
                p,
                "Topic",
                "Subscribe to it in the ntfy app. Anyone who knows a public topic's name can read it, so pick one that is hard to guess.",
                |ui| {
                    field(ui, &mut n.topic, "stewardget-…", false);
                },
            );
            row(ui, p, "Access token", "Only for protected topics. Empty otherwise.", |ui| {
                field(ui, &mut n.token, "optional", true);
            });
        }
        Service::Email(e) => {
            row(ui, p, "Mail server", "For example smtp.gmail.com or smtp.office365.com.", |ui| {
                field(ui, &mut e.host, "smtp.example.com", false);
            });
            row(ui, p, "Security", "STARTTLS usually uses port 587, SSL/TLS port 465.", |ui| {
                let before = e.security;
                widgets::segmented(
                    ui,
                    &mut e.security,
                    &[(Security::StartTls, "STARTTLS"), (Security::Tls, "SSL/TLS"), (Security::None, "None")],
                    p,
                );
                // Follow the usual port while it is still one of the defaults.
                if e.security != before && matches!(e.port, 25 | 465 | 587) {
                    e.port = match e.security {
                        Security::StartTls => 587,
                        Security::Tls => 465,
                        Security::None => 25,
                    };
                }
            });
            let unencrypted = e.security == Security::None;
            row(
                ui,
                p,
                "Port",
                if unencrypted {
                    "Without encryption the password and messages travel in plain text: only for a mail server on your own network."
                } else {
                    ""
                },
                |ui| {
                    ui.add_sized([90.0, CONTROL_H], egui::DragValue::new(&mut e.port).range(1..=65535));
                },
            );
            row(ui, p, "Username", "Usually your email address. Empty if the server needs no login.", |ui| {
                field(ui, &mut e.username, "you@example.com", false);
            });
            row(
                ui,
                p,
                "Password",
                "With Gmail and Outlook, use an app password. Stored on this computer only and left out of backups.",
                |ui| {
                    field(ui, &mut e.password, "password", true);
                },
            );
            row(ui, p, "From", "The sender address; the username is used when empty.", |ui| {
                field(ui, &mut e.from, "optional", false);
            });
            row(ui, p, "To", "One or more addresses, separated by commas.", |ui| {
                field(ui, &mut e.to, "you@example.com, team@example.com", false);
            });
        }
    }
    if !full {
        return;
    }
    // Choices go on their own wrapping line under the label: they don't fit beside it.
    row(ui, p, "Tell about", "Failures count once automatic retries are used up.", |_| {});
    chips(ui, |ui| {
        let ev = &mut t.events;
        for (label, on) in [
            ("A download finished", &mut ev.finished),
            ("A download failed", &mut ev.failed),
            ("A queue finished", &mut ev.queue_finished),
            ("Everything finished", &mut ev.all_finished),
        ] {
            if widgets::chip(ui, label, *on, p).clicked() {
                *on = !*on;
            }
        }
    });
    if queues.len() > 1 {
        let covered = if t.queues.is_empty() {
            "Every queue".to_owned()
        } else {
            queues
                .iter()
                .filter(|(id, _)| t.queues.contains(id))
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        row(
            ui,
            p,
            "For queues",
            "Each queue says for itself whether this destination hears about it: Queues & schedule, then Edit on a queue.",
            |ui| {
                let font = egui::FontId::proportional(12.5);
                let text = widgets::fit_text(ui.painter(), &covered, &font, FIELD_W);
                ui.label(RichText::new(text).size(12.5).color(p.text_dim)).on_hover_text(covered);
            },
        );
    }
}

/// A text field of the standard width; `secret` hides what is typed.
fn field(ui: &mut Ui, text: &mut String, hint: &str, secret: bool) {
    ui.add_sized([FIELD_W, CONTROL_H], widgets::input(text).hint_text(hint).password(secret));
}

/// A wrapping line of chips under a settings row.
fn chips(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        add(ui);
    });
    ui.add_space(8.0);
}
