//! The Tools page: find duplicate files, clear leftovers, check links before
//! downloading, back up settings and copy a diagnostics report.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::{Margin, RichText, Ui};
use stewardget_core::util::format_bytes;
use stewardget_core::{DupGroup, Leftover, LinkInfo, NewDownload};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind, card};

/// A background job's result, filled in when it ends.
type Slot<T> = Arc<Mutex<Option<T>>>;

/// A link and what the server said about it (`None` while waiting).
type Checked = (String, Option<Result<LinkInfo, String>>);

#[derive(Default)]
pub struct ToolsState {
    dup_job: Option<(Slot<Vec<DupGroup>>, Arc<AtomicBool>)>,
    dups: Option<Vec<DupGroup>>,
    min_mb: u32,
    left_job: Option<Slot<Vec<Leftover>>>,
    leftovers: Option<Vec<Leftover>>,
    links: String,
    checks: Option<Arc<Mutex<Vec<Checked>>>>,
    diagnostics: Option<String>,
    /// A destructive button waiting for its second click.
    confirm: Option<String>,
    message: Option<(String, bool)>,
}

impl ToolsState {
    fn say(&mut self, text: impl Into<String>, ok: bool) {
        self.message = Some((text.into(), ok));
    }
}

impl StewardGetApp {
    pub fn tools_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let mut t = std::mem::take(&mut self.tools);
        if t.min_mb == 0 {
            t.min_mb = 10;
        }
        // Collect finished jobs.
        if let Some((slot, _)) = &t.dup_job {
            let done = slot.lock().unwrap().take();
            if let Some(found) = done {
                t.dups = Some(found);
                t.dup_job = None;
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if let Some(slot) = &t.left_job {
            let done = slot.lock().unwrap().take();
            if let Some(found) = done {
                t.leftovers = Some(found);
                t.left_job = None;
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if t.checks.as_ref().is_some_and(|c| c.lock().unwrap().iter().any(|(_, r)| r.is_none())) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 20 }).show(ui, |ui| {
                ui.label(
                    RichText::new("Tools")
                        .size(crate::theme::size::XL + 2.0)
                        .family(crate::theme::semi())
                        .color(p.text),
                );
                ui.label(
                    RichText::new("Keep your downloads tidy, check links before downloading, and keep a backup.")
                        .size(crate::theme::size::SM)
                        .color(p.text_dim),
                );
                if let Some((text, ok)) = t.message.clone() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(text).color(if ok { p.green } else { p.red }));
                }
                ui.add_space(12.0);
                self.dup_card(ui, &mut t);
                ui.add_space(12.0);
                self.leftover_card(ui, &mut t);
                ui.add_space(12.0);
                self.link_check_card(ui, &mut t);
                ui.add_space(12.0);
                self.backup_card(ui, &mut t);
            });
        });
        self.tools = t;
    }

    fn dup_card(&mut self, ui: &mut Ui, t: &mut ToolsState) {
        let p = self.palette;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            widgets::caption(ui, "DUPLICATE FILES", &p);
            ui.label(
                RichText::new(
                    "Finds identical files in your download folders (compared by content, not by name) so you can free the space.",
                )
                .size(12.0)
                .color(p.text_dim),
            );
            ui.add_space(4.0);
            let running = t.dup_job.is_some();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Ignore files smaller than").size(12.5).color(p.text));
                ui.add_enabled(!running, egui::DragValue::new(&mut t.min_mb).range(0..=10_000).suffix(" MB"));
                if running {
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Close), "Stop", true, &p).clicked()
                        && let Some((_, cancel)) = &t.dup_job
                    {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    ui.spinner();
                    ui.label(RichText::new("Scanning…").color(p.text_dim));
                } else if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Search), "Scan download folders", true, &p)
                    .clicked()
                {
                    let (slot, cancel) = (Slot::default(), Arc::new(AtomicBool::new(false)));
                    let (dirs, min, out, flag) =
                        (self.engine.download_folders(), u64::from(t.min_mb) << 20, Arc::clone(&slot), Arc::clone(&cancel));
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        *out.lock().unwrap() = Some(stewardget_core::find_duplicates(&dirs, min, &flag));
                        ctx.request_repaint();
                    });
                    t.dup_job = Some((slot, cancel));
                    t.dups = None;
                }
            });
            let Some(groups) = t.dups.as_mut() else { return };
            ui.add_space(4.0);
            if groups.is_empty() {
                ui.label(RichText::new("No duplicates found.").color(p.green));
                return;
            }
            let wasted: u64 = groups.iter().map(DupGroup::wasted).sum();
            ui.label(
                RichText::new(format!("{} sets of identical files · {} could be freed", groups.len(), format_bytes(wasted)))
                    .family(crate::theme::semi())
                    .color(p.text),
            );
            let mut delete: Option<PathBuf> = None;
            for g in groups.iter().take(50) {
                ui.add_space(3.0);
                ui.label(RichText::new(format!("{} each", format_bytes(g.size))).size(12.0).color(p.text_dim));
                for f in &g.files {
                    widgets::file_row(ui, f, &p, |ui| {
                        {
                            let key = format!("dup:{}", f.display());
                            let armed = t.confirm.as_deref() == Some(key.as_str());
                            let (label, kind) = if armed { ("Really delete?", ButtonKind::Danger) } else { ("Delete", ButtonKind::Tool) };
                            if widgets::button(ui, kind, Some(Icon::Trash), label, g.files.len() > 1, &p).clicked() {
                                if armed {
                                    delete = Some(f.clone());
                                } else {
                                    t.confirm = Some(key);
                                }
                            }
                            if widgets::icon_button(ui, Icon::FolderOpen, "Show in folder", &p).clicked() {
                                let _ = stewardget_core::util::reveal_in_file_manager(f);
                            }
                        }
                    });
                }
            }
            if groups.len() > 50 {
                ui.label(RichText::new(format!("…and {} more sets", groups.len() - 50)).size(12.0).color(p.text_dim));
            }
            if let Some(path) = delete {
                t.confirm = None;
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        for g in groups.iter_mut() {
                            g.files.retain(|f| f != &path);
                        }
                        groups.retain(|g| g.files.len() > 1);
                        t.message = Some((format!("Deleted {}", path.display()), true));
                    }
                    Err(e) => t.message = Some((format!("Could not delete: {e}"), false)),
                }
            }
        });
    }

    fn leftover_card(&mut self, ui: &mut Ui, t: &mut ToolsState) {
        let p = self.palette;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            widgets::caption(ui, "LEFTOVERS OF UNFINISHED DOWNLOADS", &p);
            ui.label(
                RichText::new(
                    "Removing an unfinished download keeps its partial data in case you add it again. Partial files that no download in the list owns can be cleared here.",
                )
                .size(12.0)
                .color(p.text_dim),
            );
            ui.add_space(4.0);
            let running = t.left_job.is_some();
            ui.horizontal(|ui| {
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Search), "Look for leftovers", !running, &p).clicked() {
                    let slot = Slot::default();
                    let (engine, out, ctx) = (self.engine.clone(), Arc::clone(&slot), ui.ctx().clone());
                    std::thread::spawn(move || {
                        *out.lock().unwrap() = Some(engine.leftovers());
                        ctx.request_repaint();
                    });
                    t.left_job = Some(slot);
                    t.leftovers = None;
                }
                if running {
                    ui.spinner();
                }
            });
            let Some(found) = t.leftovers.as_ref() else { return };
            if found.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Nothing left over.").color(p.green));
                return;
            }
            let total: u64 = found.iter().map(|l| l.size).sum();
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("{} files · {}", found.len(), format_bytes(total))).family(crate::theme::semi()).color(p.text),
            );
            for l in found.iter().take(30) {
                widgets::file_row(ui, &l.path, &p, |ui| {
                    ui.label(RichText::new(format_bytes(l.size)).size(12.0).color(p.text_dim));
                });
            }
            if found.len() > 30 {
                ui.label(RichText::new(format!("\u{2026}and {} more", found.len() - 30)).size(12.0).color(p.text_dim));
            }
            let armed = t.confirm.as_deref() == Some("leftovers");
            let (label, kind) = if armed { ("Really delete them all?", ButtonKind::Danger) } else { ("Delete them all", ButtonKind::Tool) };
            if widgets::button(ui, kind, Some(Icon::Trash), label, true, &p).clicked() {
                if armed {
                    let paths: Vec<PathBuf> = found.iter().map(|l| l.path.clone()).collect();
                    let n = stewardget_core::delete_leftovers(&paths);
                    t.confirm = None;
                    t.leftovers = Some(Vec::new());
                    t.message = Some((format!("Deleted {n} files, freeing {}", format_bytes(total)), true));
                } else {
                    t.confirm = Some("leftovers".into());
                }
            }
        });
    }

    fn link_check_card(&mut self, ui: &mut Ui, t: &mut ToolsState) {
        let p = self.palette;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            widgets::caption(ui, "CHECK LINKS", &p);
            ui.label(
                RichText::new("Paste links to see which still work, how big they are and whether they can be resumed, before downloading anything.")
                    .size(12.0)
                    .color(p.text_dim),
            );
            ui.add_space(4.0);
            ui.add(
                widgets::input_multiline(&mut t.links)
                    .hint_text("One link per line")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            let links = stewardget_core::util::split_links(&t.links)
                .into_iter()
                .filter(|l| l.starts_with("http://") || l.starts_with("https://") || l.starts_with("ftp://"))
                .take(200)
                .collect::<Vec<_>>();
            ui.horizontal(|ui| {
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::CheckCircle), "Check", !links.is_empty(), &p).clicked() {
                    let results = Arc::new(Mutex::new(links.iter().map(|l| (l.clone(), None)).collect::<Vec<_>>()));
                    let gate = Arc::new(tokio::sync::Semaphore::new(4));
                    for (i, link) in links.iter().cloned().enumerate() {
                        let (engine, results, gate, ctx) =
                            (self.engine.clone(), Arc::clone(&results), Arc::clone(&gate), ui.ctx().clone());
                        self.runtime.spawn(async move {
                            let _permit = gate.acquire().await;
                            let r = tokio::time::timeout(
                                std::time::Duration::from_secs(25),
                                engine.check_link(&link, &Default::default()),
                            )
                            .await
                            .map_err(|_| "no answer in time".to_owned())
                            .and_then(|r| r.map_err(|e| e.to_string()));
                            results.lock().unwrap()[i].1 = Some(r);
                            ctx.request_repaint();
                        });
                    }
                    t.checks = Some(results);
                }
                ui.label(RichText::new(format!("{} link{}", links.len(), if links.len() == 1 { "" } else { "s" })).size(12.0).color(p.text_dim));
            });
            let Some(checks) = &t.checks else { return };
            let rows = checks.lock().unwrap().clone();
            ui.add_space(4.0);
            let mut good: Vec<NewDownload> = Vec::new();
            for (link, result) in &rows {
                ui.horizontal(|ui| match result {
                    None => {
                        ui.spinner();
                        ui.add(egui::Label::new(RichText::new(link).size(12.5).color(p.text_dim)).truncate());
                    }
                    Some(Ok(info)) => {
                        widgets::icon(ui, Icon::CheckCircle, 14.0, p.green);
                        let size = info.size.map_or("unknown size".to_owned(), format_bytes);
                        let resume = if info.resumable { "can resume" } else { "no resume" };
                        ui.add(
                            egui::Label::new(RichText::new(format!("{}  ·  {size}  ·  {resume}", info.file_name)).size(12.5).color(p.text))
                                .truncate(),
                        );
                        good.push(NewDownload::new(link.clone()));
                    }
                    Some(Err(e)) => {
                        widgets::icon(ui, Icon::Alert, 14.0, p.red);
                        ui.add(egui::Label::new(RichText::new(format!("{link}  —  {e}")).size(12.5).color(p.red)).truncate());
                    }
                });
            }
            if !good.is_empty()
                && rows.iter().all(|(_, r)| r.is_some())
                && widgets::button(ui, ButtonKind::Primary, Some(Icon::Download), &format!("Download the {} that work", good.len()), true, &p).clicked()
            {
                self.inbox.lock().unwrap().push_back(good);
                ui.ctx().request_repaint();
            }
        });
    }

    fn backup_card(&mut self, ui: &mut Ui, t: &mut ToolsState) {
        let p = self.palette;
        card(&p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            widgets::caption(ui, "BACKUP AND SUPPORT", &p);
            ui.label(
                RichText::new("A backup holds your settings (without access tokens), history, statistics and download list. Restoring brings back the settings only.")
                    .size(12.0)
                    .color(p.text_dim),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Export), "Save a backup…", true, &p).clicked()
                    && let Some(dest) = rfd::FileDialog::new().set_file_name("stewardget-backup.zip").add_filter("Backup", &["zip"]).save_file()
                {
                    match self.engine.export_backup(&dest) {
                        Ok(()) => t.say(format!("Backup saved to {}", dest.display()), true),
                        Err(e) => t.say(format!("Could not save the backup: {e}"), false),
                    }
                }
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Import), "Restore settings…", true, &p).clicked()
                    && let Some(src) = rfd::FileDialog::new().add_filter("Backup", &["zip"]).pick_file()
                {
                    match self.engine.import_settings(&src) {
                        Ok(()) => {
                            self.settings_draft = None;
                            t.say("Settings restored.", true);
                        }
                        Err(e) => t.say(format!("Could not restore: {e}"), false),
                    }
                }
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Info), "Make a diagnostics report", true, &p).clicked() {
                    let engine = self.engine.clone();
                    t.diagnostics = Some(engine.diagnostics());
                }
            });
            if let Some(text) = t.diagnostics.as_mut() {
                ui.add_space(4.0);
                ui.add(egui::TextEdit::multiline(text).desired_rows(8).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                ui.label(
                    RichText::new("It has no links, file names or tokens. Copy it into a bug report.").size(12.0).color(p.text_dim),
                );
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Copy), "Copy", true, &p).clicked() {
                    ui.ctx().copy_text(text.clone());
                }
            }
        });
    }
}
