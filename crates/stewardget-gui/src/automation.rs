//! Settings for the things StewardGet does around a download: which network
//! links it may use, and what happens to a file once it has arrived.

use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use stewardget_core::{Adapter, Category, Config, NetworkLink, PostAction, PostRule, adapters};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::settings::{row, section};
use crate::widgets::{self, ButtonKind, toggle};

/// Adapters, re-read at most every few seconds (it is a system call).
fn present_adapters(ui: &Ui) -> Vec<Adapter> {
    let id = egui::Id::unique("stewardget-adapters");
    let cached: Option<(Instant, Vec<Adapter>)> = ui.data(|d| d.get_temp(id));
    if let Some((at, list)) = cached
        && at.elapsed() < Duration::from_secs(3)
    {
        return list;
    }
    let list = adapters();
    ui.data_mut(|d| d.insert_temp(id, (Instant::now(), list.clone())));
    list
}

/// A comma separated list in a text field that survives half-typed input.
fn list_input(ui: &mut Ui, id: egui::Id, values: &mut Vec<String>, hint: &str, width: f32) {
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| values.join(", "));
    let r = ui.add(widgets::input(&mut text).hint_text(hint).desired_width(width));
    if r.changed() {
        *values = text.split([',', ' ', ';']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
}

impl StewardGetApp {
    /// "Use several connections": fastest-link bonding and the tuner.
    pub fn links_section(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        let found = present_adapters(ui);
        section(ui, &p, "Speed: use more of your connection", |ui| {
            row(
                ui,
                &p,
                "Auto-tune connections",
                "Start with a few connections per file and add more while the speed keeps improving, up to the number set under Downloads. A file with its own connection count is left alone.",
                |ui| {
                    toggle(ui, &mut draft.auto_tune, &p);
                },
            );
            row(
                ui,
                &p,
                "Download over several networks at once",
                "Spread each file over Wi-Fi, Ethernet, a phone hotspot… at the same time. Each network carries as much as it can; one that drops out is skipped without stopping the download.",
                |ui| {
                    toggle(ui, &mut draft.multi_link, &p);
                },
            );
            if !draft.multi_link {
                return;
            }
            ui.add_space(4.0);
            if found.is_empty() {
                ui.label(RichText::new("No network adapters were found.").color(p.text_dim));
            }
            let live = self.engine.links();
            for adapter in &found {
                let ip = adapter.address.to_string();
                let idx = draft.interfaces.iter().position(|l| l.address == ip);
                let mut enabled = idx.is_some_and(|i| draft.interfaces[i].enabled);
                let mut limit = idx.map_or(0, |i| draft.interfaces[i].limit);
                let speed = live.iter().find(|l| l.address.as_deref() == Some(ip.as_str())).map(|l| l.speed);
                row(ui, &p, &adapter.name, &ip, |ui| {
                    if let Some(speed) = speed.filter(|&s| s > 1.0) {
                        ui.label(RichText::new(stewardget_core::util::format_speed(speed)).size(12.0).color(p.accent2));
                    }
                    ui.label(RichText::new("limit").size(12.0).color(p.text_dim));
                    widgets::speed_input(ui, &format!("link-{ip}"), &mut limit, 90.0);
                    toggle(ui, &mut enabled, &p);
                });
                let entry = NetworkLink { address: ip, name: adapter.name.clone(), enabled, limit };
                match idx {
                    Some(i) => {
                        let old = &mut draft.interfaces[i];
                        if old.enabled != enabled || old.limit != limit || old.name != entry.name {
                            *old = entry;
                        }
                    }
                    None if enabled || limit > 0 => draft.interfaces.push(entry),
                    None => {}
                }
            }
            let enabled = draft.interfaces.iter().filter(|l| l.enabled).count();
            let msg = match enabled {
                0 => "No network is switched on: downloads use your default connection.",
                1 => "One network is switched on, so downloads go through it only. Switch on another to combine them.",
                _ => {
                    "Downloads are spread over every switched-on network that is connected. Mirrors multiply this: each mirror is tried on each network."
                }
            };
            ui.add_space(4.0);
            ui.label(RichText::new(msg).size(12.0).color(p.text_dim));
        });
    }

    /// Where yt-dlp and ffmpeg are, and whether they were found.
    pub fn media_tools_section(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        let (ytdlp, ffmpeg) = self.engine.media_tools();
        section(ui, &p, "Video and audio tools", |ui| {
            for (name, found, path, help) in [
                (
                    "yt-dlp",
                    ytdlp,
                    &mut draft.ytdlp_path,
                    "Finds the video and audio files behind a page (Tools › Video or audio from a page). StewardGet doesn't include it.",
                ),
                (
                    "ffmpeg",
                    ffmpeg,
                    &mut draft.ffmpeg_path,
                    "Joins separate picture and sound files into one video, without re-encoding.",
                ),
            ] {
                row(ui, &p, name, help, |ui| {
                    match &found {
                        Some(f) => widgets::pill(ui, &format!("found: {}", f.display()), p.green),
                        None => widgets::pill(ui, "not found", p.yellow),
                    };
                    let mut text = path.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
                    ui.add(widgets::input(&mut text).hint_text("location (optional)").desired_width(220.0));
                    *path = Some(std::path::PathBuf::from(text.trim())).filter(|d| !d.as_os_str().is_empty());
                });
            }
        });
    }

    /// Rules that run on each finished file.
    pub fn actions_section(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        section(ui, &p, "Steps for each finished file", |ui| {
            ui.label(
                RichText::new(
                    "Steps run in order on each finished file, in the background. A malware scan that finds something stops the steps after it.",
                )
                .size(12.0)
                .color(p.text_dim),
            );
            ui.add_space(4.0);
            let mut remove = None;
            let mut up = None;
            let count = draft.post_actions.len();
            for i in 0..count {
                let rule = &mut draft.post_actions[i];
                ui.push_id(("post-rule", i), |ui| {
                    egui::Frame::new()
                        .fill(p.card_hover.gamma_multiply(0.5))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                toggle(ui, &mut rule.enabled, &p);
                                action_picker(ui, i, &mut rule.action);
                                ui.add(
                                    widgets::input(&mut rule.name).hint_text(rule.action.label()).desired_width(150.0),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if widgets::icon_button(ui, Icon::Trash, "Remove step", &p).clicked() {
                                        remove = Some(i);
                                    }
                                    if i > 0 && widgets::icon_button(ui, Icon::ArrowUp, "Run earlier", &p).clicked() {
                                        up = Some(i);
                                    }
                                });
                            });
                            action_params(ui, &p, rule);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("Only for").size(12.0).color(p.text_dim));
                                for cat in Category::ALL {
                                    let on = rule.categories.contains(&cat);
                                    if widgets::chip(ui, cat.label(), on, &p).clicked() {
                                        if on {
                                            rule.categories.retain(|c| *c != cat);
                                        } else {
                                            rule.categories.push(cat);
                                        }
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Extensions").size(12.0).color(p.text_dim));
                                list_input(
                                    ui,
                                    egui::Id::unique(("rule-ext", i)),
                                    &mut rule.extensions,
                                    "zip, iso (any)",
                                    130.0,
                                );
                                ui.label(RichText::new("Sites").size(12.0).color(p.text_dim));
                                list_input(
                                    ui,
                                    egui::Id::unique(("rule-host", i)),
                                    &mut rule.hosts,
                                    "*.github.com (any)",
                                    150.0,
                                );
                            });
                        });
                });
                ui.add_space(4.0);
            }
            if let Some(i) = remove {
                draft.post_actions.remove(i);
            }
            if let Some(i) = up {
                draft.post_actions.swap(i, i - 1);
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Add a step:").color(p.text_dim));
                let presets: [(&str, PostRule); 6] = [
                    (
                        "Sort by kind and month",
                        PostRule {
                            name: "Sort".into(),
                            action: PostAction::MoveTo {
                                dir: draft.download_dir.join("{category}").join("{year}-{month}"),
                            },
                            ..Default::default()
                        },
                    ),
                    (
                        "Rename",
                        PostRule {
                            name: "Rename".into(),
                            action: PostAction::Rename { template: "{name} ({host})".into() },
                            ..Default::default()
                        },
                    ),
                    (
                        "Scan for malware",
                        PostRule { name: "Scan".into(), action: PostAction::Scan, ..Default::default() },
                    ),
                    (
                        "Unpack archives",
                        PostRule {
                            name: "Unpack".into(),
                            categories: vec![Category::Archive],
                            extensions: vec!["zip".into(), "tar".into(), "tgz".into(), "gz".into()],
                            action: PostAction::Extract { delete_archive: false },
                            ..Default::default()
                        },
                    ),
                    (
                        "Move to a folder",
                        PostRule {
                            name: "Move".into(),
                            action: PostAction::MoveTo { dir: draft.download_dir.join("Sorted") },
                            ..Default::default()
                        },
                    ),
                    (
                        "Tag",
                        PostRule {
                            name: "Tag".into(),
                            action: PostAction::Tag { tag: "new".into() },
                            ..Default::default()
                        },
                    ),
                ];
                for (label, rule) in presets {
                    if widgets::button(ui, ButtonKind::Tool, Some(Icon::Plus), label, true, &p).clicked() {
                        draft.post_actions.push(rule);
                    }
                }
            });
        });
    }
}

