//! Modal dialogs: add downloads, properties, confirmations, link refresh,
//! completion notice and the "when all done" countdown.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};
use stewardget_core::util::{expand_pattern, format_bytes, parse_size};

use crate::table::free_space;
use stewardget_core::{
    Checksum, Config, DownloadId, DownloadOptions, DownloadView, ExistingFile, LinkInfo, MAIN_QUEUE, NewDownload, QueueId, Status,
};

use crate::app::{AfterAll, ListFilter, Page, StewardGetApp, capitalize};
use crate::icons::Icon;
use crate::theme::{self, Palette};
use crate::widgets::{self, ButtonKind};

/// Most links one dialog will add (batch patterns can explode).
const MAX_LINKS: usize = 5000;
const LABEL_W: f32 = 118.0;

fn modal_frame(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(18))
        .shadow(p.shadow(14, 44))
}

pub(crate) fn modal(id: &str, p: &Palette) -> egui::Modal {
    egui::Modal::new(egui::Id::unique(id)).frame(modal_frame(p)).backdrop_color(p.backdrop())
}

pub(crate) fn title(ui: &mut Ui, p: &Palette, icon: Icon, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(9), p.accent.gamma_multiply(if p.dark { 0.2 } else { 0.12 }));
        crate::icons::paint(ui.painter(), rect.shrink(9.0), icon, p.accent);
        ui.add_space(4.0);
        ui.label(RichText::new(text).size(17.0).family(theme::semi()).color(p.text));
    });
    ui.add_space(4.0);
}

/// Right-aligned button row at the bottom of a dialog.
pub(crate) fn footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.with_layout(Layout::right_to_left(Align::Center), add);
}

pub(crate) fn note(ui: &mut Ui, text: impl Into<String>, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(12.5).color(color)).wrap());
}

/// A tinted inline message box.
fn callout(ui: &mut Ui, p: &Palette, color: Color32, icon: Icon, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(color.gamma_multiply(if p.dark { 0.12 } else { 0.08 }))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                widgets::icon(ui, icon, 16.0, color);
                ui.vertical(add);
            });
        });
}

// ---- Add ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Duplicates {
    Skip,
    AddAgain,
}

type CheckResult = Arc<Mutex<Option<Result<LinkInfo, String>>>>;

struct LinkCheck {
    url: String,
    since: Instant,
    started: bool,
    result: CheckResult,
}

pub struct AddDialog {
    pub urls: String,
    dir: String,
    default_dir: String,
    name: String,
    connections: usize,
    default_connections: usize,
    limit: String,
    checksum: String,
    referrer: String,
    headers: String,
    mirrors: String,
    advanced: bool,
    duplicates: Duplicates,
    /// What to do when the file is already there; `None` = the setting's choice.
    existing: Option<ExistingFile>,
    pub queue: QueueId,
    check: Option<LinkCheck>,
    error: Option<String>,
    focus_requested: bool,
    /// `links()` for the text it was computed from; patterns can expand to
    /// thousands of links, too many to redo every frame.
    links_cache: Option<(String, Vec<String>, bool)>,
}

impl AddDialog {
    pub fn new(config: &Config) -> Self {
        let dir = config.download_dir.display().to_string();
        Self {
            urls: String::new(),
            default_dir: dir.clone(),
            dir,
            name: String::new(),
            connections: config.connections,
            default_connections: config.connections,
            limit: String::new(),
            checksum: String::new(),
            referrer: String::new(),
            headers: String::new(),
            mirrors: String::new(),
            advanced: false,
            duplicates: Duplicates::Skip,
            existing: None,
            queue: MAIN_QUEUE,
            check: None,
            error: None,
            focus_requested: false,
            links_cache: None,
        }
    }

    /// Every link in the text box, with batch patterns expanded.
    fn links(&mut self) -> (Vec<String>, bool) {
        if let Some((text, links, expanded)) = &self.links_cache
            && *text == self.urls
        {
            return (links.clone(), *expanded);
        }
        let (links, expanded) = self.expand_links();
        self.links_cache = Some((self.urls.clone(), links.clone(), expanded));
        (links, expanded)
    }

    fn expand_links(&self) -> (Vec<String>, bool) {
        let mut out = Vec::new();
        let mut expanded = false;
        for token in stewardget_core::util::split_links(&self.urls) {
            let list = expand_pattern(&token, MAX_LINKS - out.len().min(MAX_LINKS));
            expanded |= list.len() > 1;
            out.extend(list);
            if out.len() >= MAX_LINKS {
                break;
            }
        }
        (out, expanded)
    }

    fn headers(&self) -> Result<Vec<(String, String)>, String> {
        let mut headers = Vec::new();
        for line in self.headers.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (k, v) =
                line.split_once(':').ok_or_else(|| format!("Header “{line}” must look like “Name: value”."))?;
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
        Ok(headers)
    }

    fn options(&self) -> Result<DownloadOptions, String> {
        let speed_limit = match self.limit.trim() {
            "" => 0,
            s => parse_size(s).ok_or("Speed limit must look like 500k or 2M.")?,
        };
        let checksum = match self.checksum.trim() {
            "" => None,
            s => Some(s.parse::<Checksum>().map_err(|e| capitalize(&e.to_string()))?),
        };
        let dir = self.dir.trim();
        Ok(DownloadOptions {
            file_name: None,
            directory: (dir != self.default_dir).then(|| PathBuf::from(dir)),
            connections: (self.connections != self.default_connections).then_some(self.connections),
            speed_limit,
            headers: self.headers()?,
            checksum,
            referrer: Some(self.referrer.trim().to_owned()).filter(|r| !r.is_empty()),
            user_agent: None,
            fallback_url: None,
            mirrors: stewardget_core::util::split_links(&self.mirrors),
            mux: None,
            existing: self.existing,
        })
    }

