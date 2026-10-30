//! Settings page with a tabbed layout.

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, vec2,
};
use stewardget_core::util::{format_bytes, format_duration};
use stewardget_core::{Accent, ApiState, Category, CloseAction, Config, ExistingFile, Theme};

use crate::app::StewardGetApp;
use crate::icons::{self, Icon};
use crate::theme::{self, CONTROL_H, Palette};
use crate::widgets::{self, ButtonKind, card, toggle};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Appearance,
    Connection,
    Network,
    Scheduler,
    Folders,
    Browser,
    Integrations,
    Privacy,
    About,
}

impl Tab {
    pub(crate) const ALL: [(Tab, Icon, &'static str); 10] = [
        (Tab::General, Icon::Gear, "General"),
        (Tab::Appearance, Icon::Palette, "Appearance"),
        (Tab::Connection, Icon::Gauge, "Downloads"),
        (Tab::Network, Icon::Network, "Network"),
        (Tab::Scheduler, Icon::Queue, "Queues & schedule"),
        (Tab::Folders, Icon::Layers, "Categories"),
        (Tab::Browser, Icon::Globe, "Browser"),
        (Tab::Integrations, Icon::Link, "AI & automation"),
        (Tab::Privacy, Icon::Lock, "Privacy & data"),
        (Tab::About, Icon::Info, "About"),
    ];
}

/// Browsers the extension setup has steps for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
    Opera,
    Vivaldi,
    Firefox,
}

impl Browser {
    const ALL: [(Browser, &'static str); 6] = [
        (Browser::Chrome, "Chrome"),
        (Browser::Edge, "Edge"),
        (Browser::Brave, "Brave"),
        (Browser::Opera, "Opera"),
        (Browser::Vivaldi, "Vivaldi"),
        (Browser::Firefox, "Firefox"),
    ];

    /// The browser's extensions page.
    fn page(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome://extensions",
            Browser::Edge => "edge://extensions",
            Browser::Brave => "brave://extensions",
            Browser::Opera => "opera://extensions",
            Browser::Vivaldi => "vivaldi://extensions",
            Browser::Firefox => "about:debugging#/runtime/this-firefox",
        }
    }

    fn developer_mode(self) -> &'static str {
        match self {
            Browser::Edge => "Turn on Developer mode in the left pane (on a narrow window, open the menu first).",
            _ => "Turn on Developer mode in the top-right corner.",
        }
    }
}

const RELEASES: &str = "https://github.com/anand34577/stewardget/releases/latest";

