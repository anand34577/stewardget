//! The menu bar (File, Tasks, Tools, Help) and the keyboard shortcuts sheet.

use egui::{RichText, Ui};
use stewardget_core::Status;

use crate::app::{AfterAll, Page, StewardGetApp, menu_item};
use crate::dialogs::{footer, modal, title};
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

/// Shortcut, what it does.
const SHORTCUTS: [(&str, &str); 17] = [
    ("Ctrl+N", "Add downloads"),
    ("Ctrl+V", "Add the link on the clipboard"),
    ("Ctrl+O", "Import links from a file"),
    ("Ctrl+F", "Search the list"),
    ("Ctrl+A", "Select every download in view"),
    ("↑ ↓ · Page Up/Down", "Move through the list"),
    ("Home · End", "First or last download"),
    ("Shift+↑ ↓ · Shift+click", "Select a range"),
    ("Ctrl+click", "Add to or remove from the selection"),
    ("Space", "Pause or resume the selection"),
    ("Enter · double-click", "Open a finished file, or the progress window"),
    ("F2 · Alt+Enter", "Properties"),
    ("Delete", "Remove from the list"),
    ("Shift+Delete", "Remove and delete the files"),
    ("Ctrl+,", "Settings"),
    ("F1", "This list"),
    ("Ctrl+Q", "Quit StewardGet"),
];

