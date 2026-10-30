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

    /// yt-dlp and ffmpeg: found automatically, or picked with a file chooser,
    /// or installed with one click; typing a path stays available for experts.
    pub fn media_tools_section(&mut self, ui: &mut Ui, draft: &mut Config) {
        let p = self.palette;
        section(ui, &p, "Video and audio tools", |ui| {
            ui.label(
                RichText::new(
                    "StewardGet finds these programs by itself. If they are missing, install them with one click, or point to a copy you already have.",
                )
                .size(12.0)
                .color(p.text_dim),
            );
            tool_block(
                ui,
                &p,
                &ToolInfo {
                    name: "yt-dlp",
                    help: "Finds the video and audio files behind a page (File › Video or audio from a page). StewardGet doesn't include it.",
                    winget: "yt-dlp.yt-dlp",
                    page: "https://github.com/yt-dlp/yt-dlp#installation",
                },
                &mut draft.ytdlp_path,
            );
            tool_block(
                ui,
                &p,
                &ToolInfo {
                    name: "ffmpeg",
                    help: "Joins separate picture and sound files into one video, without re-encoding.",
                    winget: "Gyan.FFmpeg",
                    page: "https://ffmpeg.org/download.html",
                },
                &mut draft.ffmpeg_path,
            );
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
                        .corner_radius(egui::CornerRadius::same(crate::theme::radius::MD))
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            let (enabled, action, name) = (&mut rule.enabled, &mut rule.action, &mut rule.name);
                            widgets::split_row(
                                ui,
                                |ui| {
                                    toggle(ui, enabled, &p);
                                    action_picker(ui, i, action, &p);
                                    ui.add(widgets::input(name).hint_text(action.label()).desired_width(150.0));
                                },
                                |ui| {
                                    if widgets::icon_button(ui, Icon::Trash, "Remove step", &p).clicked() {
                                        remove = Some(i);
                                    }
                                    if i > 0 && widgets::icon_button(ui, Icon::ArrowUp, "Run earlier", &p).clicked() {
                                        up = Some(i);
                                    }
                                },
                            );
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
                            ui.horizontal_wrapped(|ui| {
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

fn action_picker(ui: &mut Ui, i: usize, action: &mut PostAction, p: &crate::theme::Palette) {
    let current = kind(action);
    let names = ["Scan for malware", "Unpack", "Move to folder", "Run program", "Tag", "Rename"];
    let mut chosen = current;
    widgets::dropdown(p, ("post-kind", i)).width(140.0).selected_text(names[current]).show_ui(ui, |ui| {
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
            ui.horizontal_wrapped(|ui| {
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

struct ToolInfo {
    name: &'static str,
    help: &'static str,
    /// Package id for the Windows Package Manager.
    winget: &'static str,
    page: &'static str,
}

type Check = std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>;
type Install = std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>;

/// The version check of `path`, started once per path and remembered.
fn checked(ui: &Ui, name: &str, path: &std::path::Path) -> Option<Result<String, String>> {
    let id = egui::Id::unique(("tool-check", name, path.to_string_lossy().into_owned()));
    let slot: Option<Check> = ui.data(|d| d.get_temp(id));
    let slot = match slot {
        Some(s) => s,
        None => {
            let slot: Check = Check::default();
            ui.data_mut(|d| d.insert_temp(id, std::sync::Arc::clone(&slot)));
            let (out, path, name, ctx) =
                (std::sync::Arc::clone(&slot), path.to_path_buf(), name.to_owned(), ui.ctx().clone());
            std::thread::spawn(move || {
                *out.lock().unwrap() = Some(stewardget_core::tool_version(&path, &name));
                ctx.request_repaint();
            });
            slot
        }
    };
    let result = slot.lock().unwrap().clone();
    if result.is_none() {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
    result
}

fn tool_block(ui: &mut Ui, p: &crate::theme::Palette, tool: &ToolInfo, chosen: &mut Option<std::path::PathBuf>) {
    let found = stewardget_core::find_tool(chosen.as_deref(), tool.name);
    let version = found.as_deref().and_then(|f| checked(ui, tool.name, f));
    let install_id = egui::Id::unique(("tool-install", tool.name));
    let install: Option<Install> = ui.data(|d| d.get_temp(install_id));
    let installing = install.as_ref().is_some_and(|i| i.lock().unwrap().is_none());
    let can_winget = cfg!(windows) && stewardget_core::find_tool(None, "winget").is_some();

    row(ui, p, tool.name, tool.help, |ui| {
        match (&found, &version) {
            (None, _) => widgets::pill(ui, "not found", p.yellow),
            (Some(_), None) => widgets::pill(ui, "checking…", p.text_dim),
            (Some(_), Some(Ok(v))) => widgets::pill(ui, &format!("ready · {v}"), p.green),
            (Some(_), Some(Err(_))) => widgets::pill(ui, "not working", p.red),
        };
        if widgets::button(ui, ButtonKind::Ghost, Some(Icon::FolderOpen), "Choose file…", !installing, p)
            .on_hover_text("Pick the program on your computer")
            .clicked()
        {
            let mut dialog = rfd::FileDialog::new().set_title(format!("Select {}", tool.name));
            if cfg!(windows) {
                dialog = dialog.add_filter("Program", &["exe"]);
            }
            if let Some(file) = dialog.pick_file() {
                *chosen = Some(file);
            }
        }
        if found.is_none() {
            let label = if can_winget { "Install for me" } else { "Get it…" };
            if widgets::button(ui, ButtonKind::Primary, Some(Icon::Download), label, !installing, p).clicked() {
                if can_winget {
                    let state: Install = Install::default();
                    ui.data_mut(|d| d.insert_temp(install_id, std::sync::Arc::clone(&state)));
                    let (out, id, ctx) = (state, tool.winget, ui.ctx().clone());
                    std::thread::spawn(move || {
                        *out.lock().unwrap() = Some(winget_install(id));
                        ctx.request_repaint();
                    });
                } else {
                    let _ = stewardget_core::util::open_url(tool.page);
                }
            }
        }
    });

    // What happened, in plain words.
    let mut note: Option<(String, egui::Color32)> = None;
    if installing {
        ctx_repaint(ui);
        note = Some((
            format!("Installing {} with the Windows Package Manager… approve any prompt that appears.", tool.name),
            p.accent2,
        ));
    } else if let Some(Some(result)) = install.as_ref().map(|i| i.lock().unwrap().clone()) {
        note = Some(match result {
            Ok(()) if found.is_some() => (format!("{} was installed and found.", tool.name), p.green),
            Ok(()) => (
                format!(
                    "{} was installed, but isn't visible to StewardGet yet. Restart StewardGet, or choose the file.",
                    tool.name
                ),
                p.yellow,
            ),
            Err(e) => (
                format!("The installation didn't work ({e}). You can download it yourself and choose the file."),
                p.red,
            ),
        });
    } else if let (Some(f), Some(Err(why))) = (&found, &version) {
        note = Some((format!("{} {}", f.display(), why), p.red));
    } else if let Some(f) = &found {
        let how = if chosen
            .as_deref()
            .is_some_and(|c| stewardget_core::find_tool(Some(c), tool.name).as_deref() == Some(f.as_path()))
        {
            "your choice"
        } else {
            "found automatically"
        };
        note = Some((format!("{}  ·  {how}", f.display()), p.text_dim));
    } else if let Some(c) = chosen.as_ref() {
        note = Some((format!("Nothing usable at {}. Choose the program file, or its folder.", c.display()), p.red));
    }
    if let Some((text, color)) = note {
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap());
    }

    // For people who know where it is.
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Location (advanced)").size(12.0).color(p.text_dim));
        let mut text = chosen.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
        let r = ui.add(widgets::input(&mut text).hint_text("automatic").desired_width(300.0));
        if r.changed() {
            *chosen = Some(std::path::PathBuf::from(text.trim())).filter(|d| !d.as_os_str().is_empty());
        }
        if chosen.is_some()
            && widgets::button(ui, ButtonKind::Tool, Some(Icon::Retry), "Use automatic", true, p).clicked()
        {
            *chosen = None;
        }
    });
    ui.add_space(4.0);
}

fn ctx_repaint(ui: &Ui) {
    ui.ctx().request_repaint_after(Duration::from_millis(500));
}

fn winget_install(id: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new("winget");
    cmd.args(["install", "-e", "--id", id, "--accept-source-agreements", "--accept-package-agreements"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&out.stdout);
        let last = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("winget failed");
        Err(last.chars().take(120).collect())
    }
}