    fn build(&self, links: &[String], paused: bool) -> Result<Vec<NewDownload>, String> {
        if links.is_empty() {
            return Err("Enter at least one link.".into());
        }
        let options = self.options()?;
        let single = links.len() == 1;
        if !single && !options.mirrors.is_empty() {
            return Err("Mirrors belong to one file. Add these links one at a time, or clear the mirrors.".into());
        }
        if !single && options.checksum.is_some() {
            return Err("A checksum belongs to one file. Add these links one at a time, or clear the checksum.".into());
        }
        Ok(links
            .iter()
            .map(|url| NewDownload {
                url: url.clone(),
                paused,
                queue: Some(self.queue),
                options: DownloadOptions {
                    file_name: Some(self.name.trim().to_owned()).filter(|n| single && !n.is_empty()),
                    ..options.clone()
                },
            })
            .collect())
    }

    fn checked(&self) -> Option<Result<LinkInfo, String>> {
        self.check.as_ref().and_then(|c| c.result.lock().unwrap().clone())
    }
}

impl StewardGetApp {
    /// Probes a single link in the background (debounced) so the dialog can
    /// show the real file name, size and resume support before adding.
    fn run_link_check(&mut self, dialog: &mut AddDialog, link: Option<&String>) {
        let Some(url) = link else {
            dialog.check = None;
            return;
        };
        if dialog.check.as_ref().is_none_or(|c| &c.url != url) {
            dialog.check =
                Some(LinkCheck { url: url.clone(), since: Instant::now(), started: false, result: Arc::default() });
        }
        let check = dialog.check.as_mut().unwrap();
        if !check.started && check.since.elapsed() > Duration::from_millis(450) {
            check.started = true;
            let (engine, url, slot) = (self.engine.clone(), url.clone(), Arc::clone(&check.result));
            let options = DownloadOptions {
                headers: dialog.headers().unwrap_or_default(),
                referrer: Some(dialog.referrer.trim().to_owned()).filter(|r| !r.is_empty()),
                ..Default::default()
            };
            self.runtime.spawn(async move {
                let result = engine.check_link(&url, &options).await.map_err(|e| capitalize(&e.to_string()));
                *slot.lock().unwrap() = Some(result);
            });
        }
    }

    pub fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.add.take() else { return };
        let p = self.palette;
        let (links, expanded) = dialog.links();
        // Magnet links are for a torrent client; StewardGet hands them over.
        let (magnets, links): (Vec<String>, Vec<String>) = links.into_iter().partition(|l| l.starts_with("magnet:"));
        self.run_link_check(&mut dialog, (links.len() == 1).then(|| &links[0]));
        let wanted: std::collections::HashSet<&str> = links.iter().map(String::as_str).collect();
        let existing: Vec<DownloadView> =
            self.snap.downloads.iter().filter(|d| wanted.contains(d.url.as_str())).cloned().collect();
        let mut submit: Option<bool> = None; // Some(paused)
        let mut close = false;
        let mut show_existing = None;
        let width = (ctx.content_rect().width() - 80.0).clamp(380.0, 540.0);

        // Something finished earlier that looks like this: same link, or the
        // same file name and size once the server has been asked.
        self.history.refresh(&self.engine);
        let earlier = if existing.is_empty() && links.len() == 1 {
            let (name, size) = match dialog.checked() {
                Some(Ok(info)) => (Some(info.file_name), info.size),
                _ => (None, None),
            };
            self.history
                .entries()
                .iter()
                .find(|e| {
                    e.status == stewardget_core::Status::Completed
                        && (e.url == links[0]
                            || (size.is_some() && e.size == size && name.as_deref() == Some(e.file_name.as_str())))
                })
                .cloned()
        } else {
            None
        };

        // A file with this name is already on disk: the list has it finished,
        // the history remembers it, or the folder holds one by that name.
        let conflict: Option<PathBuf> = if links.len() == 1 {
            let by_name = match dialog.checked() {
                Some(Ok(info)) => Some(PathBuf::from(dialog.dir.trim()).join(info.file_name)),
                _ => None,
            };
            existing
                .iter()
                .find(|d| d.status == Status::Completed && d.path.exists())
                .map(|d| d.path.clone())
                .or_else(|| earlier.as_ref().filter(|e| e.path.exists()).map(|e| e.path.clone()))
                .or_else(|| by_name.filter(|p| p.exists()))
        } else {
            None
        };
        if conflict.is_none() {
            dialog.existing = None;
        }
        let mut open_existing: Option<PathBuf> = None;
        let default_existing = self.engine.config().existing_file;