impl StewardGetApp {
    pub fn settings_page(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let mut draft = self.settings_draft.take().unwrap_or_else(|| self.engine.config());
        let saved = self.engine.config();
        let dirty = draft != saved;

        egui::Frame::new().inner_margin(Margin::symmetric(20, 16)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Settings").size(19.0).family(theme::semi()).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Check), "Save", dirty, &p).clicked() {
                        match self.engine.update_config(draft.clone()) {
                            Ok(()) => {
                                draft = self.engine.config();
                                self.apply_appearance(ui.ctx());
                                let green = self.palette.green;
                                self.toast("Settings saved", green);
                            }
                            Err(e) => {
                                let red = self.palette.red;
                                self.toast(crate::app::capitalize(&e.to_string()), red);
                            }
                        }
                    }
                    if widgets::button(ui, ButtonKind::Ghost, None, "Discard", dirty, &p).clicked() {
                        draft = saved.clone();
                    }
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::Retry), "Defaults", true, &p)
                        .on_hover_text("Reset every setting (your download folder is kept). Nothing is saved until you click Save.")
                        .clicked()
                    {
                        draft = Config { download_dir: draft.download_dir.clone(), ..Config::default() };
                    }
                    if dirty {
                        ui.label(RichText::new("Unsaved changes").size(12.5).color(p.yellow));
                    }
                });
            });
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.settings_search)
                    .hint_text("Search settings…")
                    .desired_width(260.0),
            );
            ui.add_space(6.0);
            let searching = !self.settings_search.trim().is_empty();

            let narrow = ui.available_width() < 760.0;
            if searching {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.settings_search_results(ui, &mut draft));
            } else if narrow {
                ui.horizontal_wrapped(|ui| {
                    for (tab, icon, label) in Tab::ALL {
                        if tab_button(ui, icon, label, self.settings_tab == tab, &p, false).clicked() {
                            self.settings_tab = tab;
                        }
                    }
                });
                ui.add_space(5.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.settings_tab_body(ui, &mut draft));
            } else {
                // Two fixed columns: tab list and a scrolling body.
                let full = ui.available_rect_before_wrap();
                let tabs = egui::Rect::from_min_size(full.min, vec2(200.0, full.height()));
                let body = egui::Rect::from_min_max(egui::pos2(full.min.x + 216.0, full.min.y), full.max);
                ui.scope_builder(egui::UiBuilder::new().max_rect(tabs), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (tab, icon, label) in Tab::ALL {
                        if tab_button(ui, icon, label, self.settings_tab == tab, &p, true).clicked() {
                            self.settings_tab = tab;
                        }
                    }
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_max_width(1400.0_f32.min(body.width()));
                        self.settings_tab_body(ui, &mut draft);
                    });
                });
                ui.allocate_rect(full, Sense::hover());
            }
        });
        self.settings_draft = Some(draft);
    }

    /// Lists the settings that match the search; clicking one opens its tab.
    fn settings_search_results(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        let needle = self.settings_search.trim().to_lowercase();
        let real_tab = self.settings_tab;
        let mut all = Vec::new();
        ui.scope_builder(egui::UiBuilder::new().invisible(), |ui| {
            for (tab, _, _) in Tab::ALL {
                COLLECT.with_borrow_mut(|c| *c = Some((tab, Vec::new())));
                self.settings_tab = tab;
                self.settings_tab_body(ui, draft);
                if let Some((_, found)) = COLLECT.with_borrow_mut(|c| c.take()) {
                    all.extend(found);
                }
            }
        });
        self.settings_tab = real_tab;
        let hits: Vec<_> = all
            .into_iter()
            .filter(|(_, label, help)| label.to_lowercase().contains(&needle) || help.to_lowercase().contains(&needle))
            .collect();
        if hits.is_empty() {
            ui.label(RichText::new("No setting matches that.").color(p.text_dim));
            return;
        }
        for (tab, label, help) in hits {
            let tab_name = Tab::ALL.iter().find(|(t, _, _)| *t == tab).map_or("", |(_, _, n)| *n);
            let r = ui
                .scope(|ui| {
                    ui.label(RichText::new(&label).color(p.text));
                    ui.add(egui::Label::new(RichText::new(&help).size(12.0).color(p.text_dim)).wrap());
                    ui.label(RichText::new(format!("in {tab_name}")).size(12.0).color(p.accent));
                })
                .response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.clicked() {
                self.settings_tab = tab;
                self.settings_search.clear();
            }
            ui.add_space(8.0);
        }
    }

    fn settings_tab_body(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        match self.settings_tab {
            Tab::General => section(ui, &p, "General", |ui| {
                row(ui, &p, "Download folder", "Where new downloads are saved.", |ui| {
                    let mut dir = draft.download_dir.display().to_string();
                    let (_, browse) = widgets::input_with_button(ui, &mut dir, "", Icon::FolderOpen, "Browse", &p);
                    if browse
                        && let Some(folder) = rfd::FileDialog::new().set_directory(&draft.download_dir).pick_folder()
                    {
                        dir = folder.display().to_string();
                    }
                    draft.download_dir = dir.into();
                });
                row(
                    ui,
                    &p,
                    "Sort into category folders",
                    "Video, Music, Archives… each get their own sub-folder.",
                    |ui| {
                        toggle(ui, &mut draft.auto_categorize, &p);
                    },
                );
                row(
                    ui,
                    &p,
                    "Start with your computer",
                    "Opens StewardGet minimised when you sign in, so browser downloads always reach it. Applies right away.",
                    |ui| {
                        let mut on = self.autostart.unwrap_or(false);
                        if toggle(ui, &mut on, &p).clicked() {
                            if let Err(e) = crate::autostart::set(on) {
                                let red = self.palette.red;
                                self.toast(format!("Could not change the login item: {e}"), red);
                            }
                            self.autostart = Some(crate::autostart::is_enabled());
                        }
                    },
                );
                row(ui, &p, "Resume on start", "Continue unfinished downloads when StewardGet opens.", |ui| {
                    toggle(ui, &mut draft.resume_on_start, &p);
                });
                row(ui, &p, "Watch clipboard", "Offer to download file links you copy anywhere.", |ui| {
                    toggle(ui, &mut draft.clipboard_monitor, &p);
                });
                row(
                    ui,
                    &p,
                    "New-download window",
                    "For downloads from the browser, show name, size, folder and queue first, with Download now and Download later.",
                    |ui| {
                        toggle(ui, &mut draft.capture_dialog, &p);
                    },
                );
                row(
                    ui,
                    &p,
                    "Progress window",
                    "Open a small window with the speed, each connection's progress and Pause for each download you start on its own, also while StewardGet is in the tray (batches don't open one each). It turns into Download complete when the file is done. Double-click a download to open it any time.",
                    |ui| {
                        toggle(ui, &mut draft.progress_window, &p);
                    },
                );
                row(
                    ui,
                    &p,
                    "Closing the window",
                    "Keep running minimises StewardGet, so downloads continue and the browser can still hand it new ones.",
                    |ui| {
                        widgets::segmented(
                            ui,
                            &mut draft.close_action,
                            &[
                                (CloseAction::Ask, "Ask"),
                                (CloseAction::Background, "Keep running"),
                                (CloseAction::Quit, "Quit"),
                            ],
                            &p,
                        );
                    },
                );
                if cfg!(any(windows, target_os = "macos")) {
                    row(
                        ui,
                        &p,
                        "Minimise to the tray",
                        "The minimise button and Keep running hide StewardGet in the notification area instead of the taskbar. Click its icon to bring it back.",
                        |ui| {
                            toggle(ui, &mut draft.minimize_to_tray, &p);
                        },
                    );
                }
                row(
                    ui,
                    &p,
                    "Download complete window",
                    "Also show Download complete, with Open and Show in folder, for downloads that had no progress window open, such as the files of a batch.",
                    |ui| {
                        toggle(ui, &mut draft.completion_dialog, &p);
                    },
                );
                row(
                    ui,
                    &p,
                    "Completion sound",
                    "Play the system sound when a download finishes while StewardGet is in the background.",
                    |ui| {
                        toggle(ui, &mut draft.completion_sound, &p);
                    },
                );
                row(
                    ui,
                    &p,
                    "Notifications",
                    "A system notification when a download finishes or fails while StewardGet is in the background.",
                    |ui| {
                        toggle(ui, &mut draft.notifications, &p);
                    },
                );
            }),
            Tab::Appearance => {
                section(ui, &p, "Theme", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                        for theme in Theme::ALL {
                            if theme_card(ui, theme, draft.accent, draft.theme == theme, &p).clicked() {
                                draft.theme = theme;
                            }
                        }
                    });
                });
                section(ui, &p, "Accent colour", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                        for accent in Accent::ALL {
                            if accent_swatch(ui, accent, draft.accent == accent, &p).clicked() {
                                draft.accent = accent;
                            }
                        }
                    });
                });
                section(ui, &p, "Text and spacing", |ui| {
                    row(ui, &p, "Interface size", "Zoom the whole interface. Ctrl + and Ctrl − also work.", |ui| {
                        let mut pct = (draft.ui_scale * 100.0).round() as i32;
                        widgets::segmented(
                            ui,
                            &mut pct,
                            &[(90, "90%"), (100, "100%"), (110, "110%"), (125, "125%"), (150, "150%")],
                            &p,
                        );
                        draft.ui_scale = pct as f32 / 100.0;
                    });
                    row(
                        ui,
                        &p,
                        "List view",
                        "One dense line per download, with sortable columns (right-click the header to pick them). Off shows each download as a card.",
                        |ui| {
                            toggle(ui, &mut draft.compact_rows, &p);
                        },
                    );
                });
                ui.label(RichText::new("Changes apply when you click Save.").size(12.0).color(p.text_dim));
            }
            Tab::Connection => {
                section(ui, &p, "Queue", |ui| {
                    row(ui, &p, "Simultaneous downloads", "How many files transfer at the same time.", |ui| {
                        ui.add(
                            egui::Slider::new(&mut draft.max_concurrent, 1..=16).clamping(egui::SliderClamping::Edits),
                        );
                    });
                    row(ui, &p, "Global speed limit", "For example 2M or 500k. Leave empty for unlimited.", |ui| {
                        widgets::speed_input(ui, "global", &mut draft.speed_limit, 150.0);
                    });
                    row(
                        ui,
                        &p,
                        "Monthly data limit",
                        "For a metered or phone connection: downloads pause when this much has been downloaded in the calendar month, and start again next month or when you raise it. Empty for none.",
                        |ui| {
                            widgets::speed_input(ui, "monthly-cap", &mut draft.monthly_cap, 150.0);
                        },
                    );
                    row(
                        ui,
                        &p,
                        "Different limit during set hours",
                        "For example slower during work hours, so the connection stays usable. Overrides the limit above while it runs.",
                        |ui| {
                            toggle(ui, &mut draft.speed_schedule.enabled, &p);
                        },
                    );
                    if draft.speed_schedule.enabled {
                        let s = &mut draft.speed_schedule;
                        row(ui, &p, "Limit during those hours", "Empty for unlimited.", |ui| {
                            widgets::speed_input(ui, "scheduled", &mut s.limit, 150.0);
                        });
                        row(ui, &p, "Hours", "An end before the start runs past midnight.", |ui| {
                            time_picker(ui, "speed-from", &mut s.start_minute);
                            ui.label(RichText::new("to").color(p.text_dim));
                            time_picker(ui, "speed-to", &mut s.end_minute);
                        });
                        row(ui, &p, "Days", "The days the limit starts on.", |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (i, day) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
                                if day_chip(ui, day, s.days[i], &p).clicked() {
                                    s.days[i] = !s.days[i];
                                }
                            }
                        });
                    }
                });
                section(ui, &p, "Connections", |ui| {
                    row(
                        ui,
                        &p,
                        "Connections per file",
                        "Parallel connections for each download. 8–16 suits most servers; StewardGet automatically backs off when a server limits connections.",
                        |ui| {
                            ui.add(
                                egui::Slider::new(&mut draft.connections, 1..=32).clamping(egui::SliderClamping::Edits),
                            );
                        },
                    );
                    row(ui, &p, "Minimum segment size", "Ranges are never split below this.", |ui| {
                        egui::ComboBox::from_id_salt("min-split")
                            .width(120.0)
                            .selected_text(format_bytes(draft.min_split_size))
                            .show_ui(ui, |ui| {
                                for kib in [256u64, 512, 1024, 2048, 4096, 8192] {
                                    ui.selectable_value(
                                        &mut draft.min_split_size,
                                        kib * 1024,
                                        format_bytes(kib * 1024),
                                    );
                                }
                            });
                    });
                });
                self.links_section(ui, draft);
                section(ui, &p, "When something goes wrong", |ui| {
                    row(
                        ui,
                        &p,
                        "Retries per connection",
                        "Network errors are retried right away with exponential back-off.",
                        |ui| {
                            ui.add(egui::Slider::new(&mut draft.max_retries, 0..=50));
                        },
                    );
                    row(
                        ui,
                        &p,
                        "Retry failed downloads",
                        "When a download still fails because of the network or the server, try it again later. 0 turns this off.",
                        |ui| {
                            ui.add(egui::Slider::new(&mut draft.auto_retry, 0..=20).suffix(" times"));
                        },
                    );
                    row(ui, &p, "First retry after", "The pause doubles with every attempt.", |ui| {
                        ui.add(egui::Slider::new(&mut draft.auto_retry_delay_secs, 5..=600).suffix(" s"));
                    });
                });
                section(ui, &p, "When a download finishes", |ui| {
                    row(
                        ui,
                        &p,
                        "If the file already exists",
                        "Keep both gives the new file a number, like setup (1).exe.",
                        |ui| {
                            widgets::segmented(
                                ui,
                                &mut draft.existing_file,
                                &[(ExistingFile::Rename, "Keep both"), (ExistingFile::Overwrite, "Replace")],
                                &p,
                            );
                        },
                    );
                    row(
                        ui,
                        &p,
                        "Run a program",
                        "For example a virus scanner or an unpacker. {path}, {dir}, {name} and {url} are filled in; quote paths with spaces. No shell is used.",
                        |ui| {
                            let mut command = draft.on_complete_command.clone().unwrap_or_default();
                            ui.add(
                                widgets::input(&mut command)
                                    .hint_text("\"C:\\Program Files\\7-Zip\\7z.exe\" x {path} -o{dir}")
                                    .desired_width(ui.available_width().min(440.0)),
                            );
                            draft.on_complete_command = Some(command).filter(|c| !c.trim().is_empty());
                        },
                    );
                    if let Some(scanner) = defender_command() {
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            if widgets::button(
                                ui,
                                ButtonKind::Ghost,
                                Some(Icon::Lock),
                                "Scan with Microsoft Defender",
                                true,
                                &p,
                            )
                            .on_hover_text("Fills in the command above; click Save to use it")
                            .clicked()
                            {
                                draft.on_complete_command = Some(scanner);
                            }
                        });
                    }
                });
                self.actions_section(ui, draft);
            }
            Tab::Network => section(ui, &p, "Network", |ui| {
                row(ui, &p, "Connect timeout", "Seconds to wait for a server to accept a connection.", |ui| {
                    ui.add(egui::Slider::new(&mut draft.connect_timeout_secs, 3..=120).suffix(" s"));
                });
                row(ui, &p, "Stall timeout", "Reconnect when no data arrives for this long.", |ui| {
                    ui.add(egui::Slider::new(&mut draft.read_timeout_secs, 5..=300).suffix(" s"));
                });
                row(ui, &p, "User agent", "Some servers only serve browser-like clients.", |ui| {
                    ui.add(widgets::input(&mut draft.user_agent).desired_width(ui.available_width().min(440.0)));
                });
                row(ui, &p, "Proxy", "http://, https:// or socks5:// — empty for a direct connection.", |ui| {
                    let mut proxy = draft.proxy.clone().unwrap_or_default();
                    ui.add(
                        widgets::input(&mut proxy)
                            .hint_text("socks5://127.0.0.1:1080")
                            .desired_width(ui.available_width().min(440.0)),
                    );
                    draft.proxy = Some(proxy).filter(|s| !s.trim().is_empty());
                });
                row(
                    ui,
                    &p,
                    "HTTP/1.1 only",
                    "One TCP connection per segment — the fastest choice for segmented downloads.",
                    |ui| {
                        toggle(ui, &mut draft.http1_only, &p);
                    },
                );
                row(ui, &p, "Accept invalid certificates", "Dangerous. Only for trusted internal servers.", |ui| {
                    toggle(ui, &mut draft.insecure_tls, &p);
                    if draft.insecure_tls {
                        widgets::pill(ui, "insecure", p.red);
                    }
                });
            }),
            Tab::Scheduler => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Queues").size(14.0).family(theme::semi()).color(p.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::button(ui, ButtonKind::Primary, Some(Icon::Plus), "New queue", true, &p).clicked() {
                            self.open_queue_dialog(None);
                        }
                    });
                });
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Each queue runs its downloads in order, with its own limit and schedule. Drag downloads onto a queue in the sidebar, or use Move to queue in their menu.",
                        )
                        .size(12.5)
                        .color(p.text_dim),
                    )
                    .wrap(),
                );
                ui.add_space(5.0);
                // Queues are saved as soon as they are edited, not with the Save button.
                for q in self.engine.config().queues {
                    let count = self.snap.downloads.iter().filter(|d| d.queue == q.id).count();
                    match crate::queues::queue_card(ui, &p, &q, count) {
                        crate::queues::QueueCardAction::Edit => self.open_queue_dialog(Some(q.id)),
                        crate::queues::QueueCardAction::Delete => self.delete_queue = Some(q.id),
                        crate::queues::QueueCardAction::None => {}
                    }
                }
                ui.add_space(6.0);
                section(ui, &p, "When all downloads finish", |ui| {
                    ui.add(egui::Label::new(RichText::new(format!(
                        "For this session: {}. Change it from the toolbar's More menu. StewardGet counts down for 30 seconds first, so you can cancel.",
                        self.after_all.label().to_lowercase()
                    )).color(p.text_dim)).wrap());
                });
            }
            Tab::Folders => section(ui, &p, "Categories and folders", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new("Downloads are sorted by file extension. Add your own categories, and choose where each kind of file goes: empty means the download folder (plus a sub-folder when “Sort into category folders” is on).").size(12.5).color(p.text_dim)).wrap());
                });
                ui.add_space(6.0);
                if widgets::button(ui, ButtonKind::Primary, Some(Icon::Plus), "New category", true, &p).clicked() {
                    self.category_dialog = Some(crate::categories::CategoryDialog::create());
                }
                ui.add_space(5.0);
                for c in draft.categories() {
                    let label = draft.category_label(c);
                    row_icon(ui, &p, Icon::for_category(c), p.category(c), &label, |ui| {
                        let mut text = draft.category_dirs.get(&c).map(|d| d.display().to_string()).unwrap_or_default();
                        let (_, browse) =
                            widgets::input_with_button(ui, &mut text, "default", Icon::FolderOpen, "", &p);
                        if browse
                            && let Some(folder) =
                                rfd::FileDialog::new().set_directory(&draft.download_dir).pick_folder()
                        {
                            text = folder.display().to_string();
                        }
                        if text.trim().is_empty() {
                            draft.category_dirs.remove(&c);
                        } else {
                            // Trimmed on save; trimming here would eat the space typed in "My Videos".
                            draft.category_dirs.insert(c, text.into());
                        }
                    });
                    if let Category::Custom(id) = c {
                        let exts = draft
                            .custom_categories
                            .iter()
                            .find(|x| x.id == id)
                            .map(|x| x.extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" "))
                            .unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.add_space(36.0);
                            ui.add(egui::Label::new(RichText::new(exts).size(12.0).color(p.text_dim)).truncate());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Trash), "", true, &p)
                                    .on_hover_text("Delete category")
                                    .clicked()
                                {
                                    draft.custom_categories.retain(|x| x.id != id);
                                    draft.category_dirs.remove(&c);
                                }
                                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Edit), "Edit", true, &p).clicked()
                                    && let Some(x) = draft.custom_categories.iter().find(|x| x.id == id)
                                {
                                    self.category_dialog = Some(crate::categories::CategoryDialog::edit(x));
                                }
                            });
                        });
                    }
                }
            }),
            Tab::Browser => {
                self.browser_status(ui);
                section(ui, &p, "Install the extension", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        widgets::segmented(ui, &mut self.setup_browser, &Browser::ALL, &p);
                    });
                    ui.add_space(6.0);
                    self.browser_steps(ui);
                });
                section(ui, &p, "Browser integration", |ui| {
                    row(
                        ui,
                        &p,
                        "Take over browser downloads",
                        "Downloads started in your browser go to StewardGet. When StewardGet isn't running, the browser keeps them.",
                        |ui| {
                            toggle(ui, &mut draft.api_enabled, &p);
                        },
                    );
                    row(
                        ui,
                        &p,
                        "Port",
                        "Change it only if another program uses 7853. Set the same port in the extension's popup.",
                        |ui| {
                            ui.add(egui::DragValue::new(&mut draft.api_port).range(1024..=65535));
                        },
                    );
                    row(
                        ui,
                        &p,
                        "Access token",
                        "Optional. Without one, only the browser extension's requests (add a download, show StewardGet) are accepted; the command line uses its own key. When set, the extension must use the same token.",
                        |ui| {
                            let mut token = draft.api_token.clone().unwrap_or_default();
                            ui.add(widgets::input(&mut token).password(true).desired_width(220.0));
                            if widgets::button(ui, ButtonKind::Ghost, None, "Generate", true, &p)
                                .on_hover_text("Creates a token and copies it")
                                .clicked()
                            {
                                token = random_token();
                                ui.ctx().copy_text(token.clone());
                            }
                            draft.api_token = Some(token).filter(|t| !t.trim().is_empty());
                        },
                    );
                });
                section(ui, &p, "Refreshing expired links", |ui| {
                    ui.add(egui::Label::new(RichText::new("When a link expires mid-download, choose Refresh link. StewardGet opens the page, and the next time you click download there, it attaches the new link to your partial file and continues from where it stopped.").color(p.text)).wrap());
                });
            }
            Tab::Integrations => self.integrations_tab(ui, draft),
            Tab::About => self.about_tab(ui),
            Tab::Privacy => {
                section(ui, &p, "Privacy", |ui| {
                    ui.add(egui::Label::new(RichText::new("StewardGet has no telemetry, analytics, crash reporting or ads. Unless you turn on the update check below, the only network traffic is the downloads you start, and the browser API listens on 127.0.0.1 only.").color(p.text)).wrap());
                });
                section(ui, &p, "Updates", |ui| self.updates_section(ui, &mut draft.check_updates));
                section(ui, &p, "Your data", |ui| {
                    if let Some(dir) = self.engine.data_dir() {
                        row(
                            ui,
                            &p,
                            "Local data",
                            "Settings, download list and statistics, with automatic backups.",
                            |ui| {
                                if widgets::button(
                                    ui,
                                    ButtonKind::Ghost,
                                    Some(Icon::FolderOpen),
                                    "Open folder",
                                    true,
                                    &p,
                                )
                                .clicked()
                                {
                                    self.open_path(&dir);
                                }
                            },
                        );
                    }
                    row(ui, &p, "Export links", "Save the links of all downloads to a text file.", |ui| {
                        if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Export), "Export…", true, &p).clicked() {
                            self.selection.clear();
                            self.filter = crate::app::ListFilter::All;
                            self.export_links();
                        }
                    });
                    row(ui, &p, "Import links", "Add every link from a text file.", |ui| {
                        if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Import), "Import…", true, &p).clicked() {
                            self.import_links();
                        }
                    });
                    row(
                        ui,
                        &p,
                        "Back up settings",
                        "Save every setting, queue and category to a file, e.g. for another computer.",
                        |ui| {
                            if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Export), "Back up…", true, &p)
                                .clicked()
                            {
                                self.export_settings();
                            }
                        },
                    );
                    row(
                        ui,
                        &p,
                        "Restore settings",
                        "Load settings from a backup. They apply right away; your downloads are kept.",
                        |ui| {
                            if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Import), "Restore…", true, &p)
                                .clicked()
                                && let Some(restored) = self.import_settings(ui.ctx())
                            {
                                *draft = restored;
                            }
                        },
                    );
                });
                section(ui, &p, "Your downloads are safe", |ui| {
                    for line in [
                        "Progress is saved every 3 seconds next to each file (.vxpart / .vxmeta), so a crash or power cut loses at most a few seconds of work.",
                        "Partial data is never deleted silently. If the server's file changes, StewardGet stops and asks: resume anyway, refresh the link, or start over.",
                        "Removing a download keeps its partial data unless you choose to delete it; adding the same link again continues it.",
                        "The download list is backed up automatically; a damaged list is set aside and restored from the backup.",
                    ] {
                        ui.horizontal_top(|ui| {
                            widgets::icon(ui, Icon::Check, 14.0, p.green);
                            ui.add(egui::Label::new(RichText::new(line).color(p.text)).wrap());
                        });
                    }
                });
            }
        }
    }
}

