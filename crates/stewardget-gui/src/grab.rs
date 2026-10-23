//! The site grabber: reads a web page (and optionally the pages it links to),
//! then offers the files it found in the new-download list to pick from.

use std::sync::{Arc, Mutex};

use egui::RichText;
use stewardget_core::{GrabOptions, GrabResult, NewDownload};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

const LABEL_W: f32 = 120.0;

enum Phase {
    Idle,
    Running,
    Done(Result<GrabResult, String>),
}

pub struct GrabDialog {
    url: String,
    depth: u8,
    other_sites: bool,
    extensions: String,
    phase: Arc<Mutex<Phase>>,
    task: Option<tokio::task::JoinHandle<()>>,
    message: Option<String>,
    focus: bool,
}

impl GrabDialog {
    pub fn new(url: Option<String>) -> Self {
        Self {
            url: url.unwrap_or_default(),
            depth: 0,
            other_sites: false,
            extensions: String::new(),
            phase: Arc::new(Mutex::new(Phase::Idle)),
            task: None,
            message: None,
            focus: true,
        }
    }
}

impl Drop for GrabDialog {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl StewardGetApp {
    pub fn grab_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.grab_dialog.take() else { return };
        let p = self.palette;
        let mut close = false;
        let mut start = false;

        // A finished run: hand the files to the new-download list, or say why not.
        let finished = {
            let mut phase = d.phase.lock().unwrap();
            match &*phase {
                Phase::Done(_) => Some(std::mem::replace(&mut *phase, Phase::Idle)),
                _ => None,
            }
        };
        if let Some(Phase::Done(result)) = finished {
            d.task = None;
            match result {
                Ok(found) if !found.files.is_empty() => {
                    let batch: Vec<NewDownload> = found
                        .files
                        .iter()
                        .map(|(url, page)| {
                            let mut new = NewDownload::new(url.clone());
                            new.options.referrer = Some(page.clone());
                            new
                        })
                        .collect();
                    self.inbox.lock().unwrap().push_back(batch);
                    ctx.request_repaint();
                    if found.truncated {
                        let yellow = p.yellow;
                        self.toast("The search stopped at its limit; more files may exist".to_owned(), yellow);
                    }
                    return;
                }
                Ok(found) => {
                    let robots = if found.blocked_by_robots > 0 {
                        format!(
                            " {} page(s) were skipped because the site's robots.txt asks crawlers to stay out.",
                            found.blocked_by_robots
                        )
                    } else {
                        String::new()
                    };
                    d.message = Some(format!(
                        "No files found on {} page(s).{robots} Pages that build their links in the browser need the extension's \u{201c}Download all links\u{201d}.",
                        found.pages
                    ));
                }
                Err(e) => d.message = Some(capitalize_first(&e)),
            }
        }

        let running = matches!(*d.phase.lock().unwrap(), Phase::Running);
        let modal = crate::dialogs::modal("stewardget-grab", &p).show(ctx, |ui| {
            ui.set_width(520.0);
            crate::dialogs::title(ui, &p, Icon::Globe, "Find files on a web page");
            widgets::form_row(ui, LABEL_W, "Page address", &p, |ui| {
                let r = ui.add_enabled(
                    !running,
                    widgets::input(&mut d.url).hint_text("https://example.com/downloads/").desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
            });
            widgets::form_row(ui, LABEL_W, "Follow links", &p, |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    ui.add(egui::Slider::new(&mut d.depth, 0..=3).custom_formatter(|v, _| match v as u8 {
                        0 => "This page only".to_owned(),
                        1 => "1 level deeper".to_owned(),
                        n => format!("{n} levels deeper"),
                    }));
                });
            });
            widgets::form_row(ui, LABEL_W, "File types", &p, |ui| {
                ui.add_enabled(
                    !running,
                    widgets::input(&mut d.extensions).hint_text("all kinds, or e.g. zip, iso, pdf").desired_width(f32::INFINITY),
                );
            });
            if d.depth > 0 {
                widgets::form_row(ui, LABEL_W, "Other sites", &p, |ui| {
                    ui.add_enabled_ui(!running, |ui| {
                        widgets::toggle(ui, &mut d.other_sites, &p);
                    });
                    ui.label(RichText::new("also follow links to other websites").size(12.0).color(p.text_dim));
                });
            }
            ui.add_space(6.0);
            let note = if running {
                "Reading pages\u{2026} this can take a moment."
            } else {
                "Reads the page's HTML, one page at a time, and follows the site's robots.txt. At most 100 pages are read."
            };
            crate::dialogs::note(ui, note, p.text_dim);
            if let Some(m) = &d.message {
                ui.add_space(4.0);
                crate::dialogs::note(ui, m.as_str(), p.yellow);
            }
            crate::dialogs::footer(ui, |ui| {
                let can = !running && !d.url.trim().is_empty();
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Search), "Find files", can, &p).clicked()
                    || (can && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    start = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    close = true;
                }
            });
        });
        close |= modal.should_close();
        if close {
            return;
        }
        if start {
            let mut url = d.url.trim().to_owned();
            if !url.contains("://") {
                url = format!("https://{url}");
            }
            let options = GrabOptions {
                depth: d.depth,
                other_sites: d.other_sites,
                extensions: stewardget_core::CustomCategory::parse_extensions(&d.extensions),
            };
            d.message = None;
            *d.phase.lock().unwrap() = Phase::Running;
            let (engine, phase, ctx2) = (self.engine.clone(), Arc::clone(&d.phase), ctx.clone());
            d.task = Some(self.runtime.spawn(async move {
                let result = engine.grab_links(&url, &options).await.map_err(|e| e.to_string());
                *phase.lock().unwrap() = Phase::Done(result);
                ctx2.request_repaint();
            }));
        }
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        self.grab_dialog = Some(d);
    }
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().collect::<String>() + chars.as_str())
}
