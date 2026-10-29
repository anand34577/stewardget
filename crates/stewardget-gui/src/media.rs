//! Download video or audio from a page (through yt-dlp) or a DASH manifest.

use std::sync::{Arc, Mutex};

use egui::RichText;
use stewardget_core::MediaInfo;
use stewardget_core::util::format_bytes;

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::widgets::{self, ButtonKind};

const LABEL_W: f32 = 110.0;

enum Phase {
    Idle,
    Looking,
    Found(Result<MediaInfo, String>),
}

pub struct MediaDialog {
    url: String,
    info: Option<MediaInfo>,
    choice: usize,
    dir: String,
    phase: Arc<Mutex<Phase>>,
    task: Option<tokio::task::JoinHandle<()>>,
    message: Option<String>,
    focus: bool,
}

impl MediaDialog {
    pub fn new(url: Option<String>, dir: String) -> Self {
        Self {
            url: url.unwrap_or_default(),
            info: None,
            choice: 0,
            dir,
            phase: Arc::new(Mutex::new(Phase::Idle)),
            task: None,
            message: None,
            focus: true,
        }
    }
}

impl Drop for MediaDialog {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl StewardGetApp {
    pub fn media_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.media_dialog.take() else { return };
        let p = self.palette;
        let (ytdlp, ffmpeg) = self.engine.media_tools();

        let done = {
            let mut phase = d.phase.lock().unwrap();
            matches!(&*phase, Phase::Found(_)).then(|| std::mem::replace(&mut *phase, Phase::Idle))
        };
        if let Some(Phase::Found(result)) = done {
            d.task = None;
            match result {
                Ok(info) => {
                    d.choice = 0;
                    d.message = info.options.is_empty().then(|| "Nothing downloadable was found there.".to_owned());
                    d.info = Some(info);
                }
                Err(e) => {
                    d.info = None;
                    d.message = Some(e);
                }
            }
        }

        let looking = matches!(*d.phase.lock().unwrap(), Phase::Looking);
        let mut close = false;
        let mut start = false;
        let mut download = false;
        let modal = crate::dialogs::modal("stewardget-media", &p).show(ctx, |ui| {
            ui.set_width(560.0);
            crate::dialogs::title(ui, &p, Icon::Video, "Video and audio from a page");
            widgets::form_row(ui, LABEL_W, "Page address", &p, |ui| {
                let r = ui.add_enabled(
                    !looking,
                    widgets::input(&mut d.url).hint_text("page with a video, or a .mpd manifest").desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut d.focus) {
                    r.request_focus();
                }
            });
            if ytdlp.is_none() {
                crate::dialogs::note(
                    ui,
                    "yt-dlp isn't installed, so only DASH manifests (.mpd) can be read. For video pages, install it (for example: winget install yt-dlp) or set its location in Settings › AI & automation.",
                    p.yellow,
                );
            }
            if let Some(info) = &d.info
                && !info.options.is_empty()
            {
                ui.add_space(6.0);
                ui.label(RichText::new(&info.title).family(crate::theme::semi()).color(p.text));
                ui.label(RichText::new(format!("Found with {}", info.source)).size(12.0).color(p.text_dim));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (i, option) in info.options.iter().enumerate() {
                        let size = option.size().map(|s| format!("  ·  about {}", format_bytes(s))).unwrap_or_default();
                        let blocked = option.needs_merge() && ffmpeg.is_none();
                        ui.add_enabled_ui(!blocked, |ui| {
                            ui.radio_value(&mut d.choice, i, format!("{}{size}", option.label));
                        });
                        if blocked && i == d.choice {
                            d.choice = info.options.iter().position(|o| !o.needs_merge()).unwrap_or(0);
                        }
                    }
                });
                if ffmpeg.is_none() && info.options.iter().any(|o| o.needs_merge()) {
                    crate::dialogs::note(
                        ui,
                        "The sharpest choices keep picture and sound in separate files and need ffmpeg to join them. It isn't installed (winget install ffmpeg).",
                        p.text_dim,
                    );
                }
                ui.add_space(4.0);
                widgets::form_row(ui, LABEL_W, "Save to", &p, |ui| {
                    ui.add(widgets::input(&mut d.dir).desired_width(f32::INFINITY));
                });
            }
            if let Some(m) = &d.message {
                ui.add_space(4.0);
                crate::dialogs::note(ui, m.as_str(), p.yellow);
            }
            if looking {
                crate::dialogs::note(ui, "Asking the page what it offers\u{2026} this can take a few seconds.", p.text_dim);
            }
            crate::dialogs::footer(ui, |ui| {
                if d.info.as_ref().is_some_and(|i| !i.options.is_empty()) {
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Download), "Download", true, &p).clicked() {
                        download = true;
                    }
                    if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Search), "Look again", !looking, &p).clicked() {
                        start = true;
                    }
                } else {
                    let can = !looking && !d.url.trim().is_empty();
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Search), "Find", can, &p).clicked()
                        || (can && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        start = true;
                    }
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
        if download && let Some(info) = &d.info {
            let dir = std::path::PathBuf::from(d.dir.trim());
            let dir = (!d.dir.trim().is_empty()).then_some(dir);
            match self.engine.add_media(info, d.choice, dir, None) {
                Ok(_) => {
                    let green = p.green;
                    self.toast(format!("Downloading “{}”", info.title), green);
                    self.page = crate::app::Page::Downloads;
                    return;
                }
                Err(e) => d.message = Some(e.to_string()),
            }
        }
        if start {
            let mut url = d.url.trim().to_owned();
            if !url.contains("://") {
                url = format!("https://{url}");
            }
            d.message = None;
            d.info = None;
            *d.phase.lock().unwrap() = Phase::Looking;
            let (engine, phase, ctx2) = (self.engine.clone(), Arc::clone(&d.phase), ctx.clone());
            d.task = Some(self.runtime.spawn(async move {
                let result = engine.inspect_media(&url).await.map_err(|e| e.to_string());
                *phase.lock().unwrap() = Phase::Found(result);
                ctx2.request_repaint();
            }));
        }
        if looking {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        self.media_dialog = Some(d);
    }
}
