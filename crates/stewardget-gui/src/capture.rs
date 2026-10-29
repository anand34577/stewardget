//! The new-download window. Downloads handed over by the browser (or by a
//! second launch of StewardGet) wait here until the user has checked the name,
//! folder and queue, like IDM's "Download File Info" window. Several links at
//! once (e.g. "Download all links") get a list to pick from.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Ui, ViewportId, vec2};
use stewardget_core::util::format_bytes;

use crate::table::free_space;
use stewardget_core::{Category, Config, LinkInfo, MAIN_QUEUE, NewDownload, QueueId, Status};

use crate::app::{ListFilter, Page, StewardGetApp, capitalize};
use crate::icons::Icon;
use crate::theme::{self, Palette};
use crate::widgets::{self, ButtonKind};

/// Batches of downloads waiting for the window, filled by the engine's capture handler.
pub type Inbox = Arc<Mutex<VecDeque<Vec<NewDownload>>>>;

const LABEL_W: f32 = 62.0;
/// Initial height of the window with a list of links.
const LIST_HEIGHT: f32 = 540.0;
const ROW_H: f32 = 26.0;

type Check = Arc<Mutex<Option<Result<LinkInfo, String>>>>;

struct Item {
    new: NewDownload,
    name: String,
    category: Category,
    selected: bool,
    /// Already in the download list.
    known: bool,
}

pub struct CaptureWindow {
    items: Vec<Item>,
    dir: String,
    /// The user picked a folder; otherwise each file goes to its category's folder.
    dir_chosen: bool,
    queue: QueueId,
    /// File name for a single download (empty: the detected one).
    name: String,
    /// `name` holds a real name (the browser's, the server's or a typed one).
    named: bool,
    search: String,
    check: Option<Check>,
    error: Option<String>,
    /// Content height of the last frame, to fit the window to it.
    height: f32,
    focus_pending: bool,
    /// "Show it" on the download already in the list.
    show_existing: Option<stewardget_core::DownloadId>,
    /// Where it opened, picked once so it never jumps.
    pos: Option<egui::Pos2>,
    /// When the window appeared: Enter pressed for something else just before must not accept it.
    opened: std::time::Instant,
}

impl CaptureWindow {
    pub fn new(app: &StewardGetApp, batch: Vec<NewDownload>) -> Self {
        let config = app.engine.config();
        let items: Vec<Item> = batch
            .into_iter()
            .map(|new| {
                let name = new.options.file_name.clone().unwrap_or_else(|| short_url(&new.url));
                let known = app.snap.downloads.iter().any(|d| d.url == new.url);
                Item { category: config.categorize(&name), name, known, selected: !known, new }
            })
            .collect();
        let mut window = Self {
            dir: String::new(),
            dir_chosen: false,
            queue: match app.filter {
                ListFilter::Queue(q) => q,
                _ => MAIN_QUEUE,
            },
            name: String::new(),
            named: false,
            search: String::new(),
            check: None,
            error: None,
            height: 0.0,
            focus_pending: true,
            show_existing: None,
            pos: None,
            opened: std::time::Instant::now(),
            items,
        };
        if window.items.len() > 1 {
            // Lists of page links: preselect the files, not the web pages.
            let files = window.items.iter().any(|i| i.category != Category::Other && !i.known);
            for item in &mut window.items {
                item.selected = !item.known && (!files || item.category != Category::Other);
            }
        } else if let Some(item) = window.items.first_mut() {
            item.selected = true;
            if let Some(name) = &item.new.options.file_name {
                window.name = name.clone();
                window.named = true;
            }
            // Ask the server for the real name and size, with the browser's cookies.
            let check: Check = Arc::default();
            let (engine, url, options, slot) =
                (app.engine.clone(), item.new.url.clone(), item.new.options.clone(), Arc::clone(&check));
            app.runtime.spawn(async move {
                let result = engine.check_link(&url, &options).await.map_err(|e| capitalize(&e.to_string()));
                *slot.lock().unwrap() = Some(result);
            });
            window.check = Some(check);
        }
        window.dir = window.default_dir(app, &config);
        window
    }