impl StewardGetApp {
    pub fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| self.file_menu(ui));
            ui.menu_button("Tasks", |ui| self.tasks_menu(ui));
            ui.menu_button("Tools", |ui| self.tools_menu(ui));
            ui.menu_button("Help", |ui| self.help_menu(ui));
        });
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        let p = self.palette;
        if menu_item(ui, &p, Icon::Plus, "Add downloads…  Ctrl+N", true) {
            self.page = Page::Downloads;
            self.open_add(None);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Layers, "Batch download…", true) {
            self.open_add(Some("https://example.com/files/image[001-100].jpg".into()));
            ui.close();
        }
        if menu_item(ui, &p, Icon::Globe, "Grab files from a web page…", true) {
            self.grab_dialog = Some(crate::grab::GrabDialog::new(None));
            ui.close();
        }
        if menu_item(ui, &p, Icon::Video, "Video or audio from a page…", true) {
            let dir = self.engine.config().download_dir.display().to_string();
            self.media_dialog = Some(crate::media::MediaDialog::new(None, dir));
            ui.close();
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Import, "Import links…  Ctrl+O", true) {
            self.import_links();
            ui.close();
        }
        let n = if self.selection.is_empty() { self.visible_rows().len() } else { self.selection.len() };
        if menu_item(ui, &p, Icon::Export, &format!("Export {n} links…"), n > 0) {
            self.export_links();
            ui.close();
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Power, "Quit StewardGet  Ctrl+Q", true) {
            self.quit(ui.ctx());
            ui.close();
        }
    }

    fn tasks_menu(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let sel = self.selected_views();
        let can_resume = sel.iter().any(|d| d.status.can_resume());
        let can_pause = sel.iter().any(|d| d.status.can_pause());
        let has_sel = !sel.is_empty();
        let single = self.single_selected();
        let can_refresh = single.as_ref().is_some_and(|d| d.status != Status::Completed);
        let can_watch = single.as_ref().is_some_and(|d| d.status != Status::Completed);
        if menu_item(ui, &p, Icon::Play, "Resume  Space", can_resume) {
            self.resume_selected();
            ui.close();
        }
        if menu_item(ui, &p, Icon::Pause, "Pause  Space", can_pause) {
            self.pause_selected();
            ui.close();
        }
        if menu_item(ui, &p, Icon::Retry, "Start over…", has_sel) {
            self.restart_selected();
            ui.close();
        }
        if menu_item(ui, &p, Icon::Link, "Refresh link…", can_refresh)
            && let Some(d) = &single
        {
            self.open_refresh(d);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Eye, "Progress window", can_watch)
            && let Some(d) = &single
        {
            self.open_progress(d.id);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Sliders, "Properties…  F2", single.is_some())
            && let Some(d) = &single
        {
            self.open_properties(d);
            ui.close();
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Trash, "Remove  Del", has_sel) {
            self.remove_selected(false);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Trash, "Remove and delete files  Shift+Del", has_sel) {
            self.remove_selected(true);
            ui.close();
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Play, "Start all", true) {
            self.engine.resume_all();
            ui.close();
        }
        if menu_item(ui, &p, Icon::Pause, "Pause all", true) {
            self.engine.pause_all();
            ui.close();
        }
        let done = self.snap.count(|s| s == Status::Completed);
        if menu_item(ui, &p, Icon::Checks, &format!("Clear {done} completed"), done > 0) {
            self.engine.clear_completed();
            ui.close();
        }
    }

    fn tools_menu(&mut self, ui: &mut Ui) {
        let p = self.palette;
        if menu_item(ui, &p, Icon::Calendar, "Queues & schedule…", true) {
            self.open_settings(crate::settings::Tab::Scheduler);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Queue, "New queue…", true) {
            self.open_queue_dialog(None);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Sliders, "Utilities (duplicates, cleanup, link check, backup)…", true) {
            self.page = Page::Tools;
            ui.close();
        }
        if menu_item(ui, &p, Icon::Chart, "Statistics", true) {
            self.page = Page::Stats;
            ui.close();
        }
        if menu_item(ui, &p, Icon::Globe, "Browser extension…", true) {
            self.open_settings(crate::settings::Tab::Browser);
            ui.close();
        }
        ui.separator();
        ui.menu_button("     When all downloads finish", |ui| {
            for action in AfterAll::ALL {
                if ui.selectable_label(self.after_all == action, action.label()).clicked() {
                    self.after_all = action;
                    ui.close();
                }
            }
        });
        ui.separator();
        if menu_item(ui, &p, Icon::Gear, "Settings  Ctrl+,", true) {
            self.open_settings(crate::settings::Tab::General);
            ui.close();
        }
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        let p = self.palette;
        if menu_item(ui, &p, Icon::Info, "Keyboard shortcuts  F1", true) {
            self.shortcuts_open = true;
            ui.close();
        }
        if menu_item(ui, &p, Icon::FolderOpen, "Open data folder", self.engine.data_dir().is_some())
            && let Some(dir) = self.engine.data_dir()
        {
            let _ = stewardget_core::util::open_with_system(&dir);
            ui.close();
        }
        if menu_item(ui, &p, Icon::External, "StewardGet on GitHub", true) {
            let _ = stewardget_core::util::open_url(env!("CARGO_PKG_REPOSITORY"));
            ui.close();
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Info, "About StewardGet", true) {
            self.open_settings(crate::settings::Tab::About);
            ui.close();
        }
    }

    pub fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.shortcuts_open {
            return;
        }
        let p = self.palette;
        let mut close = false;
        let m = modal("stewardget-shortcuts", &p).show(ctx, |ui| {
            ui.set_width(520.0);
            title(ui, &p, Icon::Info, "Keyboard shortcuts");
            // Scrolls when the window is short, so Close stays reachable.
            let max_h = (ui.ctx().content_rect().height() - 190.0).max(160.0);
            egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                egui::Grid::new("shortcut-grid").num_columns(2).spacing([18.0, 2.0]).show(ui, |ui| {
                    for (keys, what) in SHORTCUTS {
                        ui.label(RichText::new(keys).size(12.5).family(crate::theme::semi()).color(p.text));
                        ui.label(RichText::new(what).size(12.5).color(p.text_dim));
                        ui.end_row();
                    }
                });
            });
            footer(ui, |ui| {
                if widgets::button(ui, ButtonKind::Primary, None, "Close", true, &p).clicked() {
                    close = true;
                }
            });
        });
        if close || m.should_close() {
            self.shortcuts_open = false;
        }
    }
}