        let modal = modal("stewardget-add", &p).show(ctx, |ui| {
            ui.set_width(width);
            title(ui, &p, Icon::Download, "Add downloads");
            let urls = ui.add(
                widgets::input_multiline(&mut dialog.urls)
                    .hint_text("Paste one or more links, one per line.\nTip: img[001-120].jpg adds a numbered series.")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            if !dialog.focus_requested {
                urls.request_focus();
                dialog.focus_requested = true;
            }
            let n = links.len();
            if n > 1 || expanded {
                ui.add_space(2.0);
                let mut text = format!("{n} links");
                if expanded {
                    text.push_str(" (batch pattern expanded)");
                }
                if n >= MAX_LINKS {
                    text.push_str(&format!(" — limited to {MAX_LINKS}"));
                }
                ui.label(RichText::new(text).size(12.0).color(p.text_dim));
            }

            if !magnets.is_empty() {
                ui.add_space(5.0);
                callout(ui, &p, p.accent2, Icon::Info, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{} magnet link{}: torrents are downloaded by a BitTorrent client, which StewardGet does not include. Your default torrent app can take {}.",
                                magnets.len(),
                                if magnets.len() == 1 { "" } else { "s" },
                                if magnets.len() == 1 { "it" } else { "them" }
                            ))
                            .color(p.text),
                        )
                        .wrap(),
                    );
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::External), "Open in torrent app", true, &p)
                        .clicked()
                    {
                        for m in &magnets {
                            let _ = stewardget_core::util::open_url(m);
                        }
                    }
                });
            }

            // Link info, as reported by the server.
            if n == 1 {
                ui.add_space(5.0);
                self.link_card(ui, &dialog);
            }

            // Duplicates.
            if !existing.is_empty() && conflict.is_none() {
                ui.add_space(5.0);
                callout(ui, &p, p.yellow, Icon::Info, |ui| {
                    let text = if existing.len() == 1 {
                        format!("Already in your list: {} ({})", existing[0].file_name, existing[0].status.label())
                    } else {
                        format!("{} of these links are already in your list.", existing.len())
                    };
                    ui.add(egui::Label::new(RichText::new(text).color(p.text)).wrap());
                    ui.horizontal(|ui| {
                        widgets::segmented(
                            ui,
                            &mut dialog.duplicates,
                            &[(Duplicates::Skip, "Skip them"), (Duplicates::AddAgain, "Download again")],
                            &p,
                        );
                        if existing.len() == 1
                            && widgets::button(ui, ButtonKind::Tool, Some(Icon::Eye), "Show it", true, &p).clicked()
                        {
                            show_existing = Some(existing[0].id);
                        }
                    });
                });
            }

            if let Some(path) = &conflict {
                ui.add_space(5.0);
                callout(ui, &p, p.yellow, Icon::Warning, |ui| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let folder = path.parent().map(|d| d.display().to_string()).unwrap_or_default();
                    ui.add(
                        egui::Label::new(RichText::new(format!("“{name}” is already downloaded")).family(theme::semi()).color(p.text))
                            .truncate(),
                    );
                    ui.add(egui::Label::new(RichText::new(folder).size(12.0).color(p.text_dim)).truncate());
                    let mut choice = dialog.existing.unwrap_or(default_existing);
                    ui.horizontal(|ui| {
                        if widgets::button(ui, ButtonKind::Tool, Some(Icon::FolderOpen), "Open existing", true, &p)
                            .on_hover_text("Show the file in its folder and cancel this download")
                            .clicked()
                        {
                            open_existing = Some(path.clone());
                        }
                        widgets::segmented(
                            ui,
                            &mut choice,
                            &[(ExistingFile::Rename, "Save with another name"), (ExistingFile::Overwrite, "Replace it")],
                            &p,
                        );
                    });
                    dialog.existing = Some(choice);
                    let hint = match choice {
                        ExistingFile::Rename => "Both files are kept; the new one gets a number in its name.".to_owned(),
                        ExistingFile::Overwrite => "The old file is replaced when the download finishes.".to_owned(),
                    };
                    ui.label(RichText::new(hint).size(12.0).color(p.text_dim));
                });
            } else if let Some(e) = &earlier {
                ui.add_space(5.0);
                callout(ui, &p, p.accent2, Icon::Info, |ui| {
                    let when = stewardget_core::util::format_relative(e.finished_at, stewardget_core::util::now_unix());
                    let text = format!("You downloaded “{}” {when}, but the file has been moved or deleted.", e.file_name);
                    ui.add(egui::Label::new(RichText::new(text).color(p.text)).wrap());
                });
            }

            ui.add_space(4.0);
            let detected = match dialog.checked() {
                Some(Ok(info)) => info.file_name,
                _ => String::new(),
            };
            let config = self.engine.config();
            widgets::form_row(ui, LABEL_W, "Save to", &p, |ui| {
                widgets::folder_field(ui, &mut dialog.dir, &config.recent_dirs, &p);
            });
            let size = match dialog.checked() {
                Some(Ok(info)) => info.size,
                _ => None,
            };
            if let Some(free) = free_space(std::path::Path::new(dialog.dir.trim())) {
                ui.horizontal(|ui| {
                    ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
                    let short = size.is_some_and(|s| s > free);
                    let text = if short {
                        format!("Not enough space: {} free, {} needed", format_bytes(free), format_bytes(size.unwrap()))
                    } else {
                        format!("{} free on this drive", format_bytes(free))
                    };
                    widgets::icon(ui, Icon::Drive, 13.0, if short { p.red } else { p.text_dim });
                    ui.label(RichText::new(text).size(12.0).color(if short { p.red } else { p.text_dim }));
                });
            }
            widgets::form_row(ui, LABEL_W, "File name", &p, |ui| {
                let hint = match n {
                    0 | 1 if !detected.is_empty() => detected.clone(),
                    0 | 1 => "detected automatically".to_owned(),
                    _ => "automatic for multiple links".to_owned(),
                };
                ui.add_enabled(n <= 1, widgets::input(&mut dialog.name).hint_text(hint).desired_width(f32::INFINITY));
            });
            widgets::form_row(ui, LABEL_W, "Queue", &p, |ui| {
                egui::ComboBox::from_id_salt("add-queue")
                    .width(220.0)
                    .selected_text(config.queue(dialog.queue).name.clone())
                    .show_ui(ui, |ui| {
                        for q in &config.queues {
                            ui.selectable_value(&mut dialog.queue, q.id, &q.name);
                        }
                    });
            });

            if dialog.advanced {
                ui.add_space(4.0);
                widgets::form_row(ui, LABEL_W, "Connections", &p, |ui| {
                    ui.add(egui::Slider::new(&mut dialog.connections, 1..=32));
                });
                widgets::form_row(ui, LABEL_W, "Speed limit", &p, |ui| {
                    ui.add(widgets::input(&mut dialog.limit).hint_text("unlimited, or e.g. 2M").desired_width(170.0));
                });
                widgets::form_row(ui, LABEL_W, "Checksum", &p, |ui| {
                    ui.add(
                        widgets::input(&mut dialog.checksum)
                            .hint_text("sha256:… (verified after download)")
                            .desired_width(f32::INFINITY),
                    );
                });
                widgets::form_row(ui, LABEL_W, "Referrer", &p, |ui| {
                    ui.add(
                        widgets::input(&mut dialog.referrer)
                            .hint_text("page the link came from (optional)")
                            .desired_width(f32::INFINITY),
                    );
                });
                widgets::form_row(ui, LABEL_W, "Mirrors", &p, |ui| {
                    ui.add(
                        widgets::input_multiline(&mut dialog.mirrors)
                            .hint_text(
                                "Other links to the same file, one per line.
Connections are shared across all of them.",
                            )
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                });
                widgets::form_row(ui, LABEL_W, "Headers", &p, |ui| {
                    ui.add(
                        widgets::input_multiline(&mut dialog.headers)
                            .hint_text("Cookie: session=…\nAuthorization: Bearer …")
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                });
            }
            if let Some(err) = &dialog.error {
                ui.add_space(6.0);
                note(ui, err.as_str(), p.red);
            }

            let to_add = match dialog.duplicates {
                _ if conflict.is_some() => n,
                Duplicates::Skip => links.iter().filter(|l| !existing.iter().any(|d| &d.url == *l)).count(),
                Duplicates::AddAgain => n,
            };
            footer(ui, |ui| {
                let label = if to_add > 1 { format!("Download {to_add} files") } else { "Download".to_owned() };
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Download), &label, to_add > 0, &p)
                    .on_hover_text("Ctrl+Enter")
                    .clicked()
                {
                    submit = Some(false);
                }
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Clock), "Download later", to_add > 0, &p)
                    .on_hover_text(
                        "Adds them to the queue paused. They start when you start the queue, or at its scheduled time.",
                    )
                    .clicked()
                {
                    submit = Some(true);
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    close = true;
                }
                let arrow = if dialog.advanced { Icon::CaretUp } else { Icon::CaretDown };
                if widgets::button(ui, ButtonKind::Tool, Some(arrow), "Advanced", true, &p).clicked() {
                    dialog.advanced = !dialog.advanced;
                }
            });
        });

        if let Some(path) = open_existing {
            let _ = stewardget_core::util::reveal_in_file_manager(&path);
            return;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            submit = Some(false);
        }
        close |= modal.should_close();
        if let Some(id) = show_existing {
            self.page = Page::Downloads;
            self.filter = ListFilter::All;
            self.selection = [id].into();
            return;
        }
        if let Some(paused) = submit {
            let wanted: Vec<String> = match dialog.duplicates {
                _ if conflict.is_some() => links.clone(),
                Duplicates::Skip => links.iter().filter(|l| !existing.iter().any(|d| &d.url == *l)).cloned().collect(),
                Duplicates::AddAgain => links.clone(),
            };
            match dialog.build(&wanted, paused) {
                Ok(requests) => {
                    let mut ids = Vec::new();
                    for req in requests {
                        match self.engine.add(req) {
                            Ok(id) => ids.push(id),
                            Err(e) => {
                                dialog.error = Some(capitalize(&e.to_string()));
                                break;
                            }
                        }
                    }
                    if dialog.error.is_none() {
                        let dir = PathBuf::from(dialog.dir.trim());
                        if dialog.dir.trim() != dialog.default_dir {
                            let mut config = self.engine.config();
                            config.remember_dir(dir);
                            let r = self.engine.update_config(config);
                            self.report(r);
                        }
                        let accent = p.accent2;
                        let verb = if paused { "Added" } else { "Started" };
                        self.toast(
                            format!("{verb} {} download{}", ids.len(), if ids.len() == 1 { "" } else { "s" }),
                            accent,
                        );
                        self.page = Page::Downloads;
                        self.filter = ListFilter::All;
                        self.selection = ids.into_iter().collect();
                        return;
                    }
                }
                Err(e) => dialog.error = Some(e),
            }
        }
        if !close {
            self.add = Some(dialog);
        }
    }

    fn link_card(&self, ui: &mut Ui, dialog: &AddDialog) {
        let p = self.palette;
        Frame::new()
            .fill(p.card_hover)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match dialog.checked() {
                    None => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(15.0).color(p.accent2));
                            ui.label(RichText::new("Checking the link…").color(p.text_dim));
                        });
                        ui.ctx().request_repaint_after(Duration::from_millis(150));
                    }
                    Some(Ok(info)) => {
                        let category = self.cfg.categorize(&info.file_name);
                        ui.horizontal(|ui| {
                            widgets::icon(ui, Icon::for_category(category), 22.0, p.category(category));
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&info.file_name).family(theme::semi()).color(p.text),
                                    )
                                    .truncate(),
                                );
                                ui.horizontal(|ui| {
                                    let size = info.size.map_or("Unknown size".to_owned(), format_bytes);
                                    ui.label(RichText::new(size).size(12.5).color(p.text_dim));
                                    if info.resumable {
                                        widgets::pill(ui, "Resumable · multi-connection", p.green);
                                    } else {
                                        widgets::pill(ui, "No resume support", p.yellow)
                                            .on_hover_text("The server doesn't support byte ranges: one connection, and pausing restarts from zero.");
                                    }
                                });
                            });
                        });
                    }
                    Some(Err(e)) => {
                        ui.horizontal(|ui| {
                            widgets::icon(ui, Icon::Warning, 16.0, p.yellow);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("Couldn't check the link: {e}. You can still add it."))
                                        .size(12.5)
                                        .color(p.text_dim),
                                )
                                .wrap(),
                            );
                        });
                    }
                }
            });
    }
}