    /// The downloads as they came in, for keeping them when StewardGet quits.
    pub fn into_downloads(self) -> Vec<NewDownload> {
        self.items.into_iter().map(|i| i.new).collect()
    }

    fn single(&self) -> bool {
        self.items.len() == 1
    }

    fn checked(&self) -> Option<Result<LinkInfo, String>> {
        self.check.as_ref().and_then(|c| c.lock().unwrap().clone())
    }

    /// The name a single download will get, as shown: the known one, else a
    /// placeholder from the link.
    fn effective_name(&self) -> String {
        self.chosen_name().unwrap_or_else(|| self.items[0].name.clone())
    }

    /// A real name for a single download: typed, the browser's, or the
    /// server's. `None` while the server hasn't answered yet; the download
    /// then finds its name itself instead of keeping a guess from the link.
    fn chosen_name(&self) -> Option<String> {
        let typed = self.name.trim();
        if !typed.is_empty() {
            return Some(typed.to_owned());
        }
        match (&self.items[0].new.options.file_name, self.checked()) {
            (Some(name), _) => Some(name.clone()),
            (None, Some(Ok(info))) => Some(info.file_name),
            (None, _) => None,
        }
    }

    /// Where the download(s) go unless the user picks a folder.
    fn default_dir(&self, app: &StewardGetApp, config: &Config) -> String {
        if let Some(dir) = self.items.iter().find_map(|i| i.new.options.directory.clone()) {
            return dir.display().to_string();
        }
        if self.single() {
            return app.engine.folder_for(&self.effective_name()).display().to_string();
        }
        config.download_dir.display().to_string()
    }
}

impl StewardGetApp {
    /// Opens the next waiting batch in the window, if none is shown.
    pub fn next_capture(&mut self) {
        if self.capture.is_some() {
            return;
        }
        let Some(batch) = self.inbox.lock().unwrap().pop_front() else { return };
        if !batch.is_empty() {
            self.capture = Some(CaptureWindow::new(self, batch));
        }
    }

    pub fn capture_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.capture.take() else { return };
        let p = self.palette;
        let id = ViewportId::from_hash_of("stewardget-new-download");
        let single = w.single();
        let width = if single { 470.0 } else { 580.0 };
        let waiting: usize = self.inbox.lock().unwrap().iter().map(Vec::len).sum();
        let title = if single { "New download" } else { "New downloads" };
        let height = if single { w.height.max(240.0) } else { LIST_HEIGHT };
        let pos = *w.pos.get_or_insert_with(|| crate::app::screen_spot(ctx, vec2(width, height), 0));
        let builder = egui::ViewportBuilder::default()
            .with_position(pos)
            .with_title(format!("{title} – StewardGet"))
            .with_app_id("dev.stewardget.StewardGet")
            // A single download fits its content; the list has a fixed start size and can be resized.
            .with_inner_size([width, if single { w.height.max(240.0) } else { LIST_HEIGHT }])
            .with_min_inner_size([420.0, 220.0])
            .with_resizable(!single)
            .with_maximize_button(false)
            .with_always_on_top()
            .with_icon(crate::app_icon());

