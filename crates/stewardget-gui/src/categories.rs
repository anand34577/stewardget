//! Custom categories (the editor on the Categories settings page) and
//! backing up and restoring the settings.

use egui::RichText;
use stewardget_core::{Config, CustomCategory};

use crate::app::{StewardGetApp, capitalize};
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

const LABEL_W: f32 = 96.0;

/// Creates or edits a custom category in the settings draft (applied with Save).
pub struct CategoryDialog {
    /// `None` for a new category.
    id: Option<u32>,
    name: String,
    extensions: String,
    error: Option<String>,
    focus: bool,
}

impl CategoryDialog {
    pub fn create() -> Self {
        Self { id: None, name: String::new(), extensions: String::new(), error: None, focus: true }
    }

    pub fn edit(category: &CustomCategory) -> Self {
        Self {
            id: Some(category.id),
            name: category.name.clone(),
            extensions: category.extensions.join(", "),
            error: None,
            focus: false,
        }
    }

    /// Checks the input against the other categories and applies it to `draft`.
    fn apply(&self, draft: &mut Config) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Give the category a name.".into());
        }
        let taken = draft
            .categories()
            .into_iter()
            .filter(|c| !matches!(c, stewardget_core::Category::Custom(id) if Some(*id) == self.id))
            .any(|c| draft.category_label(c).eq_ignore_ascii_case(name));
        if taken {
            return Err(format!("There is already a category called \u{201c}{name}\u{201d}."));
        }
        let extensions = CustomCategory::parse_extensions(&self.extensions);
        if extensions.is_empty() {
            return Err("List at least one file extension, e.g. iso, img.".into());
        }
        let id = self.id.unwrap_or_else(|| draft.next_category_id());
        let category = CustomCategory { id, name: name.to_owned(), extensions };
        match draft.custom_categories.iter_mut().find(|c| c.id == id) {
            Some(c) => *c = category,
            None => draft.custom_categories.push(category),
        }
        Ok(())
    }
}

impl StewardGetApp {
    pub fn category_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.category_dialog.take() else { return };
        let p = self.palette;
        let mut close = false;
        let mut save = false;
        let modal = crate::dialogs::modal("stewardget-category", &p).show(ctx, |ui| {
            ui.set_width(480.0);
            let title = if d.id.is_some() { "Edit category" } else { "New category" };
            crate::dialogs::title(ui, &p, Icon::Layers, title);
            widgets::form_row(ui, LABEL_W, "Name", &p, |ui| {
                let r = ui.add(
                    widgets::input(&mut d.name).hint_text("e.g. Disk images, Fonts, Ebooks").desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
            });
            widgets::form_row(ui, LABEL_W, "Extensions", &p, |ui| {
                ui.add(widgets::input(&mut d.extensions).hint_text("iso, img, vhdx").desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
                crate::dialogs::note(
                    ui,
                    "Separate them with commas or spaces. Files with these extensions join this category instead of a built-in one. The new category gets a folder and a sidebar entry.",
                    p.text_dim,
                );
            });
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                crate::dialogs::note(ui, e.as_str(), p.red);
            }
            crate::dialogs::footer(ui, |ui| {
                let label = if d.id.is_some() { "Done" } else { "Add category" };
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Check), label, true, &p).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    save = true;
                }
                if widgets::button(ui, ButtonKind::Ghost, None, "Cancel", true, &p).clicked() {
                    close = true;
                }
                ui.label(RichText::new("Click Save on the settings page to apply.").size(12.0).color(p.text_dim));
            });
        });
        close |= modal.should_close();
        if save {
            let mut draft = self.settings_draft.take().unwrap_or_else(|| self.engine.config());
            match d.apply(&mut draft) {
                Ok(()) => close = true,
                Err(e) => d.error = Some(e),
            }
            self.settings_draft = Some(draft);
        }
        if !close {
            self.category_dialog = Some(d);
        }
    }

    /// Saves the settings to a file the user picks.
    pub fn export_settings(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("StewardGet settings", &["json"])
            .set_file_name("stewardget-settings.json")
            .save_file()
        else {
            return;
        };
        let result = serde_json::to_vec_pretty(&self.engine.config())
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                let green = self.palette.green;
                self.toast("Settings backed up", green);
            }
            Err(e) => {
                let red = self.palette.red;
                self.toast(format!("Could not write the backup: {e}"), red);
            }
        }
    }

    /// Loads and applies settings from a backup file; returns them for the
    /// settings page's draft.
    pub fn import_settings(&mut self, ctx: &egui::Context) -> Option<Config> {
        let path = rfd::FileDialog::new().add_filter("StewardGet settings", &["json"]).pick_file()?;
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Config>(&bytes).map_err(|e| format!("not a StewardGet settings file ({e})"))
            })
            .and_then(|config| self.engine.update_config(config).map_err(|e| capitalize(&e.to_string())));
        match result {
            Ok(()) => {
                self.apply_appearance(ctx);
                let green = self.palette.green;
                self.toast("Settings restored", green);
                Some(self.engine.config())
            }
            Err(e) => {
                let red = self.palette.red;
                self.toast(format!("Could not restore: {e}"), red);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_need_a_unique_name_and_extensions() {
        let mut draft = Config::default();
        let mut d = CategoryDialog::create();
        d.name = "Music".into();
        d.extensions = "mid".into();
        assert!(d.apply(&mut draft).unwrap_err().contains("already"), "built-in names are taken");
        d.name = "Disk images".into();
        d.extensions = " , ".into();
        assert!(d.apply(&mut draft).is_err());
        d.extensions = "ISO, .img".into();
        d.apply(&mut draft).unwrap();
        assert_eq!(draft.custom_categories[0].extensions, ["iso", "img"]);
        // Editing keeps the id and may keep the name.
        let mut e = CategoryDialog::edit(&draft.custom_categories[0]);
        e.extensions.push_str(", vhdx");
        e.apply(&mut draft).unwrap();
        assert_eq!(draft.custom_categories.len(), 1);
        assert_eq!(draft.custom_categories[0].extensions.len(), 3);
    }
}