// ---- Remove -------------------------------------------------------------------------

pub struct ConfirmRemove {
    items: Vec<(DownloadId, String, Status, u64)>,
    delete_files: bool,
}

impl ConfirmRemove {
    pub fn new(views: &[DownloadView], delete_files: bool) -> Self {
        Self {
            items: views.iter().map(|d| (d.id, d.file_name.clone(), d.status, d.downloaded)).collect(),
            delete_files,
        }
    }
}

impl StewardGetApp {
    pub fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.confirm.take() else { return };
        let p = self.palette;
        let mut done = false;
        let modal = modal("stewardget-remove", &p).show(ctx, |ui| {
            ui.set_width(440.0);
            let n = c.items.len();
            title(
                ui,
                &p,
                Icon::Trash,
                &if n == 1 { "Remove download?".to_owned() } else { format!("Remove {n} downloads?") },
            );
            for (_, name, _, _) in c.items.iter().take(5) {
                ui.add(egui::Label::new(RichText::new(name).color(p.text)).truncate());
            }
            if n > 5 {
                ui.label(RichText::new(format!("…and {} more", n - 5)).color(p.text_dim));
            }
            ui.add_space(6.0);
            let partial: u64 = c.items.iter().filter(|i| i.2 != Status::Completed).map(|i| i.3).sum();
            let has_done = c.items.iter().any(|i| i.2 == Status::Completed);
            ui.horizontal(|ui| {
                widgets::toggle(ui, &mut c.delete_files, &p);
                let what = match (has_done, partial > 0) {
                    (true, true) => "Also delete files and partial data from disk",
                    (true, false) => "Also delete the downloaded files from disk",
                    _ => "Also delete the partial data from disk",
                };
                ui.label(RichText::new(what).color(p.text));
            });
            if partial > 0 && !c.delete_files {
                ui.add_space(4.0);
                note(
                    ui,
                    format!(
                        "{} of partial data stays on disk. Add the same link again later to continue it.",
                        format_bytes(partial)
                    ),
                    p.text_dim,
                );
            }
            footer(ui, |ui| {
                let label = if c.delete_files { "Delete" } else { "Remove" };
                if widgets::button(ui, ButtonKind::Danger, Some(Icon::Trash), label, true, &p).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    for (id, _, _, _) in &c.items {
                        let r = self.engine.remove(*id, c.delete_files);
                        self.report(r);
                        self.selection.remove(id);
                    }
                    done = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    done = true;
                }
            });
        });
        if !done && !modal.should_close() {
            self.confirm = Some(c);
        }
    }
}