        let mut decision: Option<Decision> = None;
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            if std::mem::take(&mut w.focus_pending) && class == egui::ViewportClass::Immediate {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
            }
            let full = ui.max_rect();
            ui.painter().rect_filled(full, CornerRadius::ZERO, p.card);
            let inner = Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                if single {
                    // The title bar already says what this is: no header, like IDM's compact window.
                    self.single_body(ui, &mut w, waiting);
                } else {
                    header(ui, &p, Icon::Layers, title, waiting);
                    self.list_body(ui, &mut w);
                }
                self.common_fields(ui, &mut w);
                if let Some(e) = &w.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).size(12.5).color(p.red));
                }
                decision = footer(ui, &p, &w, waiting).or(decision);
            });
            // Fit the window to its content (single) or keep at least that much (list).
            let wanted = inner.response.rect.height() + 2.0;
            if class == egui::ViewportClass::Immediate && (wanted - w.height).abs() > 1.5 {
                w.height = wanted;
                if single {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(width, wanted)));
                }
            }
            ui.input(|i| {
                if i.viewport().close_requested() || i.key_pressed(egui::Key::Escape) {
                    decision = Some(Decision::Cancel);
                } else if i.key_pressed(egui::Key::Enter)
                    && !i.modifiers.shift
                    && single
                    && w.opened.elapsed() > Duration::from_millis(600)
                {
                    decision = decision.or(Some(Decision::Add { later: false }));
                }
            });
            if single && w.check.is_some() && w.checked().is_none() {
                ui.ctx().request_repaint_after(Duration::from_millis(150));
            }
        });

        if let Some(id) = w.show_existing.take() {
            // Its progress window, or "Download complete" if it is done.
            self.open_progress(id);
            decision = Some(Decision::Cancel);
        }
        match decision {
            None => self.capture = Some(w),
            Some(Decision::Cancel) => {}
            Some(Decision::DontAsk) => {
                let mut config = self.engine.config();
                config.capture_dialog = false;
                let r = self.engine.update_config(config);
                self.report(r);
                if let Some(draft) = &mut self.settings_draft {
                    draft.capture_dialog = false;
                }
                self.accept(&mut w, false);
                if w.error.is_some() {
                    self.capture = Some(w);
                }
            }
            Some(Decision::Add { later }) => {
                self.accept(&mut w, later);
                if w.error.is_some() {
                    self.capture = Some(w);
                }
            }
        }
        if self.capture.is_none() {
            self.next_capture();
        }
    }

    fn single_body(&mut self, ui: &mut Ui, w: &mut CaptureWindow, waiting: usize) {
        let p = self.palette;
        let check = w.checked();
        let name = w.effective_name();
        let category = self.cfg.categorize(&name);
        Frame::new()
            .fill(p.card_hover)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    widgets::icon(ui, Icon::for_category(category), 22.0, p.category(category));
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&name).size(13.5).family(theme::semi()).color(p.text))
                                .truncate(),
                        );
                        ui.horizontal(|ui| {
                            link_status(ui, &p, &check);
                            if waiting > 0 {
                                widgets::pill(ui, &format!("+{waiting} waiting"), p.accent2)
                                    .on_hover_text("More downloads from the browser are waiting for this window");
                            }
                        });
                    });
                });
                ui.horizontal(|ui| {
                    let url = &w.items[0].new.url;
                    ui.add(egui::Label::new(RichText::new(url).size(11.0).color(p.accent2)).truncate())
                        .on_hover_text(url);
                });
            });
        if w.items[0].known
            && let Some(d) = self.snap.downloads.iter().find(|d| d.url == w.items[0].new.url)
        {
            let text = if d.status == Status::Completed {
                format!("You already downloaded this: {}", d.file_name)
            } else {
                format!("Already in your list: {} ({})", d.file_name, d.status.label().to_lowercase())
            };
            ui.horizontal(|ui| {
                widgets::icon(ui, Icon::Info, 13.0, p.yellow);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::Eye), "Show it", true, &p)
                        .on_hover_text("Open the download already in your list instead")
                        .clicked()
                    {
                        w.show_existing = Some(d.id);
                    }
                    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(p.yellow)).truncate());
                });
            });
        }
        if stewardget_core::util::disguised_program(&name) {
            ui.horizontal(|ui| {
                widgets::icon(ui, Icon::Warning, 13.0, p.red);
                ui.add(
                    egui::Label::new(
                        RichText::new("This is a program disguised as a document or picture. Be careful.")
                            .size(12.0)
                            .color(p.red),
                    )
                    .truncate(),
                );
            });
        }
        ui.add_space(2.0);
        // The server's name, once known, unless the browser gave one or the user typed one.
        if !w.named
            && let Some(Ok(info)) = &check
        {
            w.name = info.file_name.clone();
            w.named = true;
            if !w.dir_chosen {
                w.dir = w.default_dir(self, &self.engine.config());
            }
        }
        widgets::form_row(ui, LABEL_W, "Save as", &p, |ui| {
            let r = ui.add(widgets::input(&mut w.name).hint_text(name.clone()).desired_width(f32::INFINITY));
            if r.changed() {
                w.named = true;
                if !w.dir_chosen {
                    // Another extension may belong in another category folder.
                    w.dir = self.engine.folder_for(&w.effective_name()).display().to_string();
                }
            }
        });
    }

    fn list_body(&mut self, ui: &mut Ui, w: &mut CaptureWindow) {
        let p = self.palette;
        let host = w.items[0].new.options.referrer.as_deref().map(host_of).unwrap_or_default();
        let selected = w.items.iter().filter(|i| i.selected).count();
        ui.label(
            RichText::new(if host.is_empty() {
                format!("{} links · {selected} selected", w.items.len())
            } else {
                format!("{} links from {host} · {selected} selected", w.items.len())
            })
            .size(12.5)
            .color(p.text_dim),
        );
        ui.add_space(5.0);

        // Pick by type: one chip per category present.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for c in self.cfg.categories() {
                let total = w.items.iter().filter(|i| i.category == c).count();
                if total == 0 {
                    continue;
                }
                let on = w.items.iter().filter(|i| i.category == c && i.selected).count();
                let label = format!("{} {on}/{total}", self.cfg.category_label(c));
                if chip(ui, &p, Icon::for_category(c), p.category(c), &label, on == total).clicked() {
                    let select = on < total;
                    for item in w.items.iter_mut().filter(|i| i.category == c) {
                        item.selected = select;
                    }
                }
            }
        });
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add(widgets::input(&mut w.search).hint_text("Filter by name or link").desired_width(260.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Both act on the links the filter shows.
                let search = w.search.clone();
                let any = w.items.iter().any(|i| matches_search(i, &search));
                if widgets::button(ui, ButtonKind::Tool, None, "None", any, &p).clicked() {
                    w.items.iter_mut().filter(|i| matches_search(i, &search)).for_each(|i| i.selected = false);
                }
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Checks), "All", any, &p).clicked() {
                    w.items.iter_mut().filter(|i| matches_search(i, &search)).for_each(|i| i.selected = true);
                }
            });
        });
        ui.add_space(6.0);

        let rows: Vec<usize> = (0..w.items.len()).filter(|&i| matches_search(&w.items[i], &w.search)).collect();
        Frame::new()
            .fill(p.bg)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(4, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Leave room for the folder, queue and buttons below.
                let height = (ui.available_height() - 215.0).max(ROW_H * 3.0);
                egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show_rows(
                    ui,
                    ROW_H,
                    rows.len(),
                    |ui, range| {
                        for &i in &rows[range] {
                            let item = &mut w.items[i];
                            ui.horizontal(|ui| {
                                ui.set_height(ROW_H);
                                ui.checkbox(&mut item.selected, "");
                                widgets::icon(ui, Icon::for_category(item.category), 15.0, p.category(item.category));
                                ui.add(egui::Label::new(RichText::new(&item.name).size(13.0).color(p.text)).truncate())
                                    .on_hover_text(&item.new.url);
                                if item.known {
                                    widgets::pill(ui, "in list", p.yellow);
                                }
                            });
                        }
                    },
                );
                if rows.is_empty() {
                    ui.label(RichText::new("No links match the filter.").color(p.text_dim));
                }
            });
        ui.add_space(6.0);
    }

    /// Folder, free space and queue: shared by both layouts.
    fn common_fields(&mut self, ui: &mut Ui, w: &mut CaptureWindow) {
        let p = self.palette;
        let config = self.engine.config();
        widgets::form_row(ui, LABEL_W, "Save to", &p, |ui| {
            if widgets::folder_field(ui, &mut w.dir, &config.recent_dirs, &p) {
                w.dir_chosen = true;
            }
        });
        let size = if w.single() {
            match w.checked() {
                Some(Ok(info)) => info.size,
                _ => None,
            }
        } else {
            None
        };
        widgets::form_row(ui, LABEL_W, "Queue", &p, |ui| {
            egui::ComboBox::from_id_salt("capture-queue")
                .width(170.0)
                .selected_text(config.queue(w.queue).name.clone())
                .show_ui(ui, |ui| {
                    for q in &config.queues {
                        ui.selectable_value(&mut w.queue, q.id, &q.name);
                    }
                });
            ui.add_space(6.0);
            if let Some(free) = free_space(std::path::Path::new(w.dir.trim())) {
                let short = size.is_some_and(|s| s > free);
                let text = match size {
                    Some(s) if short => format!("Only {} free, {} needed", format_bytes(free), format_bytes(s)),
                    _ => format!("{} free", format_bytes(free)),
                };
                widgets::icon(ui, Icon::Drive, 13.0, if short { p.red } else { p.text_dim });
                ui.add(
                    egui::Label::new(RichText::new(text).size(12.0).color(if short { p.red } else { p.text_dim }))
                        .truncate(),
                );
            }
        });
        if !w.single() && !w.dir_chosen && (config.auto_categorize || !config.category_dirs.is_empty()) {
            ui.label(RichText::new("Each file goes to its category folder.").size(12.0).color(p.text_dim));
        }
    }

    /// Adds the chosen downloads; leaves an error in the window if that fails.
    fn accept(&mut self, w: &mut CaptureWindow, later: bool) {
        let dir = PathBuf::from(w.dir.trim());
        if w.dir_chosen && dir.as_os_str().is_empty() {
            w.error = Some("Choose a folder.".into());
            return;
        }
        let single = w.single();
        let name = single.then(|| w.chosen_name()).flatten();
        let mut ids = Vec::new();
        for item in w.items.iter().filter(|i| i.selected) {
            let mut new = item.new.clone();
            if let Some(name) = name.as_ref().filter(|n| !n.trim().is_empty()) {
                new.options.file_name = Some(name.clone());
            }
            if w.dir_chosen {
                new.options.directory = Some(dir.clone());
            } else if single && name.is_some() {
                // (Without a name yet, the folder shown is only a guess: the
                // download picks its category folder once it knows the name.)
                // Keep the category folder shown in the window, even if settings change meanwhile.
                new.options.directory = new.options.directory.or_else(|| Some(dir.clone()));
            }
            new.queue = Some(w.queue);
            new.paused = later;
            match self.engine.add(new) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    w.error = Some(capitalize(&e.to_string()));
                    return;
                }
            }
        }
        if w.dir_chosen {
            let mut config = self.engine.config();
            config.remember_dir(dir);
            let r = self.engine.update_config(config);
            self.report(r);
        }
        let n = ids.len();
        if n > 0 {
            let queue = self.engine.config().queue(w.queue).name.clone();
            let what = if n == 1 { name.unwrap_or_else(|| "1 download".into()) } else { format!("{n} downloads") };
            let text = if later { format!("Added {what} to {queue}") } else { format!("Started {what}") };
            let accent = self.palette.accent2;
            self.toast(text, accent);
            // Open its progress window now, rather than leave it to the next
            // snapshot, which skips it when other downloads appear alongside.
            if !later && self.cfg.progress_window && n <= crate::app::PROGRESS_WINDOWS_AT_ONCE {
                for &id in &ids {
                    self.open_progress(id);
                }
            }
            if self.page == Page::Downloads {
                self.selection = ids.into_iter().collect();
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Decision {
    Add {
        later: bool,
    },
    /// Add, and stop asking for browser downloads.
    DontAsk,
    Cancel,
}

/// Size and resume support from the link check, or why it failed.
fn link_status(ui: &mut Ui, p: &Palette, check: &Option<Result<LinkInfo, String>>) {
    match check {
        None => {
            ui.add(egui::Spinner::new().size(13.0).color(p.accent2));
            ui.label(RichText::new("Checking the link…").size(12.0).color(p.text_dim));
        }
        Some(Ok(info)) => {
            let size = info.size.map_or("Unknown size".to_owned(), format_bytes);
            ui.label(RichText::new(size).size(12.0).color(p.text_dim));
            if info.resumable {
                widgets::pill(ui, "Resumable", p.green)
                    .on_hover_text("Pausing keeps the progress, and it downloads over several connections.");
            } else {
                widgets::pill(ui, "No resume support", p.yellow).on_hover_text(
                    "The server doesn't support byte ranges: one connection, and pausing restarts from zero.",
                );
            }
        }
        Some(Err(e)) => {
            widgets::icon(ui, Icon::Warning, 13.0, p.yellow);
            ui.add(
                egui::Label::new(RichText::new(format!("Couldn't check the link: {e}")).size(12.0).color(p.text_dim))
                    .truncate(),
            );
        }
    }
}

fn header(ui: &mut Ui, p: &Palette, icon: Icon, text: &str, waiting: usize) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(9), p.accent.gamma_multiply(if p.dark { 0.2 } else { 0.12 }));
        crate::icons::paint(ui.painter(), rect.shrink(8.0), icon, p.accent);
        ui.add_space(4.0);
        ui.label(RichText::new(text).size(17.0).family(theme::semi()).color(p.text));
        if waiting > 0 {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::pill(ui, &format!("{waiting} more waiting"), p.accent2);
            });
        }
    });
    ui.add_space(6.0);
}