/// A titled card grouping related settings.
pub(crate) fn section(ui: &mut Ui, p: &Palette, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.label(RichText::new(title).size(14.0).family(theme::semi()).color(p.text));
    ui.add_space(6.0);
    card(p).inner_margin(Margin::symmetric(18, 12)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        add(ui);
    });
    ui.add_space(18.0);
}

fn tab_button(ui: &mut Ui, icon: Icon, label: &str, selected: bool, p: &Palette, full_width: bool) -> egui::Response {
    let font = FontId::new(13.5, if selected { theme::semi() } else { egui::FontFamily::Proportional });
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let width = if full_width { ui.available_width() } else { galley.size().x + 46.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click());
    if selected {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            p.accent.gamma_multiply(if p.dark { 0.18 } else { 0.12 }),
        );
    } else if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), p.card_hover);
    }
    let color = if selected { p.accent } else { p.text_dim };
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.left_center() + vec2(19.0, 0.0), vec2(16.0, 16.0)),
        icon,
        color,
    );
    let fg = if selected { p.text } else { p.text_dim };
    ui.painter().galley(rect.left_center() + vec2(36.0, -galley.size().y / 2.0), galley, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The tab being collected and the (tab, title, help) rows found so far.
type Collected = Option<(Tab, Vec<(Tab, String, String)>)>;

thread_local! {
    /// While set, [`row`] only records (tab, title, help) instead of drawing: how
    /// the settings search learns what exists without a second list to maintain.
    static COLLECT: std::cell::RefCell<Collected> = const { std::cell::RefCell::new(None) };
}

/// A setting: title and help on the left, the control on the right.
pub(crate) fn row(ui: &mut Ui, p: &Palette, label: &str, help: &str, add: impl FnOnce(&mut Ui)) {
    if COLLECT.with_borrow_mut(|c| match c {
        Some((tab, found)) => {
            found.push((*tab, label.to_owned(), help.to_owned()));
            true
        }
        None => false,
    }) {
        return;
    }
    let width = ui.available_width();
    let label_w = (width * 0.5).clamp(180.0, 520.0);
    // Dividers go between rows only, never after the last one in a card.
    if ui.min_rect().height() > 4.0 {
        divider(ui, p, width);
    }
    ui.horizontal(|ui| {
        ui.set_max_width(width);
        ui.allocate_ui_with_layout(vec2(label_w, CONTROL_H), Layout::top_down(Align::Min), |ui| {
            ui.set_width(label_w);
            ui.label(RichText::new(label).color(p.text));
            if !help.is_empty() {
                ui.add(egui::Label::new(RichText::new(help).size(12.0).color(p.text_dim)).wrap());
            }
        });
        // Controls sit at the right edge of the card and keep their order: the
        // width they took last frame says how much room to leave on the left.
        let key = egui::Id::unique(("settings-row-width", label));
        let known: Option<f32> = ui.data(|d| d.get_temp(key));
        if let Some(w) = known {
            ui.add_space((ui.available_width() - w).max(0.0));
        }
        let used = ui.horizontal(add).response.rect.width();
        if known.is_none_or(|w| (w - used).abs() > 0.5) {
            ui.data_mut(|d| d.insert_temp(key, used));
            ui.ctx().request_repaint();
        }
    });
}

/// A row with a coloured icon instead of help text.
fn row_icon(ui: &mut Ui, p: &Palette, icon: Icon, color: Color32, label: &str, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.set_max_width(width);
        widgets::icon(ui, icon, 16.0, color);
        let (rect, _) = ui.allocate_exact_size(vec2(110.0, CONTROL_H), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, FontId::proportional(13.5), p.text);
        add(ui);
    });
    ui.add_space(4.0);
}