// ---- Restart ------------------------------------------------------------------------

pub struct ConfirmRestart {
    items: Vec<(DownloadId, String)>,
    discarded: u64,
}

impl ConfirmRestart {
    pub fn new(views: &[DownloadView]) -> Self {
        Self {
            items: views.iter().map(|d| (d.id, d.file_name.clone())).collect(),
            discarded: views.iter().filter(|d| d.status != Status::Completed).map(|d| d.downloaded).sum(),
        }
    }
}

impl StewardGetApp {
    pub fn restart_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = self.restart.take() else { return };
        let p = self.palette;
        let mut done = false;
        let modal = modal("stewardget-restart", &p).show(ctx, |ui| {
            ui.set_width(440.0);
            let n = c.items.len();
            title(ui, &p, Icon::Retry, &if n == 1 { "Start over?".to_owned() } else { format!("Start {n} downloads over?") });
            for (_, name) in c.items.iter().take(5) {
                ui.add(egui::Label::new(RichText::new(name).color(p.text)).truncate());
            }
            ui.add_space(5.0);
            let text = if c.discarded > 0 {
                format!(
                    "{} already downloaded will be discarded and everything is fetched again. To keep it, use Resume or Refresh link instead.",
                    format_bytes(c.discarded)
                )
            } else {
                "Finished files stay where they are; a new copy is downloaded.".to_owned()
            };
            note(ui, text, p.text_dim);
            footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Danger, Some(Icon::Retry), "Start over", true, &p).clicked() {
                    for (id, _) in &c.items {
                        let r = self.engine.restart(*id);
                        self.report(r);
                    }
                    done = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    done = true;
                }
            });
        });
        if !done && !modal.should_close() {
            self.restart = Some(c);
        }
    }
}

// ---- Properties ---------------------------------------------------------------------

pub struct PropertiesDialog {
    id: DownloadId,
    name: String,
    dir: String,
    url: String,
    connections: usize,
    default_connections: usize,
    limit: String,
    checksum: String,
    referrer: String,
    user_agent: String,
    headers: String,
    mirrors: String,
    error: Option<String>,
}