fn footer(ui: &mut Ui, p: &Palette, w: &CaptureWindow, waiting: usize) -> Option<Decision> {
    let mut decision = None;
    let n = w.items.iter().filter(|i| i.selected).count();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let r = ui
            .add(
                egui::Label::new(RichText::new("Don't ask again").size(11.5).color(p.text_dim).underline())
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(
                "Start browser downloads right away, without this window. Turn it back on in Settings › General.",
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() && n > 0 {
            decision = Some(Decision::DontAsk);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let label = if w.single() || n <= 1 { "Download".to_owned() } else { format!("Download {n}") };
            if widgets::button(ui, ButtonKind::Primary, Some(Icon::Download), &label, n > 0, p)
                .on_hover_text("Start now (Enter)")
                .clicked()
            {
                decision = Some(Decision::Add { later: false });
            }
            if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Clock), "Later", n > 0, p)
                .on_hover_text("Add to the queue paused. It starts when you start the queue, or at its scheduled time.")
                .clicked()
            {
                decision = Some(Decision::Add { later: true });
            }
            let cancel = if waiting > 0 { "Skip" } else { "Cancel" };
            if widgets::button(ui, ButtonKind::Ghost, None, cancel, true, p).on_hover_text("Esc").clicked() {
                decision = Some(Decision::Cancel);
            }
        });
    });
    decision
}

