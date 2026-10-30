//! The optional update check: off by default, one request to the release page.

use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};
use stewardget_core::update::{self, Release};

use crate::app::StewardGetApp;
use crate::icons::Icon;
use crate::settings::row;
use crate::widgets::{self, ButtonKind};

type Slot = Arc<Mutex<Option<Result<Release, String>>>>;

#[derive(Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Failed(String),
}

#[derive(Default)]
pub struct Updates {
    pub state: UpdateState,
    slot: Slot,
    /// The automatic check on startup was already made this session.
    auto_done: bool,
    /// The result of the check in flight should be announced with a toast.
    announce: bool,
}

impl StewardGetApp {
    /// Starts a check; `announce` also tells the user about a new version.
    pub fn check_for_updates(&mut self, ctx: &egui::Context, announce: bool) {
        if matches!(self.updates.state, UpdateState::Checking) {
            return;
        }
        self.updates.state = UpdateState::Checking;
        self.updates.announce = announce;
        let (slot, ctx) = (self.updates.slot.clone(), ctx.clone());
        self.runtime.spawn(async move {
            let result = update::latest().await;
            *slot.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// Runs the startup check when the user allowed it, and picks up finished checks.
    pub fn poll_updates(&mut self, ctx: &egui::Context) {
        if !self.updates.auto_done && self.engine.config().check_updates {
            self.updates.auto_done = true;
            self.check_for_updates(ctx, true);
        }
        let Some(result) = self.updates.slot.lock().unwrap().take() else { return };
        let announce = std::mem::take(&mut self.updates.announce);
        self.updates.state = match result {
            Ok(r) if update::is_newer(&r.version, env!("CARGO_PKG_VERSION")) => {
                if announce {
                    let color = self.palette.accent;
                    self.toast(
                        format!("StewardGet {} is available — see Settings › Privacy & data.", r.version),
                        color,
                    );
                }
                UpdateState::Available(r)
            }
            Ok(_) => UpdateState::UpToDate,
            Err(e) => UpdateState::Failed(e),
        };
    }

    /// The "Updates" card of the Privacy tab.
    pub fn updates_section(&mut self, ui: &mut Ui, check_updates: &mut bool) {
        let p = self.palette;
        row(
            ui,
            &p,
            "Check for updates",
            "Off by default. When on, StewardGet asks the project's GitHub release page for the newest version number once each time it starts. Nothing about you or your downloads is sent, and nothing is installed automatically.",
            |ui| {
                widgets::toggle(ui, check_updates, &p);
            },
        );
        row(ui, &p, "Check now", "Look once, right now.", |ui| {
            let ctx = ui.ctx().clone();
            let checking = matches!(self.updates.state, UpdateState::Checking);
            match &self.updates.state {
                UpdateState::Checking => {
                    ui.label(RichText::new("Checking…").color(p.text_dim));
                }
                UpdateState::UpToDate => {
                    ui.label(RichText::new("You have the newest version.").color(p.green));
                }
                UpdateState::Failed(e) => {
                    ui.label(RichText::new(format!("Couldn't check: {e}")).color(p.red));
                }
                UpdateState::Available(r) => {
                    ui.label(RichText::new(format!("Version {} is available.", r.version)).color(p.accent));
                    if widgets::button(ui, ButtonKind::Primary, Some(Icon::Globe), "Open download page", true, &p)
                        .clicked()
                    {
                        let _ = stewardget_core::util::open_url(&r.url);
                    }
                }
                UpdateState::Idle => {}
            }
            if widgets::button(ui, ButtonKind::Ghost, Some(Icon::Retry), "Check now", !checking, &p).clicked() {
                self.check_for_updates(&ctx, false);
            }
        });
    }
}