impl PropertiesDialog {
    pub fn new(d: &DownloadView, config: &Config) -> Self {
        let o = &d.options;
        Self {
            id: d.id,
            name: d.file_name.clone(),
            dir: d.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            url: d.url.clone(),
            connections: o.connections.unwrap_or(config.connections),
            default_connections: config.connections,
            limit: if o.speed_limit == 0 { String::new() } else { format_bytes(o.speed_limit) },
            checksum: o.checksum.as_ref().map(ToString::to_string).unwrap_or_default(),
            referrer: o.referrer.clone().unwrap_or_default(),
            user_agent: o.user_agent.clone().unwrap_or_default(),
            headers: o.headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect::<String>().trim_end().to_owned(),
            mirrors: o.mirrors.join(
                "
",
            ),
            error: None,
        }
    }

    fn options(&self, current: &DownloadOptions) -> Result<DownloadOptions, String> {
        let speed_limit = match self.limit.trim() {
            "" => 0,
            s => parse_size(s).ok_or("Speed limit must look like 500k or 2M.")?,
        };
        let checksum = match self.checksum.trim() {
            "" => None,
            s => Some(s.parse::<Checksum>().map_err(|e| capitalize(&e.to_string()))?),
        };
        let mut headers = Vec::new();
        for line in self.headers.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (k, v) =
                line.split_once(':').ok_or_else(|| format!("Header “{line}” must look like “Name: value”."))?;
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
        Ok(DownloadOptions {
            connections: (self.connections != self.default_connections || current.connections.is_some())
                .then_some(self.connections),
            speed_limit,
            headers,
            checksum,
            referrer: Some(self.referrer.trim().to_owned()).filter(|r| !r.is_empty()),
            user_agent: Some(self.user_agent.trim().to_owned()).filter(|r| !r.is_empty()),
            mirrors: stewardget_core::util::split_links(&self.mirrors),
            ..current.clone()
        })
    }
}

impl StewardGetApp {
    pub fn properties_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.properties.take() else { return };
        let Some(view) = self.snap.downloads.iter().find(|v| v.id == d.id).cloned() else { return };
        let p = self.palette;
        let mut close = false;
        let mut save = false;
        let running = view.status.is_active();
        let width = (ctx.content_rect().width() - 80.0).clamp(380.0, 620.0);
        let modal = modal("stewardget-properties", &p).show(ctx, |ui| {
            ui.set_width(width);
            title(ui, &p, Icon::Sliders, "Download properties");
            ui.horizontal(|ui| {
                let (label, color) = crate::table::status_label(&view, &p);
                widgets::pill(ui, &label, color);
                let size = match view.total {
                    Some(t) => format!("{} of {}", format_bytes(view.downloaded), format_bytes(t)),
                    None => format_bytes(view.downloaded),
                };
                ui.label(RichText::new(size).size(12.5).color(p.text_dim));
            });
            ui.add_space(6.0);
            widgets::caption(ui, "FILE", &p);
            ui.add_space(4.0);
            ui.add_enabled_ui(!running, |ui| {
                widgets::form_row(ui, LABEL_W, "File name", &p, |ui| {
                    ui.add(widgets::input(&mut d.name).desired_width(f32::INFINITY));
                });
                widgets::form_row(ui, LABEL_W, "Folder", &p, |ui| {
                    let (_, browse) = widgets::input_with_button(ui, &mut d.dir, "", Icon::FolderOpen, "Browse", &p);
                    if browse && let Some(folder) = rfd::FileDialog::new().set_directory(&d.dir).pick_folder() {
                        d.dir = folder.display().to_string();
                    }
                });
            });
            if running {
                note(ui, "Pause the download to rename or move it. Files on disk are moved with it.", p.text_dim);
            }
            ui.add_space(6.0);
            widgets::caption(ui, "CONNECTION", &p);
            ui.add_space(4.0);
            ui.add_enabled_ui(view.status != Status::Completed, |ui| {
                widgets::form_row(ui, LABEL_W, "Link", &p, |ui| {
                    ui.add(widgets::input(&mut d.url).desired_width(f32::INFINITY));
                });
            });
            widgets::form_row(ui, LABEL_W, "Connections", &p, |ui| {
                ui.add(egui::Slider::new(&mut d.connections, 1..=32));
            });
            widgets::form_row(ui, LABEL_W, "Speed limit", &p, |ui| {
                ui.add(widgets::input(&mut d.limit).hint_text("unlimited").desired_width(170.0));
            });
            widgets::form_row(ui, LABEL_W, "Referrer", &p, |ui| {
                ui.add(widgets::input(&mut d.referrer).hint_text("none").desired_width(f32::INFINITY));
            });
            widgets::form_row(ui, LABEL_W, "User agent", &p, |ui| {
                ui.add(widgets::input(&mut d.user_agent).hint_text("default").desired_width(f32::INFINITY));
            });
            widgets::form_row(ui, LABEL_W, "Mirrors", &p, |ui| {
                ui.add(
                    widgets::input_multiline(&mut d.mirrors)
                        .hint_text("Other links to the same file, one per line")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
            });
            widgets::form_row(ui, LABEL_W, "Headers", &p, |ui| {
                ui.add(
                    widgets::input_multiline(&mut d.headers)
                        .hint_text("Name: value, one per line")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
            });
            widgets::form_row(ui, LABEL_W, "Checksum", &p, |ui| {
                ui.add(widgets::input(&mut d.checksum).hint_text("sha256:…").desired_width(f32::INFINITY));
            });
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                note(ui, e.as_str(), p.red);
            }
            footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Check), "Save", true, &p).clicked() {
                    save = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    close = true;
                }
            });
        });
        close |= modal.should_close();
        if save {
            match self.apply_properties(&d, &view) {
                Ok(()) => {
                    let green = p.green;
                    self.toast("Properties saved", green);
                    close = true;
                }
                Err(e) => d.error = Some(e),
            }
        }
        if !close {
            self.properties = Some(d);
        }
    }

    fn apply_properties(&mut self, d: &PropertiesDialog, view: &DownloadView) -> Result<(), String> {
        let err = |e: stewardget_core::Error| capitalize(&e.to_string());
        let options = d.options(&view.options)?;
        let dir = PathBuf::from(d.dir.trim());
        let moved = d.name.trim() != view.file_name || Some(dir.as_path()) != view.path.parent();
        if moved {
            if d.name.trim().is_empty() {
                return Err("The file name can't be empty.".into());
            }
            self.engine.relocate(d.id, dir, d.name.trim()).map_err(err)?;
        }
        self.engine.update_options(d.id, options).map_err(err)?;
        if d.url.trim() != view.url && view.status != Status::Completed {
            self.engine.set_url(d.id, d.url.trim()).map_err(err)?;
        }
        Ok(())
    }
}