/// A toggle chip for one category in the link list.
fn chip(ui: &mut Ui, p: &Palette, icon: Icon, color: egui::Color32, label: &str, on: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(12.5), p.text);
    let size = vec2(galley.size().x + 40.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let (fill, stroke) = if on {
        (color.gamma_multiply(if p.dark { 0.22 } else { 0.14 }), egui::Stroke::new(1.0, color.gamma_multiply(0.7)))
    } else if response.hovered() {
        (p.card_hover, egui::Stroke::new(1.0, p.control_border))
    } else {
        (p.card, egui::Stroke::new(1.0, p.control_border))
    };
    ui.painter().rect(rect, CornerRadius::same(14), fill, stroke, egui::StrokeKind::Inside);
    crate::icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.left_center() + vec2(17.0, 0.0), vec2(14.0, 14.0)),
        icon,
        color,
    );
    let fg = if on { p.text } else { p.text_dim };
    ui.painter().galley(rect.left_center() + vec2(30.0, -galley.size().y / 2.0), galley, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn matches_search(item: &Item, search: &str) -> bool {
    let needle = search.trim().to_lowercase();
    needle.is_empty() || item.name.to_lowercase().contains(&needle) || item.new.url.to_lowercase().contains(&needle)
}

/// Last path segment of a link, for lists of links without a file name.
fn short_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && !s.contains(':'))
        .unwrap_or(url)
        .to_owned()
}

pub(crate) fn host_of(url: &str) -> String {
    url.split_once("://").map_or(url, |(_, rest)| rest).split(['/', '?', '#']).next().unwrap_or("").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_labels() {
        assert_eq!(short_url("https://example.com/files/a.zip?x=1"), "a.zip");
        assert_eq!(short_url("https://example.com/files/"), "files");
        assert_eq!(short_url("https://example.com"), "example.com");
        assert_eq!(host_of("https://cdn.example.com/a/b"), "cdn.example.com");
    }
}