fn divider(ui: &mut Ui, p: &Palette, width: f32) {
    ui.add_space(5.0);
    let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::ZERO, p.border.gamma_multiply(0.6));
    ui.add_space(5.0);
}

/// A miniature preview of a theme.
fn theme_card(ui: &mut Ui, theme_choice: Theme, accent: Accent, selected: bool, p: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(138.0, 104.0), Sense::click());
    let preview = match theme_choice {
        Theme::System => None,
        t => Some(Palette::new(t, accent, true)),
    };
    let painter = ui.painter();
    // The preview fills the top of the card, so its top corners follow the card's.
    const RADIUS: u8 = 10;
    painter.rect_filled(rect, CornerRadius::same(RADIUS), p.card);
    let shot = egui::Rect::from_min_size(rect.min, vec2(rect.width(), 72.0));
    let paint_shot = |painter: &egui::Painter, area: egui::Rect, q: &Palette| {
        painter.rect_filled(area, CornerRadius { nw: RADIUS, ne: RADIUS, sw: 0, se: 0 }, q.bg);
        let side = egui::Rect::from_min_size(area.min, vec2(30.0, area.height()));
        painter.rect_filled(side, CornerRadius { nw: RADIUS, ..CornerRadius::ZERO }, q.sidebar);
        for i in 0..3 {
            let y = area.top() + 12.0 + i as f32 * 14.0;
            let line = egui::Rect::from_min_size(egui::pos2(area.left() + 38.0, y), vec2(area.width() - 50.0, 7.0));
            painter.rect_filled(line, CornerRadius::same(3), q.card);
            let prog = egui::Rect::from_min_size(line.min, vec2(line.width() * (0.8 - i as f32 * 0.25), 7.0));
            painter.rect_filled(
                prog,
                CornerRadius::same(3),
                if i == 0 { q.accent } else { q.accent2.gamma_multiply(0.8) },
            );
        }
        painter.circle_filled(egui::pos2(side.center().x, area.top() + 12.0), 5.0, q.accent);
    };
    let clip = painter.with_clip_rect(shot);
    match preview {
        Some(q) => paint_shot(&clip, shot, &q),
        None => {
            // Half light, half dark.
            let light = Palette::new(Theme::Light, accent, false);
            let dark = Palette::new(Theme::Dark, accent, true);
            let left = egui::Rect::from_min_max(shot.min, egui::pos2(shot.center().x, shot.bottom()));
            paint_shot(&painter.with_clip_rect(left), shot, &light);
            let right = egui::Rect::from_min_max(egui::pos2(shot.center().x, shot.top()), shot.max);
            paint_shot(&painter.with_clip_rect(right), shot, &dark);
        }
    }
    let stroke = if selected {
        Stroke::new(2.0, p.accent)
    } else if response.hovered() {
        Stroke::new(1.0, p.text_dim)
    } else {
        Stroke::new(1.0, p.border)
    };
    painter.rect_stroke(rect, CornerRadius::same(RADIUS), stroke, StrokeKind::Inside);
    let label_y = shot.bottom() + 16.0;
    let font = FontId::new(13.0, if selected { theme::semi() } else { egui::FontFamily::Proportional });
    painter.text(egui::pos2(rect.left() + 12.0, label_y), Align2::LEFT_CENTER, theme_choice.label(), font, p.text);
    if selected {
        icons::paint(
            painter,
            egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, label_y), vec2(14.0, 14.0)),
            Icon::CheckCircle,
            p.accent,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn accent_swatch(ui: &mut Ui, accent: Accent, selected: bool, p: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(92.0, 36.0), Sense::click());
    let color = Palette::new(if p.dark { Theme::Dark } else { Theme::Light }, accent, p.dark).accent;
    let painter = ui.painter();
    let bg = if selected { p.card_hover } else { p.card };
    let stroke = if selected { Stroke::new(2.0, color) } else { Stroke::new(1.0, p.border) };
    painter.rect(rect, CornerRadius::same(9), bg, stroke, StrokeKind::Inside);
    painter.circle_filled(rect.left_center() + vec2(18.0, 0.0), 8.0, color);
    painter.text(
        rect.left_center() + vec2(32.0, 0.0),
        Align2::LEFT_CENTER,
        accent.label(),
        FontId::proportional(12.5),
        p.text,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn day_chip(ui: &mut Ui, label: &str, on: bool, p: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(44.0, CONTROL_H), Sense::click());
    let (fill, fg, stroke) = if on {
        (p.accent, p.on_accent, Stroke::NONE)
    } else {
        (p.card, p.text_dim, Stroke::new(1.0, p.control_border))
    };
    ui.painter().rect(rect, CornerRadius::same(8), fill, stroke, StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Hour and minute pickers for a minute-of-day value.
pub(crate) fn time_picker(ui: &mut Ui, id: &str, minute: &mut u16) {
    let (mut h, mut m) = (*minute / 60, *minute % 60);
    ui.push_id(id, |ui| {
        ui.add(egui::DragValue::new(&mut h).range(0..=23).custom_formatter(|v, _| format!("{v:02}")));
        ui.label(":");
        ui.add(egui::DragValue::new(&mut m).range(0..=59).speed(0.5).custom_formatter(|v, _| format!("{v:02}")));
    });
    *minute = h * 60 + m;
}

/// The Microsoft Defender command-line scanner, as an "after each download"
/// command, when it is installed (Windows only).
fn defender_command() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let program_files = std::env::var_os("ProgramFiles")?;
    let exe = std::path::Path::new(&program_files).join("Windows Defender").join("MpCmdRun.exe");
    exe.is_file().then(|| format!("\"{}\" -Scan -ScanType 3 -File {{path}}", exe.display()))
}

/// A random token from the OS clock and address-space entropy (no RNG crate needed).
/// What an MCP client that starts programs needs to launch `stewardget mcp`.
fn mcp_client_json() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(if cfg!(windows) { "stewardget.exe" } else { "stewardget" })))
        .map_or("stewardget".to_owned(), |p| p.display().to_string());
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": { "stewardget": { "command": exe, "args": ["mcp"] } }
    }))
    .unwrap_or_default()
}

fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..4u8 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(i);
        h.write_u128(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out[..40].to_owned()
}

impl StewardGetApp {
    /// MCP server for AI assistants, and a pointer to the command line.
    fn integrations_tab(&mut self, ui: &mut Ui, draft: &mut Config) {
        self.media_tools_section(ui, draft);
        let p = self.palette;
        if draft.mcp_enabled && draft.mcp_token.is_none() {
            draft.mcp_token = Some(random_token());
        }
        section(ui, &p, "AI assistants (MCP)", |ui| {
            row(
                ui,
                &p,
                "MCP server",
                "Lets an AI assistant that speaks the Model Context Protocol add, check, pause, resume and remove your downloads. Off by default.",
                |ui| {
                    toggle(ui, &mut draft.mcp_enabled, &p);
                },
            );
            if draft.mcp_enabled {
                let url = format!("http://127.0.0.1:{}/mcp", draft.api_port);
                row(ui, &p, "Address", "Only this computer can reach it.", |ui| {
                    ui.label(RichText::new(&url).monospace().color(p.text));
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy", true, &p).clicked() {
                        ui.ctx().copy_text(url.clone());
                    }
                });
                let token = draft.mcp_token.clone().unwrap_or_default();
                row(ui, &p, "Token", "Clients send it as \u{201c}Authorization: Bearer\u{201d}.", |ui| {
                    let mut shown = token.clone();
                    ui.add(widgets::input(&mut shown).password(true).desired_width(120.0));
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy", true, &p).clicked() {
                        ui.ctx().copy_text(token.clone());
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Refresh), "Renew", true, &p)
                        .on_hover_text("Create a new token; clients need the new one")
                        .clicked()
                    {
                        draft.mcp_token = Some(random_token());
                    }
                });
                row(
                    ui,
                    &p,
                    "Claude Desktop and others",
                    "For clients that start a program. Paste this into the client's MCP settings.",
                    |ui| {
                        if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy settings", true, &p).clicked()
                        {
                            ui.ctx().copy_text(mcp_client_json());
                        }
                    },
                );
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        RichText::new("Click Save to apply. The assistant can manage downloads inside StewardGet only: new files go below your download folder, and it cannot change settings, run programs or read files.")
                            .size(12.5)
                            .color(p.text_dim),
                    )
                    .wrap(),
                );
            }
        });
        section(ui, &p, "Command line", |ui| {
            let text = "stewardget list --json\nstewardget pause all  |  stewardget resume 4\nstewardget limit 2M\nstewardget mcp --config   (settings for an MCP client)\nstewardget completions powershell";
            ui.label(RichText::new(text).monospace().size(12.5).color(p.text));
        });
    }

    /// Version, data locations and licence.
    fn about_tab(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let version = env!("CARGO_PKG_VERSION");
        let dir = self.engine.data_dir();
        section(ui, &p, "StewardGet", |ui| {
            row(ui, &p, "Version", "", |ui| {
                ui.label(RichText::new(version).family(theme::semi()).color(p.text));
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy details", true, &p).clicked() {
                    let details = format!(
                        "StewardGet {version}\n{} {}\ndata: {}",
                        std::env::consts::OS,
                        std::env::consts::ARCH,
                        dir.as_ref().map_or("(memory)".to_owned(), |d| d.display().to_string())
                    );
                    ui.ctx().copy_text(details);
                }
            });
            row(ui, &p, "Browser extension", "", |ui| {
                let text = match &self.snap.extension {
                    Some(x) => format!("{} {}", x.browser, x.version),
                    None => "not connected".to_owned(),
                };
                ui.label(RichText::new(text).color(p.text));
            });
            if let Some(dir) = dir {
                row(ui, &p, "Data folder", &dir.display().to_string(), |ui| {
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Open", true, &p).clicked() {
                        self.open_path(&dir);
                    }
                });
            }
            row(ui, &p, "Licence", "Free software, no telemetry, no accounts.", |ui| {
                ui.label(RichText::new("MIT or Apache-2.0").color(p.text));
            });
        });
    }

    /// Whether the browser can reach StewardGet, and what to do if not.
    fn browser_status(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let app_version = env!("CARGO_PKG_VERSION");
        let (color, headline, detail): (Color32, String, String) = match (&self.snap.api, &self.snap.extension) {
            (ApiState::Disabled, _) => (
                p.yellow,
                "Browser integration is off".into(),
                "Turn on \u{201c}Take over browser downloads\u{201d} below and click Save.".into(),
            ),
            (ApiState::Failed(e), _) => (
                p.red,
                "The browser can't reach StewardGet".into(),
                format!(
                    "StewardGet could not listen on {e}. Another program probably uses this port: choose a different port below, click Save, and enter the same port in the extension's popup."
                ),
            ),
            (ApiState::Listening(_), Some(x)) if x.seen_secs < 150 => {
                let name = if x.browser == "Browser" { "your browser".to_owned() } else { x.browser.clone() };
                let detail = if x.version != app_version {
                    "The extension is older than the app. Restart the browser, or click reload on StewardGet in its extensions page, to update it."
                } else {
                    "Downloads you start in the browser now go to StewardGet. Right-click a link for \u{201c}Download with StewardGet\u{201d}."
                };
                (p.green, format!("Connected to {name}"), detail.to_owned())
            }
            (ApiState::Listening(_), Some(x)) => (
                p.text_dim,
                format!("Last connected {} ago", format_duration(x.seen_secs)),
                "The browser is probably closed. The extension reconnects within a minute of opening it.".into(),
            ),
            (ApiState::Listening(_), None) => (
                p.yellow,
                "Waiting for the browser extension".into(),
                "Install it with the steps below. This turns green within a minute of the extension connecting.".into(),
            ),
        };
        section(ui, &p, "Connection", |ui| {
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, color);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(headline).family(theme::semi()).color(p.text));
                    ui.add(egui::Label::new(RichText::new(detail).size(12.5).color(p.text_dim)).wrap());
                });
            });
        });
    }

    /// Numbered install steps for the chosen browser.
    fn browser_steps(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let browser = self.setup_browser;
        let folder = self.extension_dir.clone().map(|dir| match browser {
            Browser::Firefox => stewardget_core::extension::firefox_folder(&dir),
            _ => dir,
        });
        let mut copied: Option<&'static str> = None;

        let folder_buttons = |ui: &mut Ui, copied: &mut Option<&'static str>| match &folder {
            Ok(dir) => {
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(RichText::new(dir.display().to_string()).size(12.5).monospace().color(p.text_dim))
                        .wrap(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy folder path", true, &p).clicked()
                    {
                        ui.ctx().copy_text(dir.display().to_string());
                        *copied = Some("Folder path copied");
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Show folder", true, &p).clicked()
                    {
                        let _ = stewardget_core::util::open_with_system(dir);
                    }
                });
            }
            Err(e) => {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("The extension folder could not be prepared: {e}")).color(p.red),
                    )
                    .wrap(),
                );
            }
        };
        let page_button = |ui: &mut Ui, copied: &mut Option<&'static str>| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(browser.page()).monospace().color(p.text_dim));
                if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Copy), "Copy address", true, &p)
                    .on_hover_text("Browsers don't let other apps open this page. Paste it into the address bar.")
                    .clicked()
                {
                    ui.ctx().copy_text(browser.page().to_owned());
                    *copied = Some("Address copied. Paste it into the browser's address bar.");
                }
            });
        };

        let mut n = 0;
        if browser == Browser::Firefox {
            step(
                ui,
                &p,
                &mut n,
                "Permanent install: open the StewardGet release page and click stewardget-firefox.xpi. Firefox asks whether to add StewardGet; click Add.",
                |ui| {
                    ui.add_space(4.0);
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::External), "Open release page", true, &p)
                        .clicked()
                    {
                        let _ = stewardget_core::util::open_url(RELEASES);
                    }
                },
            );
            step(
                ui,
                &p,
                &mut n,
                "No .xpi on the release page? Load it for this session instead. Open this page in Firefox:",
                |ui| page_button(ui, &mut copied),
            );
            step(
                ui,
                &p,
                &mut n,
                "Click \u{201c}Load Temporary Add-on\u{2026}\u{201d} and choose manifest.json in this folder. Firefox removes temporary add-ons when it closes.",
                |ui| folder_buttons(ui, &mut copied),
            );
            step(
                ui,
                &p,
                &mut n,
                "Open about:addons, choose StewardGet, and on the Permissions tab allow access to all websites. The extension needs it to hand downloads over.",
                |_| {},
            );
        } else {
            step(ui, &p, &mut n, "Open the extensions page in the browser:", |ui| page_button(ui, &mut copied));
            step(ui, &p, &mut n, browser.developer_mode(), |_| {});
            step(ui, &p, &mut n, "Click \u{201c}Load unpacked\u{201d} and choose this folder:", |ui| {
                folder_buttons(ui, &mut copied)
            });
            step(
                ui,
                &p,
                &mut n,
                "Done. Pin StewardGet to the toolbar if you like. The status above turns green once the extension connects.",
                |_| {},
            );
        }
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Keep this folder where it is: the browser loads the extension from it, and StewardGet updates it when the app updates.",
                )
                .size(12.0)
                .color(p.text_dim),
            )
            .wrap(),
        );
        if let Some(text) = copied {
            let green = p.green;
            self.toast(text, green);
        }
    }
}

/// One numbered instruction, with optional controls under it.
fn step(ui: &mut Ui, p: &Palette, n: &mut usize, text: &str, add: impl FnOnce(&mut Ui)) {
    *n += 1;
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 11.0, p.accent.gamma_multiply(if p.dark { 0.2 } else { 0.12 }));
        ui.painter().text(r.center(), Align2::CENTER_CENTER, n.to_string(), FontId::new(12.0, theme::semi()), p.accent);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(text).color(p.text)).wrap());
            add(ui);
        });
    });
    ui.add_space(5.0);
}