// ---- Refresh link ---------------------------------------------------------------------

pub struct RefreshDialog {
    id: DownloadId,
    name: String,
    progress: String,
    page: Option<String>,
    url: String,
    new_url: String,
    waiting_since: Option<Instant>,
    error: Option<String>,
}

impl RefreshDialog {
    pub fn new(d: &DownloadView) -> Self {
        let progress = match d.total {
            Some(t) if t > 0 => format!(
                "{:.1}% ({} of {}) is already downloaded and will be kept.",
                d.downloaded as f64 / t as f64 * 100.0,
                format_bytes(d.downloaded),
                format_bytes(t)
            ),
            _ => format!("{} is already downloaded and will be kept.", format_bytes(d.downloaded)),
        };
        Self {
            id: d.id,
            name: d.file_name.clone(),
            progress,
            page: d.options.referrer.clone(),
            url: d.url.clone(),
            new_url: String::new(),
            waiting_since: d.awaiting_link.then(Instant::now),
            error: None,
        }
    }
}

impl StewardGetApp {
    pub fn refresh_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.refresh.take() else { return };
        let p = self.palette;
        let Some(current) = self.snap.downloads.iter().find(|d| d.id == r.id).cloned() else { return };

        // The browser delivered a fresh link: the engine already resumed it.
        if r.waiting_since.is_some() && !current.awaiting_link {
            let green = p.green;
            self.toast(format!("Link refreshed — resuming {}", r.name), green);
            return;
        }

        let mut close = false;
        let width = (ctx.content_rect().width() - 80.0).clamp(380.0, 580.0);
        let modal = modal("stewardget-refresh", &p).show(ctx, |ui| {
            ui.set_width(width);
            title(ui, &p, Icon::Link, "Refresh download link");
            ui.add(egui::Label::new(RichText::new(&r.name).family(theme::semi()).color(p.text)).truncate());
            ui.label(RichText::new(&r.progress).size(12.5).color(p.green));
            ui.add_space(14.0);

            widgets::caption(ui, "OPTION 1 · RECAPTURE FROM THE BROWSER", &p);
            ui.add_space(2.0);
            note(
                ui,
                "StewardGet opens the download page. Start the download there again (log in first if needed) and StewardGet catches the new link automatically and continues.",
                p.text_dim,
            );
            ui.add_space(6.0);
            match r.waiting_since {
                None => {
                    let target = r.page.clone().unwrap_or_else(|| r.url.clone());
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Globe), "Open page & wait for link", true, &p)
                        .clicked()
                    {
                        let res = self.engine.await_new_link(r.id, true);
                        self.report(res);
                        let _ = stewardget_core::util::open_url(&target);
                        r.waiting_since = Some(Instant::now());
                    }
                    if r.page.is_none() {
                        ui.label(
                            RichText::new("No source page is known, so the original link is opened instead.")
                                .size(11.5)
                                .color(p.text_dim),
                        );
                    }
                }
                Some(since) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(15.0).color(p.accent2));
                        ui.label(
                            RichText::new(format!("Waiting for the browser… {}s", since.elapsed().as_secs()))
                                .color(p.accent2),
                        );
                    });
                    if matches!(self.snap.api, stewardget_core::ApiState::Disabled | stewardget_core::ApiState::Failed(_)) {
                        note(
                            ui,
                            "Browser integration is off — enable it in Settings › Browser integration, or paste the link below.",
                            p.yellow,
                        );
                    }
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
            }

            ui.add_space(8.0);
            widgets::caption(ui, "OPTION 2 · PASTE A NEW LINK", &p);
            ui.add_space(4.0);
            let (field, clicked) =
                widgets::input_with_button(ui, &mut r.new_url, "https://…", Icon::Check, "Use link", &p);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (clicked || enter) && !r.new_url.trim().is_empty() {
                match self.engine.set_url(r.id, r.new_url.trim()) {
                    Ok(()) => {
                        let green = p.green;
                        self.toast(format!("Link updated — resuming {}", r.name), green);
                        close = true;
                    }
                    Err(e) => r.error = Some(capitalize(&e.to_string())),
                }
            }
            if let Some(e) = &r.error {
                note(ui, e.as_str(), p.red);
            }
            footer(ui, |ui| {
                if r.waiting_since.is_some() {
                    if widgets::button(ui, ButtonKind::Ghost, None, "Hide", true, &p)
                        .on_hover_text("StewardGet keeps waiting for the link in the background")
                        .clicked()
                    {
                        close = true;
                    }
                    if widgets::button(ui, ButtonKind::Tool, None, "Stop waiting", true, &p).clicked() {
                        let res = self.engine.await_new_link(r.id, false);
                        self.report(res);
                        close = true;
                    }
                } else if widgets::button(ui, ButtonKind::Ghost, None, "Close", true, &p).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if !close {
            self.refresh = Some(r);
        }
    }
}