fn kind(a: &PostAction) -> usize {
    match a {
        PostAction::Scan => 0,
        PostAction::Extract { .. } => 1,
        PostAction::MoveTo { .. } => 2,
        PostAction::Command { .. } => 3,
        PostAction::Tag { .. } => 4,
        PostAction::Rename { .. } => 5,
    }
}

fn action_picker(ui: &mut Ui, i: usize, action: &mut PostAction) {
    let current = kind(action);
    let names = ["Scan for malware", "Unpack", "Move to folder", "Run program", "Tag", "Rename"];
    let mut chosen = current;
    egui::ComboBox::from_id_salt(("post-kind", i)).width(140.0).selected_text(names[current]).show_ui(ui, |ui| {
        for (k, name) in names.iter().enumerate() {
            ui.selectable_value(&mut chosen, k, *name);
        }
    });
    if chosen != current {
        *action = match chosen {
            0 => PostAction::Scan,
            1 => PostAction::Extract { delete_archive: false },
            2 => PostAction::MoveTo { dir: Default::default() },
            3 => PostAction::Command { command: String::new() },
            4 => PostAction::Tag { tag: String::new() },
            _ => PostAction::Rename { template: "{name}".into() },
        };
    }
}

fn action_params(ui: &mut Ui, p: &crate::theme::Palette, rule: &mut PostRule) {
    match &mut rule.action {
        PostAction::Scan => {
            ui.label(
                RichText::new("Uses Microsoft Defender on Windows, ClamAV elsewhere. The file stays where it is.")
                    .size(12.0)
                    .color(p.text_dim),
            );
        }
        PostAction::Extract { delete_archive } => {
            ui.horizontal(|ui| {
                toggle(ui, delete_archive, p);
                ui.label(RichText::new("Delete the archive afterwards").size(12.5).color(p.text));
                ui.label(RichText::new(".zip, .tar, .tar.gz and .tgz").size(12.0).color(p.text_dim));
            });
        }
        PostAction::MoveTo { dir } => {
            let mut text = dir.display().to_string();
            ui.horizontal(|ui| {
                let (_, browse) = widgets::input_with_button(ui, &mut text, "Folder", Icon::FolderOpen, "Browse", p);
                if browse && let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    text = folder.display().to_string();
                }
            });
            *dir = text.into();
            ui.label(
                RichText::new("Folder names can use {category} {year} {month} {day} {host}.")
                    .size(12.0)
                    .color(p.text_dim),
            );
        }
        PostAction::Rename { template } => {
            ui.add(widgets::input(template).hint_text("{host}-{name}").desired_width(260.0));
            ui.label(
                RichText::new("{name} {ext} {host} {year} {month} {day}. The extension is kept.")
                    .size(12.0)
                    .color(p.text_dim),
            );
        }
        PostAction::Command { command } => {
            ui.add(
                widgets::input(command)
                    .hint_text("\"C:\\Tools\\convert.exe\" {path}")
                    .desired_width(ui.available_width().min(460.0)),
            );
            ui.label(
                RichText::new("{path} {dir} {name} {url} are filled in. No shell is used.")
                    .size(12.0)
                    .color(p.text_dim),
            );
        }
        PostAction::Tag { tag } => {
            ui.add(widgets::input(tag).hint_text("tag").desired_width(180.0));
        }
    }
}
