//! The command palette (Ctrl+K): one box to reach any page, setting, action or download.

use egui::{Key, RichText};
use stewardget_core::DownloadId;

use crate::app::{ListFilter, Page, StewardGetApp};
use crate::dialogs::modal;
use crate::icons::Icon;
use crate::settings::Tab;
use crate::widgets;

#[derive(Default)]
pub struct Palette {
    query: String,
    cursor: usize,
    focus: bool,
}

#[derive(Clone)]
enum Command {
    Page(Page),
    Settings(Tab),
    AddUrl,
    Import,
    PauseAll,
    ResumeAll,
    ClearCompleted,
    Shortcuts,
    Filter(ListFilter),
    Download(DownloadId),
}

impl StewardGetApp {
    pub fn open_palette(&mut self) {
        self.palette_box = Some(Palette { focus: true, ..Palette::default() });
    }

    fn palette_commands(&self, query: &str) -> Vec<(Icon, String, Command)> {
        let mut all: Vec<(Icon, String, Command)> = vec![
            (Icon::Plus, "Add URL…".into(), Command::AddUrl),
            (Icon::FolderOpen, "Import links from a file…".into(), Command::Import),
            (Icon::Play, "Start all downloads".into(), Command::ResumeAll),
            (Icon::Pause, "Pause all downloads".into(), Command::PauseAll),
            (Icon::Checks, "Clear completed downloads".into(), Command::ClearCompleted),
            (Icon::Info, "Keyboard shortcuts".into(), Command::Shortcuts),
            (Icon::Inbox, "Show all downloads".into(), Command::Filter(ListFilter::All)),
            (Icon::Alert, "Show failed downloads".into(), Command::Filter(ListFilter::Failed)),
            (Icon::Download, "Show running downloads".into(), Command::Filter(ListFilter::Active)),
            (Icon::History, "Go to History".into(), Command::Page(Page::History)),
            (Icon::Info, "Go to Overview".into(), Command::Page(Page::Overview)),
            (Icon::Info, "Go to Statistics".into(), Command::Page(Page::Stats)),
            (Icon::Info, "Go to Tools".into(), Command::Page(Page::Tools)),
        ];
        for (tab, icon, name) in Tab::ALL {
            all.push((icon, format!("Settings › {name}"), Command::Settings(tab)));
        }
        let needle = query.trim().to_lowercase();
        let mut out: Vec<_> =
            all.into_iter().filter(|(_, l, _)| needle.is_empty() || l.to_lowercase().contains(&needle)).collect();
        if !needle.is_empty() {
            let downloads = self
                .snap
                .downloads
                .iter()
                .filter(|d| d.file_name.to_lowercase().contains(&needle))
                .take(8)
                .map(|d| (Icon::Download, d.file_name.clone(), Command::Download(d.id)));
            out.extend(downloads);
        }
        out.truncate(14);
        out
    }

    pub fn palette_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut pal) = self.palette_box.take() else { return };
        let p = self.palette;
        let mut run = None;
        let m = modal("stewardget-palette", &p).show(ctx, |ui| {
            ui.set_width(460.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut pal.query)
                    .hint_text("Type a command, page, setting or download name…")
                    .desired_width(f32::INFINITY),
            );
            if pal.focus {
                edit.request_focus();
                pal.focus = false;
            }
            if edit.changed() {
                pal.cursor = 0;
            }
            let items = self.palette_commands(&pal.query);
            let last = items.len().saturating_sub(1);
            ui.input(|i| {
                if i.key_pressed(Key::ArrowDown) {
                    pal.cursor = (pal.cursor + 1).min(last);
                }
                if i.key_pressed(Key::ArrowUp) {
                    pal.cursor = pal.cursor.saturating_sub(1);
                }
            });
            if ui.input(|i| i.key_pressed(Key::Enter))
                && let Some((_, _, c)) = items.get(pal.cursor)
            {
                run = Some(c.clone());
            }
            ui.add_space(6.0);
            if items.is_empty() {
                ui.label(RichText::new("Nothing matches.").color(p.text_dim));
            }
            for (n, (icon, label, cmd)) in items.iter().enumerate() {
                let selected = n == pal.cursor;
                ui.horizontal(|ui| {
                    widgets::icon(ui, *icon, 14.0, if selected { p.accent } else { p.text_dim });
                    let text = RichText::new(label).color(if selected { p.text } else { p.text_dim });
                    if ui.selectable_label(selected, text).clicked() {
                        run = Some(cmd.clone());
                    }
                });
            }
        });
        if let Some(cmd) = run {
            self.run_command(cmd);
        } else if !m.should_close() {
            self.palette_box = Some(pal);
        }
    }

    fn run_command(&mut self, cmd: Command) {
        match cmd {
            Command::Page(page) => self.page = page,
            Command::Settings(tab) => self.open_settings(tab),
            Command::AddUrl => self.open_add(None),
            Command::Import => self.import_links(),
            Command::PauseAll => self.engine.pause_all(),
            Command::ResumeAll => self.engine.resume_all(),
            Command::ClearCompleted => self.engine.clear_completed(),
            Command::Shortcuts => self.shortcuts_open = true,
            Command::Filter(f) => {
                self.page = Page::Downloads;
                self.filter = f;
            }
            Command::Download(id) => {
                self.page = Page::Downloads;
                self.filter = ListFilter::All;
                self.search.clear();
                self.selection.clear();
                self.selection.insert(id);
                self.cursor = Some(id);
                self.anchor = Some(id);
            }
        }
    }
}