// ---- Completion & power ---------------------------------------------------------------

impl StewardGetApp {
    /// Countdown before quitting, sleeping or shutting down.
    pub fn power_dialog(&mut self, ctx: &egui::Context) {
        const SECS: u64 = 30;
        let Some(since) = self.power_countdown else { return };
        let left = SECS.saturating_sub(since.elapsed().as_secs());
        if left == 0 {
            self.run_after_all(ctx);
            return;
        }
        let p = self.palette;
        let action = self.after_all;
        let mut cancel = false;
        let mut now = false;
        let verb = match action {
            AfterAll::Quit => "StewardGet will close",
            AfterAll::Sleep => "The computer will go to sleep",
            AfterAll::Shutdown => "The computer will shut down",
            AfterAll::Nothing => "",
        };
        modal("stewardget-power", &p).show(ctx, |ui| {
            ui.set_width(420.0);
            title(ui, &p, Icon::Power, "All downloads are finished");
            ui.label(RichText::new(format!("{verb} in {left} seconds.")).size(14.0).color(p.text));
            ui.add_space(5.0);
            let frac = since.elapsed().as_secs_f32() / SECS as f32;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(3), p.track);
            let fill = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac.min(1.0), rect.height()));
            ui.painter().rect_filled(fill, CornerRadius::same(3), p.accent);
            footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Primary, None, action.label(), true, &p).clicked() {
                    now = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    cancel = true;
                }
            });
        });
        ctx.request_repaint_after(Duration::from_millis(250));
        if now {
            self.run_after_all(ctx);
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.after_all = AfterAll::Nothing;
            self.power_countdown = None;
        }
    }
}

// ---- First run ----------------------------------------------------------------------

/// First-run setup: download folder, start with the computer, browser.
pub struct Welcome {
    download_dir: String,
    autostart: bool,
    error: Option<String>,
}

impl Welcome {
    pub fn new(config: &Config) -> Self {
        // After the Windows installer, keep what was chosen there; elsewhere suggest it.
        let from_setup = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe").is_file()))
            .unwrap_or(false);
        let autostart = if from_setup { crate::autostart::is_enabled() } else { true };
        Self { download_dir: config.download_dir.display().to_string(), autostart, error: None }
    }
}

impl StewardGetApp {
    pub fn welcome_dialog(&mut self, ctx: &egui::Context) {
        let Some(w) = self.welcome.as_mut() else { return };
        let p = self.palette;
        let mut finish = None;
        modal("stewardget-welcome", &p).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                widgets::logo(ui, 40.0);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Welcome to StewardGet").size(19.0).family(theme::semi()).color(p.text));
                    ui.label(
                        RichText::new("Three quick choices. You can change them any time in Settings.")
                            .size(12.5)
                            .color(p.text_dim),
                    );
                });
            });
            ui.add_space(8.0);

            step(ui, &p, Icon::Folder, "Save downloads to", |ui| {
                let (_, browse) = widgets::input_with_button(ui, &mut w.download_dir, "", Icon::FolderOpen, "Browse", &p);
                if browse && let Some(folder) = rfd::FileDialog::new().set_directory(&w.download_dir).pick_folder() {
                    w.download_dir = folder.display().to_string();
                }
            });
            step(ui, &p, Icon::Power, "Start StewardGet with your computer", |ui| {
                ui.horizontal(|ui| {
                    widgets::toggle(ui, &mut w.autostart, &p);
                    note(ui, "Recommended. Browser downloads reach StewardGet only while it runs. It starts minimised.", p.text_dim);
                });
            });
            step(ui, &p, Icon::Globe, "Connect your browser", |ui| {
                note(
                    ui,
                    "Chrome, Edge, Brave, Opera, Vivaldi and Firefox can hand their downloads to StewardGet. Setting it up takes about a minute.",
                    p.text_dim,
                );
            });
            if let Some(e) = &w.error {
                ui.add_space(4.0);
                callout(ui, &p, p.red, Icon::Alert, |ui| note(ui, e.clone(), p.text));
            }
            footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Globe), "Set up browser", true, &p).clicked() {
                    finish = Some(true);
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Start using StewardGet", true, &p).clicked() {
                    finish = Some(false);
                }
            });
        });
        let Some(setup_browser) = finish else { return };

        let dir = PathBuf::from(w.download_dir.trim());
        if dir.as_os_str().is_empty() {
            w.error = Some("Choose a folder for your downloads.".into());
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            w.error = Some(format!("StewardGet can't use this folder: {e}"));
            return;
        }
        let autostart = w.autostart;
        let config = Config { download_dir: dir, ..self.engine.config() };
        // Saving also marks the first run as done.
        if let Err(e) = self.engine.update_config(config) {
            w.error = Some(capitalize(&e.to_string()));
            return;
        }
        if autostart != crate::autostart::is_enabled()
            && let Err(e) = crate::autostart::set(autostart)
        {
            let red = self.palette.red;
            self.toast(format!("Could not change the login item: {e}"), red);
        }
        self.welcome = None;
        if setup_browser {
            self.open_settings(crate::settings::Tab::Browser);
        }
    }
}

/// One numbered choice in the welcome dialog.
fn step(ui: &mut Ui, p: &Palette, icon: Icon, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(8), p.accent.gamma_multiply(if p.dark { 0.2 } else { 0.12 }));
        crate::icons::paint(ui.painter(), rect.shrink(8.0), icon, p.accent);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(label).family(theme::semi()).color(p.text));
            ui.add_space(2.0);
            add(ui);
        });
    });
    ui.add_space(6.0);
}
