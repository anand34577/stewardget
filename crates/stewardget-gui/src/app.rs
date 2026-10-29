//! Application state and the window frame: toolbar, sidebar, status bar.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui::{Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use stewardget_core::util::{PowerAction, format_bytes, format_speed};
use stewardget_core::{
    ApiState, Capture, Category, CloseAction, Config, DownloadId, DownloadView, Engine, EngineSnapshot, MAIN_QUEUE,
    QueueId, Status, Theme,
};
use tokio::runtime::Runtime;

use crate::capture::{CaptureWindow, Inbox};
use crate::details::HashJob;
use crate::dialogs::{AddDialog, ConfirmRemove, ConfirmRestart, PropertiesDialog, RefreshDialog, Welcome};
use crate::icons::{self, Icon};
use crate::queues::QueueDialog;
use crate::table::{SortKey, TableState};
use crate::theme::{self, Palette};
use crate::widgets::{self, ButtonKind};

/// Downloads that appear together and still get a progress window each;
/// more than this (a page's worth of links) get none.
pub const PROGRESS_WINDOWS_AT_ONCE: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    All,
    Active,
    Waiting,
    Completed,
    Failed,
    Category(Category),
    Queue(QueueId),
}

/// Downloads being dragged onto a queue in the sidebar.
pub struct DraggedDownloads(pub Vec<DownloadId>);

impl ListFilter {
    pub fn matches(self, d: &DownloadView) -> bool {
        match self {
            ListFilter::All => true,
            ListFilter::Active => d.status.is_active(),
            ListFilter::Waiting => matches!(d.status, Status::Queued | Status::Paused),
            ListFilter::Completed => d.status == Status::Completed,
            ListFilter::Failed => d.status == Status::Failed,
            ListFilter::Category(c) => d.category == c,
            ListFilter::Queue(q) => d.queue == q,
        }
    }

    pub fn title(self, config: &Config) -> String {
        match self {
            ListFilter::All => "All downloads".into(),
            ListFilter::Active => "Downloading".into(),
            ListFilter::Waiting => "Queued & paused".into(),
            ListFilter::Completed => "Completed".into(),
            ListFilter::Failed => "Failed".into(),
            ListFilter::Category(c) => config.category_label(c),
            ListFilter::Queue(q) => config.queue(q).name.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Downloads,
    History,
    Stats,
    Tools,
    Settings,
}

/// What happens once every download has finished (this session only, like IDM).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AfterAll {
    Nothing,
    Quit,
    Sleep,
    Shutdown,
}

impl AfterAll {
    pub const ALL: [AfterAll; 4] = [AfterAll::Nothing, AfterAll::Quit, AfterAll::Sleep, AfterAll::Shutdown];

    pub fn label(self) -> &'static str {
        match self {
            AfterAll::Nothing => "Do nothing",
            AfterAll::Quit => "Quit StewardGet",
            AfterAll::Sleep => "Sleep",
            AfterAll::Shutdown => "Shut down",
        }
    }
}

pub struct Toast {
    pub text: String,
    pub color: Color32,
    pub born: Instant,
}

pub struct StewardGetApp {
    pub runtime: Runtime,
    pub engine: Engine,
    pub snap: EngineSnapshot,
    /// Bumped whenever `snap` is replaced.
    pub snap_gen: u64,
    snap_taken: Instant,
    rows_cache: std::cell::RefCell<Option<(RowsKey, std::rc::Rc<Vec<usize>>)>>,
    /// The settings as of this frame (refreshed with `snap`).
    pub cfg: Config,
    pub palette: Palette,
    pub page: Page,
    pub history: crate::history::HistoryView,
    pub filter: ListFilter,
    pub search: String,
    pub focus_search: bool,
    pub selection: BTreeSet<DownloadId>,
    pub anchor: Option<DownloadId>,
    /// Row the keyboard moves from (the last one clicked or reached with the arrows).
    pub cursor: Option<DownloadId>,
    /// Scroll the list so this download is visible (keyboard navigation).
    pub scroll_to: Option<DownloadId>,
    /// The list's scroll offset and visible height, from the last frame.
    pub table_view: (f32, f32),
    pub table: TableState,
    pub show_details: bool,
    pub sidebar_pref: Option<bool>,
    pub add: Option<AddDialog>,
    pub confirm: Option<ConfirmRemove>,
    pub restart: Option<ConfirmRestart>,
    pub refresh: Option<RefreshDialog>,
    pub properties: Option<PropertiesDialog>,
    pub settings_draft: Option<Config>,
    pub settings_tab: crate::settings::Tab,
    pub limit_text: String,
    pub toasts: Vec<Toast>,
    /// Open progress windows, one per download.
    pub progress: Vec<crate::progress::ProgressWindow>,
    /// The progress windows show the parts table (IDM's "Show details").
    pub progress_details: bool,
    /// The keyboard shortcuts sheet is open.
    pub shortcuts_open: bool,
    pub hashes: HashMap<DownloadId, HashJob>,
    pub after_all: AfterAll,
    pub power_countdown: Option<Instant>,
    /// First-run setup, shown once.
    pub welcome: Option<Welcome>,
    /// Where the browser extension was unpacked, or why it couldn't be.
    pub extension_dir: Result<PathBuf, String>,
    /// Browser picked on the extension setup page.
    pub setup_browser: crate::settings::Browser,
    /// Login item state, read from the OS when the settings open.
    pub autostart: Option<bool>,
    /// Downloads from the browser waiting for the new-download window.
    pub inbox: Inbox,
    /// The new-download window, while it is open.
    pub capture: Option<CaptureWindow>,
    pub queue_dialog: Option<QueueDialog>,
    /// The custom-category editor, opened from the Categories settings.
    pub category_dialog: Option<crate::categories::CategoryDialog>,
    /// The "find files on a web page" window.
    pub grab_dialog: Option<crate::grab::GrabDialog>,
    pub media_dialog: Option<crate::media::MediaDialog>,
    pub tools: crate::toolbox::ToolsState,
    /// Queue whose deletion is being confirmed.
    pub delete_queue: Option<QueueId>,
    /// "Keep running?" prompt after the close button; holds "remember my choice".
    pub close_prompt: Option<bool>,
    /// The window is closing for real (not minimising to the background).
    quitting: bool,
    /// The main window is hidden in the tray (not just minimised).
    hidden: bool,
    /// Another program (e.g. the installer) asked StewardGet to quit.
    quit_requested: Arc<AtomicBool>,
    /// Another launch of StewardGet asked for the window.
    show_requested: Arc<AtomicBool>,
    /// Minimise on the first frame (`--background`).
    start_minimized: bool,
    /// The window is cloaked (Windows) until it has been put away, so a login
    /// start never shows even the first frame eframe insists on presenting.
    cloaked: Option<windows_hwnd::Hwnd>,
    had_work: bool,
    /// The window has been checked against the screen's work area.
    fitted: bool,
    /// The appearance currently applied, to notice changes (e.g. OS theme).
    applied_look: Option<(Palette, f32)>,
    last_status: HashMap<DownloadId, Status>,
    clipboard: Option<arboard::Clipboard>,
    clipboard_last: Option<String>,
    clipboard_checked: Instant,
    /// Windows' clipboard change counter at the last look (reading is skipped while it stands still).
    clipboard_seq: Option<u32>,
    /// The tray / menu-bar icon (Windows and macOS).
    tray: Option<crate::tray::Tray>,
    tray_updated: Instant,
}

impl StewardGetApp {
    pub fn new(cc: &eframe::CreationContext<'_>, runtime: Runtime, engine: Engine, launch: crate::Launch) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let show_requested = Arc::new(AtomicBool::new(false));
        {
            let (flag, ctx) = (show_requested.clone(), cc.egui_ctx.clone());
            engine.set_show_handler(move || {
                flag.store(true, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        let quit_requested = Arc::new(AtomicBool::new(false));
        {
            let (flag, ctx) = (quit_requested.clone(), cc.egui_ctx.clone());
            engine.set_quit_handler(move || {
                flag.store(true, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        let inbox = Inbox::default();
        {
            let (inbox, ctx) = (inbox.clone(), cc.egui_ctx.clone());
            engine.set_capture_handler(move |batch| {
                inbox.lock().unwrap().push_back(batch);
                ctx.request_repaint();
            });
        }
        {
            // Wake up as soon as a download is added or changes state, also in
            // the tray, where the app otherwise only looks every few seconds:
            // its progress window must not wait for that.
            let (mut events, ctx) = (engine.subscribe(), cc.egui_ctx.clone());
            runtime.spawn(async move {
                use tokio::sync::broadcast::error::RecvError;
                while !matches!(events.recv().await, Err(RecvError::Closed)) {
                    ctx.request_repaint();
                }
            });
        }
        // Links on the command line ("Open with StewardGet") go through the same window.
        let mut launched = Vec::new();
        if !launch.urls.is_empty() {
            let captures = launch.urls.iter().map(|url| Capture { url: url.clone(), ..Default::default() }).collect();
            launched =
                engine.capture_many(captures).map(|r| r.into_iter().map(|(id, _)| id).collect()).unwrap_or_default();
        }
        // Keep the unpacked extension in step with this version of the app.
        let extension_dir = match engine.data_dir() {
            Some(dir) => stewardget_core::extension::unpack(&dir).map_err(|e| e.to_string()),
            None => Err("StewardGet runs without a data folder".to_owned()),
        };
        stewardget_core::extension::refresh_legacy_copy();
        crate::autostart::adopt_legacy();
        let welcome = (engine.first_run() && !launch.background).then(|| Welcome::new(&engine.config()));
        let config = engine.config();
        let palette = Palette::new(config.theme, config.accent, system_dark(&cc.egui_ctx));
        let snap = engine.snapshot();
        // Links from the command line count as new, so they get a progress window like browser ones.
        let last_status =
            snap.downloads.iter().filter(|d| !launched.contains(&d.id)).map(|d| (d.id, d.status)).collect();
        let mut clipboard = arboard::Clipboard::new().ok();
        // Ignore whatever was on the clipboard before StewardGet started.
        let clipboard_last = clipboard.as_mut().and_then(|c| c.get_text().ok());
        let mut app = Self {
            runtime,
            engine,
            snap,
            snap_gen: 0,
            snap_taken: Instant::now(),
            rows_cache: std::cell::RefCell::new(None),
            cfg: config.clone(),
            palette,
            page: Page::Overview,
            history: Default::default(),
            filter: ListFilter::All,
            search: String::new(),
            focus_search: false,
            selection: BTreeSet::new(),
            anchor: None,
            cursor: None,
            scroll_to: None,
            table_view: (0.0, 0.0),
            table: TableState { sort: SortKey::Queue, ascending: true },
            show_details: true,
            sidebar_pref: None,
            add: None,
            confirm: None,
            restart: None,
            refresh: None,
            properties: None,
            settings_draft: None,
            settings_tab: crate::settings::Tab::General,
            limit_text: String::new(),
            toasts: Vec::new(),
            progress: Vec::new(),
            progress_details: false,
            shortcuts_open: false,
            hashes: HashMap::new(),
            after_all: AfterAll::Nothing,
            power_countdown: None,
            welcome,
            extension_dir,
            setup_browser: crate::settings::Browser::Chrome,
            autostart: None,
            inbox,
            capture: None,
            queue_dialog: None,
            category_dialog: None,
            grab_dialog: None,
            media_dialog: None,
            tools: Default::default(),
            delete_queue: None,
            close_prompt: None,
            quitting: false,
            hidden: false,
            quit_requested,
            show_requested,
            start_minimized: launch.background,
            cloaked: launch.background.then(|| windows_hwnd::cloak(cc)).flatten(),
            had_work: false,
            fitted: false,
            applied_look: None,
            last_status,
            clipboard,
            clipboard_last,
            clipboard_checked: Instant::now(),
            clipboard_seq: clipboard_sequence(),
            tray: crate::tray::Tray::new(&cc.egui_ctx, crate::ICON_RGBA.to_vec(), 128),
            tray_updated: Instant::now(),
        };
        app.apply_appearance(&cc.egui_ctx);
        if launch.setup_extension {
            app.welcome = None;
            app.open_settings(crate::settings::Tab::Browser);
        }
        app
    }

    /// Applies theme, accent and zoom from the settings. Cheap when nothing changed.
    pub fn apply_appearance(&mut self, ctx: &egui::Context) {
        let config = self.engine.config();
        let palette = Palette::new(config.theme, config.accent, system_dark(ctx));
        let scale = config.ui_scale;
        let unchanged =
            self.applied_look.is_some_and(|(p, s)| s == scale && p.accent == palette.accent && p.bg == palette.bg);
        if unchanged {
            return;
        }
        self.palette = palette;
        ctx.set_theme(if palette.dark { egui::Theme::Dark } else { egui::Theme::Light });
        palette.apply(ctx);
        ctx.set_zoom_factor(scale);
        self.applied_look = Some((palette, scale));
    }

    pub fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast { text: text.into(), color, born: Instant::now() });
    }

    pub fn report(&mut self, result: Result<(), stewardget_core::Error>) {
        if let Err(e) = result {
            let red = self.palette.red;
            self.toast(capitalize(&e.to_string()), red);
        }
    }

    /// Downloads matching the current filter and search, in display order.
    pub fn visible(&self) -> Vec<DownloadView> {
        self.visible_rows().iter().map(|&i| self.snap.downloads[i].clone()).collect()
    }

    /// [`Self::visible`] as positions in `snap.downloads`, without copying any
    /// download. Kept until the snapshot, filter, search or sort changes, so a
    /// list of thousands isn't filtered and sorted again every frame.
    pub fn visible_rows(&self) -> std::rc::Rc<Vec<usize>> {
        let key = RowsKey {
            snap: self.snap_gen,
            filter: self.filter,
            search: self.search.clone(),
            sort: (self.table.sort, self.table.ascending),
        };
        if let Some((k, rows)) = &*self.rows_cache.borrow()
            && *k == key
        {
            return rows.clone();
        }
        let needle = self.search.to_lowercase();
        let all = &self.snap.downloads;
        let mut rows: Vec<usize> = (0..all.len())
            .filter(|&i| self.filter.matches(&all[i]))
            .filter(|&i| {
                needle.is_empty()
                    || all[i].file_name.to_lowercase().contains(&needle)
                    || all[i].url.to_lowercase().contains(&needle)
            })
            .collect();
        self.table.sort(&mut rows, all);
        let rows = std::rc::Rc::new(rows);
        *self.rows_cache.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    /// Moves the keyboard cursor through the visible rows; with Shift the
    /// selection stretches from the anchor, otherwise it follows the cursor.
    fn move_cursor(&mut self, step: isize, extend: bool) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let ids: Vec<DownloadId> = rows.iter().map(|&i| self.snap.downloads[i].id).collect();
        let from = self.cursor.or(self.anchor).and_then(|c| ids.iter().position(|&id| id == c));
        let to = match from {
            Some(at) => (at as isize + step).clamp(0, ids.len() as isize - 1) as usize,
            None if step < 0 => ids.len() - 1,
            None => 0,
        };
        let id = ids[to];
        if extend && let Some(anchor) = self.anchor.and_then(|a| ids.iter().position(|&x| x == a)) {
            let (lo, hi) = (anchor.min(to), anchor.max(to));
            self.selection = ids[lo..=hi].iter().copied().collect();
        } else {
            self.selection = [id].into();
            self.anchor = Some(id);
        }
        self.cursor = Some(id);
        self.scroll_to = Some(id);
    }

    pub fn selected_views(&self) -> Vec<&DownloadView> {
        self.snap.downloads.iter().filter(|d| self.selection.contains(&d.id)).collect()
    }

    pub fn single_selected(&self) -> Option<DownloadView> {
        match self.selection.len() {
            1 => self.snap.downloads.iter().find(|d| self.selection.contains(&d.id)).cloned(),
            _ => None,
        }
    }

    pub fn open_add(&mut self, urls: Option<String>) {
        let mut dialog = AddDialog::new(&self.engine.config());
        if let ListFilter::Queue(q) = self.filter {
            dialog.queue = q;
        }
        if let Some(text) = urls {
            dialog.urls = text.trim().to_owned();
        }
        self.add = Some(dialog);
    }

    pub fn open_refresh(&mut self, d: &DownloadView) {
        self.refresh = Some(RefreshDialog::new(d));
    }

    pub fn open_properties(&mut self, d: &DownloadView) {
        self.properties = Some(PropertiesDialog::new(d, &self.engine.config()));
    }

    /// Closes the window for real: no "keep running" prompt.
    pub fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The queue the toolbar's start and stop buttons act on, when one is shown.
    fn shown_queue(&self) -> Option<QueueId> {
        match self.filter {
            ListFilter::Queue(q) if self.page == Page::Downloads => Some(q),
            _ => None,
        }
    }

    /// Adds every http(s) link from text files dropped onto the window.
    fn dropped_files(&mut self, ctx: &egui::Context) {
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        if files.is_empty() {
            return;
        }
        let mut links = Vec::new();
        for file in files {
            // Big files are not link lists; don't read a dropped video into memory.
            if std::fs::metadata(file.path()).is_ok_and(|m| m.len() > 16 * 1024 * 1024) {
                continue;
            }
            let text = file.bytes().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            links.extend(
                stewardget_core::util::split_links(&text)
                    .into_iter()
                    .filter(|t| ["http://", "https://", "ftp://", "magnet:"].iter().any(|s| t.starts_with(s))),
            );
        }
        if links.is_empty() {
            let yellow = self.palette.yellow;
            self.toast("No http(s) links in the dropped file", yellow);
        } else {
            self.open_add(Some(links.join("\n")));
        }
    }

    pub fn open_settings(&mut self, tab: crate::settings::Tab) {
        self.settings_draft = Some(self.engine.config());
        self.autostart = Some(crate::autostart::is_enabled());
        self.settings_tab = tab;
        self.page = Page::Settings;
    }

    // ---- actions on the selection -------------------------------------------------

    pub fn resume_selected(&mut self) {
        let ids: Vec<_> = self.selected_views().iter().filter(|d| d.status.can_resume()).map(|d| d.id).collect();
        for id in ids {
            let r = self.engine.resume(id);
            self.report(r);
        }
    }

    pub fn pause_selected(&mut self) {
        let ids: Vec<_> = self.selected_views().iter().filter(|d| d.status.can_pause()).map(|d| d.id).collect();
        for id in ids {
            let r = self.engine.pause(id);
            self.report(r);
        }
    }

    /// Restarting throws data away, so it is confirmed when there is any.
    pub fn restart_selected(&mut self) {
        let views: Vec<DownloadView> = self.selected_views().into_iter().cloned().collect();
        if views.is_empty() {
            return;
        }
        if views.iter().all(|d| d.downloaded == 0 || d.status == Status::Completed && !d.path.exists()) {
            for d in &views {
                let r = self.engine.restart(d.id);
                self.report(r);
            }
        } else {
            self.restart = Some(ConfirmRestart::new(&views));
        }
    }

    pub fn remove_selected(&mut self, delete_files: bool) {
        let views: Vec<DownloadView> = self.selected_views().into_iter().cloned().collect();
        if !views.is_empty() {
            self.confirm = Some(ConfirmRemove::new(&views, delete_files));
        }
    }

    pub fn import_links(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Text", &["txt", "lst", "csv"]).pick_file() else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let links: Vec<String> = stewardget_core::util::split_links(&text)
                    .into_iter()
                    .filter(|t| t.starts_with("http://") || t.starts_with("https://"))
                    .collect();
                if links.is_empty() {
                    let yellow = self.palette.yellow;
                    self.toast("No http(s) links found in that file", yellow);
                } else {
                    self.open_add(Some(links.join("\n")));
                }
            }
            Err(e) => {
                let red = self.palette.red;
                self.toast(format!("Could not read the file: {e}"), red);
            }
        }
    }

    /// Writes the links of the selection (or of the current view) to a text file.
    pub fn export_links(&mut self) {
        let rows: Vec<DownloadView> = if self.selection.is_empty() {
            self.visible()
        } else {
            self.selected_views().into_iter().cloned().collect()
        };
        if rows.is_empty() {
            return;
        }
        let Some(path) = rfd::FileDialog::new().set_file_name("stewardget-links.txt").save_file() else { return };
        let text: String = rows.iter().map(|d| format!("{}\n", d.url)).collect();
        match std::fs::write(&path, text) {
            Ok(()) => {
                let green = self.palette.green;
                self.toast(format!("Exported {} links", rows.len()), green);
            }
            Err(e) => {
                let red = self.palette.red;
                self.toast(format!("Could not write the file: {e}"), red);
            }
        }
    }

    fn track_transitions(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        // Downloads that appeared since the last look and are about to run.
        let mut new_running = Vec::new();
        let (mut finished_names, mut failed_names) = (Vec::new(), Vec::new());
        let completion_dialog = self.cfg.completion_dialog;
        // Finished downloads to show in their own "Download complete" window.
        let mut announce = Vec::new();
        for d in &self.snap.downloads {
            let prev = self.last_status.insert(d.id, d.status);
            // A small file can be done, or a bad link have failed, before the
            // first look: it still gets its window, which then says "Download
            // complete" or what went wrong.
            let just_added = d.added_at + 15 >= stewardget_core::util::now_unix();
            if prev.is_none()
                && (matches!(d.status, Status::Queued | Status::Connecting | Status::Downloading | Status::Verifying)
                    || (d.status.is_finished() && just_added))
            {
                new_running.push(d.id);
            }
            if prev.is_some_and(|p| p != d.status) {
                match d.status {
                    Status::Completed => {
                        finished_names.push(d.file_name.clone());
                        if completion_dialog {
                            announce.push(d.id);
                        } else {
                            events.push((format!("Finished: {}", d.file_name), self.palette.green));
                        }
                    }
                    Status::Failed => {
                        failed_names.push(d.file_name.clone());
                        events.push((format!("Failed: {}", d.file_name), self.palette.red));
                    }
                    _ => {}
                }
            }
        }
        // Like IDM: a download started on its own gets a progress window; a
        // batch doesn't get dozens. Two browser downloads arriving within the
        // same look still get one each.
        if self.cfg.progress_window && new_running.len() <= PROGRESS_WINDOWS_AT_ONCE {
            for id in new_running {
                self.open_progress(id);
            }
        }
        for id in announce {
            self.announce_completion(id);
        }
        let finished = !finished_names.is_empty();
        let in_background = ctx.input(|i| {
            !i.focused || i.raw.viewports.get(&egui::ViewportId::ROOT).and_then(|v| v.minimized).unwrap_or(false)
        });
        if self.cfg.notifications
            && in_background
            && let Some((summary, body)) = crate::notify::describe(&finished_names, &failed_names)
        {
            crate::notify::show(summary, body);
        }
        // One lookup set per frame: checking each id against the whole list was
        // quadratic and showed up with thousands of downloads.
        if self.last_status.len() != self.snap.downloads.len() || !self.selection.is_empty() {
            let ids: std::collections::HashSet<DownloadId> = self.snap.downloads.iter().map(|d| d.id).collect();
            self.last_status.retain(|id, _| ids.contains(id));
            self.selection.retain(|id| ids.contains(id));
        }
        for (text, color) in events {
            self.toast(text, color);
        }
        if finished && in_background && self.cfg.completion_sound {
            crate::notify::beep();
        }
        if finished && !ctx.input(|i| i.focused) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
    }

    /// Starts the "when all done" countdown once the queue has drained.
    fn watch_queue_end(&mut self) {
        if self.after_all == AfterAll::Nothing {
            self.had_work = false;
            self.power_countdown = None;
            return;
        }
        let busy = self.snap.downloads.iter().any(|d| d.status.is_active() || d.status == Status::Queued);
        if busy {
            self.had_work = true;
            self.power_countdown = None;
        } else if self.had_work && self.power_countdown.is_none() {
            self.power_countdown = Some(Instant::now());
        }
    }

    pub fn run_after_all(&mut self, ctx: &egui::Context) {
        let action = self.after_all;
        self.after_all = AfterAll::Nothing;
        self.power_countdown = None;
        self.had_work = false;
        let result = match action {
            AfterAll::Nothing => Ok(()),
            AfterAll::Quit => {
                self.quit(ctx);
                Ok(())
            }
            AfterAll::Sleep => stewardget_core::util::power(PowerAction::Sleep),
            AfterAll::Shutdown => {
                let r = stewardget_core::util::power(PowerAction::Shutdown);
                self.quit(ctx);
                r
            }
        };
        if let Err(e) = result {
            let red = self.palette.red;
            self.toast(format!("Could not {}: {e}", action.label().to_lowercase()), red);
        }
    }

    /// IDM-style clipboard monitoring: copying a file link offers to download it.
    /// Runs every pass, also while the window is minimised, and brings the
    /// window forward for the offer when it is in the background.
    fn watch_clipboard(&mut self, ctx: &egui::Context) {
        if self.clipboard_checked.elapsed() < Duration::from_millis(900) || self.add.is_some() {
            return;
        }
        self.clipboard_checked = Instant::now();
        if !self.cfg.clipboard_monitor {
            return;
        }
        // Opening the clipboard on every look disturbs other programs; Windows says whether it changed.
        let seq = clipboard_sequence();
        if seq.is_some() && seq == self.clipboard_seq {
            return;
        }
        self.clipboard_seq = seq;
        let Some(text) = self.clipboard.as_mut().and_then(|c| c.get_text().ok()) else { return };
        if self.clipboard_last.as_deref() == Some(text.as_str()) {
            return;
        }
        self.clipboard_last = Some(text.clone());
        let candidate = text.trim();
        if looks_like_file_link(candidate, &self.cfg) && !self.snap.downloads.iter().any(|d| d.url == candidate) {
            let minimized =
                ctx.input(|i| i.raw.viewports.get(&egui::ViewportId::ROOT).and_then(|v| v.minimized).unwrap_or(false));
            if (self.hidden || minimized) && self.capture.is_none() {
                // Offer it in the small new-download window and leave the main window where it is.
                self.inbox.lock().unwrap().push_back(vec![stewardget_core::NewDownload::new(candidate)]);
                ctx.request_repaint();
            } else {
                self.open_add(Some(candidate.to_owned()));
                if !ctx.input(|i| i.focused) {
                    self.show_window(ctx);
                }
            }
        }
    }

    fn any_dialog(&self) -> bool {
        self.add.is_some()
            || self.confirm.is_some()
            || self.restart.is_some()
            || self.refresh.is_some()
            || self.properties.is_some()
            || self.power_countdown.is_some()
            || self.queue_dialog.is_some()
            || self.category_dialog.is_some()
            || self.delete_queue.is_some()
            || self.close_prompt.is_some()
            || self.capture.is_some()
            || self.shortcuts_open
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Q)) {
            self.quit(ctx);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.shortcuts_open = !self.shortcuts_open;
        }
        if self.any_dialog() {
            return;
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        let keys = ctx.input(|i| {
            let paste = i.events.iter().find_map(|e| match e {
                egui::Event::Paste(text) if text.contains("://") => Some(text.clone()),
                _ => None,
            });
            let cmd = i.modifiers.command;
            Keys {
                new: cmd && i.key_pressed(egui::Key::N),
                find: cmd && i.key_pressed(egui::Key::F),
                import: cmd && i.key_pressed(egui::Key::O),
                settings: cmd && i.key_pressed(egui::Key::Comma),
                paste,
                space: i.key_pressed(egui::Key::Space),
                delete: i.key_pressed(egui::Key::Delete),
                shift: i.modifiers.shift,
                select_all: cmd && i.key_pressed(egui::Key::A),
                enter: i.key_pressed(egui::Key::Enter),
                properties: i.key_pressed(egui::Key::F2) || (i.modifiers.alt && i.key_pressed(egui::Key::Enter)),
                escape: i.key_pressed(egui::Key::Escape),
                step: if i.key_pressed(egui::Key::ArrowDown) {
                    1
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    -1
                } else if i.key_pressed(egui::Key::PageDown) {
                    10
                } else if i.key_pressed(egui::Key::PageUp) {
                    -10
                } else if i.key_pressed(egui::Key::End) {
                    isize::MAX / 2
                } else if i.key_pressed(egui::Key::Home) {
                    isize::MIN / 2
                } else {
                    0
                },
            }
        });
        if keys.new {
            self.open_add(None);
        }
        if keys.import {
            self.import_links();
        }
        if keys.settings {
            self.open_settings(crate::settings::Tab::General);
        }
        if keys.find {
            if self.page != Page::History {
                self.page = Page::Downloads;
            }
            self.focus_search = true;
        }
        if typing || self.page != Page::Downloads {
            return;
        }
        if let Some(text) = keys.paste {
            self.open_add(Some(text));
        }
        if keys.escape {
            self.selection.clear();
        }
        if keys.step != 0 {
            self.move_cursor(keys.step, keys.shift);
        }
        if keys.select_all {
            self.selection = self.visible_rows().iter().map(|&i| self.snap.downloads[i].id).collect();
        }
        if keys.delete {
            self.remove_selected(keys.shift);
        }
        if keys.space {
            let any_running = self.selected_views().iter().any(|d| d.status.can_pause());
            if any_running { self.pause_selected() } else { self.resume_selected() }
        }
        if keys.properties {
            if let Some(d) = self.single_selected() {
                self.open_properties(&d);
            }
        } else if keys.enter
            && let Some(d) = self.single_selected()
        {
            if d.status == Status::Completed {
                let _ = stewardget_core::util::open_with_system(&d.path);
            } else {
                self.open_progress(d.id);
            }
        }
    }

    // ---- frame ---------------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut Ui, compact: bool, collapsed: bool) {
        let p = self.palette;
        let sel = self.selected_views();
        let can_resume = sel.iter().any(|d| d.status.can_resume());
        let can_pause = sel.iter().any(|d| d.status.can_pause());
        let has_sel = !sel.is_empty();
        let can_refresh = sel.len() == 1 && sel[0].status != Status::Completed;
        let label = |s: &'static str| if compact { "" } else { s };
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let tip = if collapsed { "Expand sidebar" } else { "Collapse sidebar" };
            if widgets::icon_button(ui, Icon::Sidebar, tip, &p).clicked() {
                self.sidebar_pref = Some(!collapsed);
            }
            ui.add_space(6.0);
            if widgets::button(ui, ButtonKind::Primary, Some(Icon::Plus), "Add URL", true, &p)
                .on_hover_text("Add downloads (Ctrl+N)")
                .clicked()
            {
                self.open_add(None);
            }
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Globe), label("Grab"), true, &p)
                .on_hover_text("Find the files linked from a web page")
                .clicked()
            {
                self.grab_dialog = Some(crate::grab::GrabDialog::new(None));
            }
            ui.add_space(5.0);
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Play), label("Resume"), can_resume, &p)
                .on_hover_text("Resume selected (Space)")
                .clicked()
            {
                self.resume_selected();
            }
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Pause), label("Pause"), can_pause, &p)
                .on_hover_text("Pause selected (Space)")
                .clicked()
            {
                self.pause_selected();
            }
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Retry), label("Restart"), has_sel, &p)
                .on_hover_text("Download again from scratch")
                .clicked()
            {
                self.restart_selected();
            }
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Link), label("Refresh link"), can_refresh, &p)
                .on_hover_text("The link expired? Capture a fresh one and continue where it stopped")
                .clicked()
                && let Some(d) = self.single_selected()
            {
                self.open_refresh(&d);
            }
            if widgets::button(ui, ButtonKind::Tool, Some(Icon::Trash), label("Remove"), has_sel, &p)
                .on_hover_text("Remove selected (Delete; Shift+Delete also deletes files)")
                .clicked()
            {
                self.remove_selected(false);
            }
            toolbar_separator(ui, &p);
            if let Some(q) = self.shown_queue() {
                let name = self.engine.config().queue(q).name.clone();
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Play), label("Start queue"), true, &p)
                    .on_hover_text(format!("Resume the paused and failed downloads of {name}"))
                    .clicked()
                {
                    self.engine.start_queue(q);
                }
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Pause), label("Stop queue"), true, &p)
                    .on_hover_text(format!("Pause every download of {name}"))
                    .clicked()
                {
                    self.engine.stop_queue(q);
                }
            } else {
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Play), label("Start all"), true, &p)
                    .on_hover_text("Resume every paused or failed download")
                    .clicked()
                {
                    self.engine.resume_all();
                }
                if widgets::button(ui, ButtonKind::Tool, Some(Icon::Pause), label("Pause all"), true, &p)
                    .on_hover_text("Pause everything")
                    .clicked()
                {
                    self.engine.pause_all();
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(
                    ui,
                    Icon::PanelBottom,
                    if self.show_details { "Hide details" } else { "Show details" },
                    &p,
                )
                .clicked()
                {
                    self.show_details = !self.show_details;
                }
                let more = widgets::icon_button(ui, Icon::More, "More", &p);
                egui::Popup::menu(&more).show(|ui| self.more_menu(ui));
                ui.add_space(4.0);
                let width = if compact { 160.0 } else { 240.0 };
                self.search_box(ui, width);
            });
        });
    }

    fn search_box(&mut self, ui: &mut Ui, width: f32) {
        let p = self.palette;
        let r = ui
            .add(widgets::input(&mut self.search).hint_text("Search").desired_width(width).margin(Margin {
                left: 32,
                right: 10,
                top: 7,
                bottom: 7,
            }))
            .on_hover_text("Search names and links (Ctrl+F)");
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(r.rect.left_center() + vec2(17.0, 0.0), vec2(15.0, 15.0)),
            Icon::Search,
            p.text_dim,
        );
        if self.focus_search {
            r.request_focus();
            self.focus_search = false;
        }
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.clear();
            r.surrender_focus();
        }
    }

    fn more_menu(&mut self, ui: &mut Ui) {
        let p = self.palette;
        if menu_item(ui, &p, Icon::Layers, "Batch download…", true) {
            self.open_add(Some("https://example.com/files/image[001-100].jpg".into()));
            ui.close();
        }
        if menu_item(ui, &p, Icon::Import, "Import links from file…  Ctrl+O", true) {
            self.import_links();
            ui.close();
        }
        let n = if self.selection.is_empty() { self.visible_rows().len() } else { self.selection.len() };
        if menu_item(ui, &p, Icon::Export, &format!("Export {n} links…"), n > 0) {
            self.export_links();
            ui.close();
        }
        ui.separator();
        let done = self.snap.count(|s| s == Status::Completed);
        if menu_item(ui, &p, Icon::Checks, &format!("Clear {done} completed"), done > 0) {
            self.engine.clear_completed();
            ui.close();
        }
        if menu_item(ui, &p, Icon::Queue, "New queue…", true) {
            self.open_queue_dialog(None);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Calendar, "Queues & schedule…", true) {
            self.open_settings(crate::settings::Tab::Scheduler);
            ui.close();
        }
        ui.separator();
        ui.add_space(2.0);
        ui.label(RichText::new("   When all downloads finish").size(11.5).color(p.text_dim));
        for action in AfterAll::ALL {
            if menu_row(ui, &p, None, action.label(), true, self.after_all == action) {
                self.after_all = action;
                ui.close();
            }
        }
        ui.separator();
        if menu_item(ui, &p, Icon::Gear, "Settings  Ctrl+,", true) {
            self.open_settings(crate::settings::Tab::General);
            ui.close();
        }
        if menu_item(ui, &p, Icon::Power, "Quit StewardGet  Ctrl+Q", true) {
            self.quit(ui.ctx());
            ui.close();
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, collapsed: bool) {
        let p = self.palette;
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if collapsed {
                ui.add_space((ui.available_width() - 30.0) / 2.0 - 4.0);
            } else {
                ui.add_space(4.0);
            }
            widgets::logo(ui, 30.0);
            if !collapsed {
                ui.add_space(2.0);
                ui.label(RichText::new("StewardGet").size(18.0).family(theme::semi()).color(p.text));
            }
        });
        ui.add_space(8.0);

        // The lists scroll when the window is short; Statistics and Settings stay pinned.
        // Three pinned entries, plus the version line when the sidebar is open.
        let list_h = (ui.available_height() - 3.0 * 34.0 - 10.0 - if collapsed { 0.0 } else { 24.0 }).max(60.0);
        ui.allocate_ui(vec2(ui.available_width(), list_h), |ui| {
            egui::ScrollArea::vertical().id_salt("nav-scroll").auto_shrink([false, false]).show(ui, |ui| {
                // Keep clear of the scroll bar, which floats over the right edge.
                ui.set_max_width(ui.available_width() - 8.0);
                fn count(all: &[DownloadView], f: ListFilter) -> usize {
                    all.iter().filter(|d| f.matches(d)).count()
                }
                if nav_item(ui, Icon::Gauge, None, "Overview", 0, self.page == Page::Overview, collapsed, &p).clicked()
                {
                    self.page = Page::Overview;
                }
                if nav_item(ui, Icon::History, None, "History", 0, self.page == Page::History, collapsed, &p).clicked()
                {
                    self.page = Page::History;
                }
                ui.add_space(6.0);
                let open = collapsed || widgets::section(ui, "DOWNLOADS", &p);
                if !collapsed {
                    ui.add_space(2.0);
                }
                let items = [
                    (Icon::Inbox, "All", ListFilter::All),
                    (Icon::Download, "Downloading", ListFilter::Active),
                    (Icon::Clock, "Queued & paused", ListFilter::Waiting),
                    (Icon::CheckCircle, "Completed", ListFilter::Completed),
                    (Icon::Alert, "Failed", ListFilter::Failed),
                ];
                for (icon, label, filter) in items.into_iter().filter(|_| open) {
                    let selected = self.page == Page::Downloads && self.filter == filter;
                    if nav_item(ui, icon, None, label, count(&self.snap.downloads, filter), selected, collapsed, &p)
                        .clicked()
                    {
                        self.page = Page::Downloads;
                        self.filter = filter;
                    }
                }
                ui.add_space(6.0);
                self.sidebar_queues(ui, collapsed);
                ui.add_space(6.0);
                let open = collapsed || widgets::section(ui, "CATEGORIES", &p);
                if !collapsed {
                    ui.add_space(2.0);
                }
                for c in self.cfg.categories().into_iter().filter(|_| open) {
                    let filter = ListFilter::Category(c);
                    let selected = self.page == Page::Downloads && self.filter == filter;
                    if nav_item(
                        ui,
                        Icon::for_category(c),
                        Some(p.category(c)),
                        &self.cfg.category_label(c),
                        count(&self.snap.downloads, filter),
                        selected,
                        collapsed,
                        &p,
                    )
                    .clicked()
                    {
                        self.page = Page::Downloads;
                        self.filter = filter;
                    }
                }
            });
        });
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            if !collapsed {
                let version = ui
                    .add(
                        egui::Label::new(
                            RichText::new(concat!("StewardGet v", env!("CARGO_PKG_VERSION")))
                                .size(11.5)
                                .color(p.text_dim),
                        )
                        .sense(Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("About StewardGet");
                if version.clicked() {
                    self.open_settings(crate::settings::Tab::About);
                }
                ui.add_space(4.0);
            }
            if nav_item(ui, Icon::Gear, None, "Settings", 0, self.page == Page::Settings, collapsed, &p).clicked() {
                self.open_settings(self.settings_tab);
            }
            if nav_item(ui, Icon::Chart, None, "Statistics", 0, self.page == Page::Stats, collapsed, &p).clicked() {
                self.page = Page::Stats;
            }
            if nav_item(ui, Icon::Sliders, None, "Tools", 0, self.page == Page::Tools, collapsed, &p).clicked() {
                self.page = Page::Tools;
            }
        });
    }

    /// The QUEUES section: one entry per queue, a drop target for dragged downloads.
    fn sidebar_queues(&mut self, ui: &mut Ui, collapsed: bool) {
        let p = self.palette;
        let config = self.engine.config();
        let mut open = true;
        if !collapsed {
            ui.horizontal(|ui| {
                open = widgets::section(ui, "QUEUES", &p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (rect, r) = ui.allocate_exact_size(vec2(18.0, 14.0), Sense::click());
                    let color = if r.hovered() { p.accent } else { p.text_dim };
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(rect.center(), vec2(13.0, 13.0)),
                        Icon::Plus,
                        color,
                    );
                    if r.on_hover_text("New queue").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.open_queue_dialog(None);
                    }
                });
            });
            ui.add_space(2.0);
        }
        let dragging = egui::DragAndDrop::has_payload_of_type::<DraggedDownloads>(ui.ctx());
        for q in config.queues.iter().filter(|_| open) {
            let filter = ListFilter::Queue(q.id);
            let count = self.snap.downloads.iter().filter(|d| d.queue == q.id).count();
            let active = self.snap.downloads.iter().filter(|d| d.queue == q.id && d.status.is_active()).count();
            let selected = self.page == Page::Downloads && self.filter == filter;
            let tint = if active > 0 { Some(p.accent2) } else { None };
            let response = nav_item(ui, Icon::Queue, tint, &q.name, count, selected, collapsed, &p);
            if dragging && response.contains_pointer() {
                ui.painter().rect_stroke(
                    response.rect,
                    CornerRadius::same(8),
                    Stroke::new(1.5, p.accent),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(payload) = response.dnd_release_payload::<DraggedDownloads>() {
                let r = self.engine.set_queue(&payload.0, q.id);
                self.report(r);
                let n = payload.0.len();
                let accent = p.accent2;
                self.toast(format!("Moved {n} download{} to {}", if n == 1 { "" } else { "s" }, q.name), accent);
            }
            let response = response.on_hover_text(crate::queues::summary(q));
            if response.clicked() {
                self.page = Page::Downloads;
                self.filter = filter;
            }
            response.context_menu(|ui| self.queue_menu(ui, q.id));
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let s = &self.snap;
        let speed = s.speed;
        let today = s.stats.today(stewardget_core::util::now_unix());
        let (active, queued, paused, done, failed) = (
            s.count(Status::is_active),
            s.count(|x| x == Status::Queued),
            s.count(|x| x == Status::Paused),
            s.count(|x| x == Status::Completed),
            s.count(|x| x == Status::Failed),
        );
        let api = s.api.clone();
        let config = self.engine.config();
        let wide = ui.available_width() > 900.0;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            widgets::icon(ui, Icon::Download, 14.0, p.accent2);
            ui.label(RichText::new(format_speed(speed)).size(12.0).family(theme::semi()).color(p.text));
            sep(ui, &p);
            let paused = if paused > 0 { format!(" · {paused} paused") } else { String::new() };
            ui.label(
                RichText::new(format!("{active} active · {queued} queued{paused} · {done} done"))
                    .size(12.0)
                    .color(p.text_dim),
            );
            if failed > 0 {
                ui.label(RichText::new(format!("· {failed} failed")).size(12.0).color(p.red));
            }
            if wide {
                sep(ui, &p);
                ui.label(RichText::new(format!("Today {}", format_bytes(today))).size(12.0).color(p.text_dim));
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.limit_picker(ui);
                let limit_label = match self.snap.scheduled_limit {
                    Some(limit) => {
                        let now = if limit == 0 { "unlimited".to_owned() } else { format_speed(limit as f64) };
                        let r = status_link(ui, &p, &format!("Scheduled limit {now} \u{00b7} Limit"), Some(p.yellow))
                            .on_hover_text(
                                "A time-of-day speed limit is in force; the limit picked here applies outside its hours. Click to change the hours.",
                            );
                        if r.clicked() {
                            self.open_settings(crate::settings::Tab::Connection);
                        }
                        None
                    }
                    None => Some("Limit"),
                };
                if let Some(text) = limit_label {
                    ui.label(RichText::new(text).size(12.0).color(p.text_dim));
                }
                sep(ui, &p);
                let connected = self.snap.extension.as_ref().is_some_and(|x| x.seen_secs < 150);
                let (color, text, tip) = match &api {
                    ApiState::Listening(port) if connected => (
                        p.green,
                        "Browser connected".to_owned(),
                        format!("Accepting downloads from the browser extension on 127.0.0.1:{port}"),
                    ),
                    ApiState::Listening(_) => (
                        p.yellow,
                        "Connect browser".to_owned(),
                        "No browser extension has connected yet. Click to set it up.".to_owned(),
                    ),
                    ApiState::Disabled => {
                        (p.text_dim, "Browser off".to_owned(), "Browser integration is disabled in Settings".to_owned())
                    }
                    ApiState::Failed(e) => (p.red, "Browser error".to_owned(), e.clone()),
                };
                if status_link(ui, &p, &text, Some(color)).on_hover_text(tip).clicked() {
                    self.open_settings(crate::settings::Tab::Browser);
                }
                let scheduled: Vec<_> = config.queues.iter().filter(|q| q.schedule.enabled).collect();
                if !scheduled.is_empty() {
                    sep(ui, &p);
                    let text = match scheduled.as_slice() {
                        [q] => {
                            let stop = q.schedule.stop_minute.map_or(String::new(), |m| format!("–{}", hhmm(m)));
                            let who = if q.id == MAIN_QUEUE { "Scheduled".to_owned() } else { q.name.clone() };
                            format!("{who} {}{stop}", hhmm(q.schedule.start_minute))
                        }
                        many => format!("{} queues scheduled", many.len()),
                    };
                    let tip: Vec<String> =
                        scheduled.iter().map(|q| format!("{}: {}", q.name, crate::queues::summary(q))).collect();
                    if status_link(ui, &p, &text, None).on_hover_text(tip.join("\n")).clicked() {
                        self.open_settings(crate::settings::Tab::Scheduler);
                    }
                }
                if self.after_all != AfterAll::Nothing {
                    sep(ui, &p);
                    let text = format!("When done: {}", self.after_all.label());
                    if status_link(ui, &p, &text, Some(p.yellow)).on_hover_text("Click to cancel").clicked() {
                        self.after_all = AfterAll::Nothing;
                    }
                }
            });
        });
    }

    fn limit_picker(&mut self, ui: &mut Ui) {
        const PRESETS: [u64; 9] = [0, 128 << 10, 256 << 10, 512 << 10, 1 << 20, 2 << 20, 5 << 20, 10 << 20, 50 << 20];
        let current = self.snap.speed_limit;
        let label = |v: u64| if v == 0 { "Unlimited".to_owned() } else { format_speed(v as f64) };
        let mut chosen = current;
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 20.0;
            ui.spacing_mut().button_padding = vec2(8.0, 1.0);
            egui::ComboBox::from_id_salt("speed-limit")
                .width(104.0)
                .selected_text(RichText::new(label(current)).size(12.0))
                .show_ui(ui, |ui| {
                    for v in PRESETS {
                        ui.selectable_value(&mut chosen, v, label(v));
                    }
                });
        });
        if chosen != current {
            self.engine.set_speed_limit(chosen);
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.born.elapsed() < Duration::from_millis(4500));
        if self.toasts.is_empty() {
            return;
        }
        let p = self.palette;
        egui::Area::new(egui::Id::unique("stewardget-toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -42.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for t in self.toasts.iter().rev().take(4) {
                    let fade = (1.0 - (t.born.elapsed().as_secs_f32() - 3.9).max(0.0) / 0.6).clamp(0.0, 1.0);
                    Frame::new()
                        .fill(p.card.gamma_multiply(fade))
                        .stroke(Stroke::new(1.0, p.border.gamma_multiply(fade)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(14, 10))
                        .shadow(p.shadow(4, 18))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                widgets::dot(ui.painter(), r.center(), t.color.gamma_multiply(fade));
                                ui.add(
                                    egui::Label::new(RichText::new(&t.text).color(p.text.gamma_multiply(fade))).wrap(),
                                );
                            });
                        });
                    ui.add_space(6.0);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

/// What [`StewardGetApp::visible_rows`] was computed for.
#[derive(PartialEq)]
struct RowsKey {
    snap: u64,
    filter: ListFilter,
    search: String,
    sort: (SortKey, bool),
}

struct Keys {
    new: bool,
    find: bool,
    import: bool,
    settings: bool,
    paste: Option<String>,
    space: bool,
    delete: bool,
    shift: bool,
    select_all: bool,
    enter: bool,
    properties: bool,
    escape: bool,
    /// Rows to move the cursor by (arrows, Page Up/Down, Home/End).
    step: isize,
}

fn system_dark(ctx: &egui::Context) -> bool {
    ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark)
}

pub fn hhmm(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Plausible "file" link: http(s) with a known downloadable extension.
fn looks_like_file_link(text: &str, config: &Config) -> bool {
    if text.contains(char::is_whitespace) || !(text.starts_with("http://") || text.starts_with("https://")) {
        return false;
    }
    let Ok(url) = url_lite(text) else { return false };
    let name = url.rsplit('/').next().unwrap_or("");
    config.categorize(name) != Category::Other
}

/// Path part of a URL without pulling in a parser.
fn url_lite(text: &str) -> Result<&str, ()> {
    let rest = text.split_once("://").ok_or(())?.1;
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
    Ok(path.split(['?', '#']).next().unwrap_or(""))
}

fn sep(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 14.0), Sense::hover());
    ui.painter().line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(1.0, p.border));
}

/// Clickable status-bar text with an optional leading dot.
fn status_link(ui: &mut Ui, p: &Palette, text: &str, dot: Option<Color32>) -> egui::Response {
    let r = ui
        .add(egui::Label::new(RichText::new(text).size(12.0).color(p.text_dim)).sense(Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if let Some(color) = dot {
        let (dot_rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        widgets::dot(ui.painter(), dot_rect.center(), color);
    }
    r
}

/// A menu row: icon, label and an optional shortcut after two spaces
/// ("Open  Ctrl+O"). Returns whether it was clicked.
pub fn menu_item(ui: &mut Ui, p: &Palette, icon: Icon, text: &str, enabled: bool) -> bool {
    menu_row(ui, p, Some(icon), text, enabled, false)
}

fn menu_row(ui: &mut Ui, p: &Palette, icon: Option<Icon>, text: &str, enabled: bool, checked: bool) -> bool {
    let (label, shortcut) = match text.split_once("  ") {
        Some((l, s)) => (l, Some(s)),
        None => (text, None),
    };
    let width = ui.available_width().clamp(220.0, 280.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), sense);
    let painter = ui.painter();
    if enabled && response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.card_hover);
    }
    let fg = if enabled { p.text } else { p.text_dim.gamma_multiply(0.6) };
    let icon = if checked { Some(Icon::Check) } else { icon };
    if let Some(icon) = icon {
        let color = if checked {
            p.accent
        } else if enabled {
            p.text_dim
        } else {
            p.text_dim.gamma_multiply(0.4)
        };
        icons::paint(
            painter,
            egui::Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(15.0, 15.0)),
            icon,
            color,
        );
    }
    painter.text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.5), fg);
    if let Some(s) = shortcut {
        painter.text(
            rect.right_center() - vec2(10.0, 0.0),
            Align2::RIGHT_CENTER,
            s,
            FontId::proportional(11.5),
            p.text_dim,
        );
    }
    let response = if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response };
    response.clicked()
}

fn toolbar_separator(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.border);
    ui.add_space(4.0);
}

#[allow(clippy::too_many_arguments)]
fn nav_item(
    ui: &mut Ui,
    icon: Icon,
    tint: Option<Color32>,
    label: &str,
    count: usize,
    selected: bool,
    collapsed: bool,
    p: &Palette,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    // Explicit ids keep navigation stable regardless of layout nesting; the
    // icon tells a queue called "Video" from the Video category.
    let response = ui.interact(rect, egui::Id::unique(("stewardget-nav", label, format!("{icon:?}"))), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(8), p.accent.gamma_multiply(if p.dark { 0.18 } else { 0.12 }));
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(8), p.card_hover);
    }
    let fg = if selected { p.text } else { p.text_dim };
    let icon_color = tint.unwrap_or(if selected { p.accent } else { fg });
    let icon_x = if collapsed { rect.center().x } else { rect.left() + 18.0 };
    icons::paint(
        painter,
        egui::Rect::from_center_size(egui::pos2(icon_x, rect.center().y), vec2(16.0, 16.0)),
        icon,
        icon_color,
    );
    if !collapsed {
        let family = if selected { theme::semi() } else { egui::FontFamily::Proportional };
        painter.text(rect.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, label, FontId::new(13.5, family), fg);
        if count > 0 {
            painter.text(
                rect.right_center() - vec2(10.0, 0.0),
                Align2::RIGHT_CENTER,
                count.to_string(),
                FontId::proportional(11.5),
                p.text_dim,
            );
        }
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if collapsed { response.on_hover_text(format!("{label} ({count})")) } else { response }
}

/// Cloaking a window with the DWM keeps it from being drawn without hiding it.
mod windows_hwnd {
    /// A window handle, where there are such things.
    #[cfg(windows)]
    pub type Hwnd = isize;
    #[cfg(not(windows))]
    pub type Hwnd = std::convert::Infallible;

    /// Cloaks the new main window. `None` where that isn't possible.
    #[cfg(windows)]
    pub fn cloak(cc: &eframe::CreationContext<'_>) -> Option<Hwnd> {
        use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        let RawWindowHandle::Win32(handle) = cc.window_handle().ok()?.as_raw() else { return None };
        let hwnd = handle.hwnd.get();
        set(hwnd, true).then_some(hwnd)
    }

    #[cfg(not(windows))]
    pub fn cloak(_cc: &eframe::CreationContext<'_>) -> Option<Hwnd> {
        None
    }

    #[cfg(windows)]
    pub fn uncloak(hwnd: Hwnd) {
        set(hwnd, false);
    }

    #[cfg(not(windows))]
    pub fn uncloak(hwnd: Hwnd) {
        match hwnd {}
    }

    #[cfg(windows)]
    fn set(hwnd: Hwnd, cloaked: bool) -> bool {
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
        let value = i32::from(cloaked);
        // SAFETY: `hwnd` came from the window's own handle and `value` is a BOOL, as DWMWA_CLOAK expects.
        unsafe {
            DwmSetWindowAttribute(hwnd as _, DWMWA_CLOAK as u32, (&raw const value).cast(), size_of::<i32>() as u32)
                >= 0
        }
    }
}

/// Shrinks and moves the window so it fits the screen's work area: the
/// default (or remembered) size can be taller than a laptop screen minus the
/// taskbar, which hid the status bar. Returns `true` once the check is done.
fn fit_to_work_area(ctx: &egui::Context) -> bool {
    let (outer, inner, maximized, ppp) = ctx.input(|i| {
        let v = i.viewport();
        (v.outer_rect, v.inner_rect, v.maximized.unwrap_or(false), v.native_pixels_per_point)
    });
    let (Some(outer), Some(inner), Some(ppp)) = (outer, inner, ppp) else { return false };
    if maximized {
        return true;
    }
    // Only the primary screen's work area is known; a window on another screen is left alone.
    let Some(work) = work_area(ppp).filter(|w| w.contains(outer.center())) else { return true };
    let margin = 8.0;
    let max_h = work.height() - 2.0 * margin;
    let max_w = work.width() - 2.0 * margin;
    let fits = outer.height() <= max_h && outer.width() <= max_w && work.contains_rect(outer);
    if fits {
        return true;
    }
    let chrome = outer.size() - inner.size();
    let size = vec2(outer.width().min(max_w), outer.height().min(max_h));
    let x = outer.left().clamp(work.left() + margin, (work.right() - margin - size.x).max(work.left() + margin));
    let y = outer.top().clamp(work.top() + margin, (work.bottom() - margin - size.y).max(work.top() + margin));
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size - chrome));
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
    true
}

/// The primary screen minus the taskbar, in points.
#[cfg(windows)]
fn work_area(ppp: f32) -> Option<egui::Rect> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: SPI_GETWORKAREA writes one RECT to the valid pointer passed.
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut r).cast(), 0) } != 0;
    (ok && r.right > r.left && r.bottom > r.top).then(|| {
        egui::Rect::from_min_max(
            egui::pos2(r.left as f32 / ppp, r.top as f32 / ppp),
            egui::pos2(r.right as f32 / ppp, r.bottom as f32 / ppp),
        )
    })
}

#[cfg(not(windows))]
fn work_area(_ppp: f32) -> Option<egui::Rect> {
    None
}

/// Windows' clipboard change counter; `None` elsewhere.
fn clipboard_sequence() -> Option<u32> {
    #[cfg(windows)]
    {
        // SAFETY: plain Win32 call without arguments.
        let n = unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        (n != 0).then_some(n)
    }
    #[cfg(not(windows))]
    None
}

impl StewardGetApp {
    /// The close button: quit, or keep running minimised, as the settings say.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if self.quitting || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        match self.engine.config().close_action {
            CloseAction::Quit => self.quitting = true,
            CloseAction::Background => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.park(ctx);
            }
            CloseAction::Ask => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_prompt = Some(true);
            }
        }
    }

    /// While downloads are dragged: a label at the pointer, and a hint when files hover the window.
    fn drag_overlay(&mut self, ctx: &egui::Context) {
        let p = self.palette;
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("stewardget-drag")));
        if let Some(payload) = egui::DragAndDrop::payload::<DraggedDownloads>(ctx)
            && let Some(pos) = ctx.pointer_hover_pos()
        {
            let n = payload.0.len();
            let text =
                if n == 1 { "Move 1 download to a queue".to_owned() } else { format!("Move {n} downloads to a queue") };
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.5), p.text);
            let rect = egui::Rect::from_min_size(pos + vec2(14.0, 10.0), galley.size() + vec2(20.0, 10.0));
            painter.rect(rect, CornerRadius::same(8), p.card, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
            painter.galley(rect.min + vec2(10.0, 5.0), galley, p.text);
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let screen = ctx.content_rect();
            painter.rect_filled(screen, CornerRadius::ZERO, p.backdrop());
            let card = egui::Rect::from_center_size(screen.center(), vec2(360.0, 90.0));
            painter.rect(card, CornerRadius::same(14), p.card, Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
            painter.text(
                card.center() - vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                "Drop to add the links",
                FontId::new(16.0, theme::semi()),
                p.text,
            );
            painter.text(
                card.center() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                "Text files with one link per line",
                FontId::proportional(12.5),
                p.text_dim,
            );
        }
    }
}

impl StewardGetApp {
    /// Acts on clicks in the tray menu and keeps its tooltip current.
    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &mut self.tray else { return };
        let actions = tray.poll();
        if self.tray_updated.elapsed() >= Duration::from_secs(1) || !actions.is_empty() {
            self.tray_updated = Instant::now();
            let active = self.snap.count(Status::is_active);
            let tip = match active {
                0 => "StewardGet \u{2014} idle".to_owned(),
                n => format!("StewardGet \u{2014} {n} downloading, {}", format_speed(self.snap.speed)),
            };
            tray.set_tooltip(tip);
        }
        for action in actions {
            match action {
                crate::tray::TrayAction::Show => self.show_window(ctx),
                crate::tray::TrayAction::Add => {
                    self.show_window(ctx);
                    self.page = Page::Downloads;
                    if self.add.is_none() {
                        self.open_add(None);
                    }
                }
                crate::tray::TrayAction::PauseAll => self.engine.pause_all(),
                crate::tray::TrayAction::ResumeAll => self.engine.resume_all(),
                crate::tray::TrayAction::Quit => self.quit(ctx),
            }
        }
    }
}

/// Where a small window of `size` opens: centred on the screen a little above
/// the middle, like IDM's, and `nth` steps down and right of the previous ones
/// so several don't hide each other.
pub fn screen_spot(ctx: &egui::Context, size: egui::Vec2, nth: usize) -> egui::Pos2 {
    let screen = ctx.input(|i| i.viewport().monitor_size).unwrap_or(egui::vec2(1366.0, 768.0));
    let step = 28.0 * (nth % 6) as f32;
    egui::pos2(((screen.x - size.x) / 2.0).max(0.0) + step, ((screen.y - size.y) * 0.4).max(0.0) + step)
}

impl StewardGetApp {
    /// Brings the main window up on the download list with `id` selected.
    pub(crate) fn show_in_list(&mut self, ctx: &egui::Context, id: DownloadId) {
        self.page = Page::Downloads;
        self.filter = ListFilter::All;
        self.search.clear();
        self.selection = std::iter::once(id).collect();
        self.anchor = Some(id);
        self.show_details = true;
        self.show_window(ctx);
    }

    /// Restores and focuses the main window, from the tray or the taskbar.
    fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Puts the main window away: into the tray when there is one (and the
    /// setting allows), otherwise onto the taskbar.
    pub(crate) fn park(&mut self, ctx: &egui::Context) {
        if self.tray.is_some() && self.cfg.minimize_to_tray {
            self.hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        } else {
            // Visible first: a login start without a tray begins hidden.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }
}

impl eframe::App for StewardGetApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.engine.config().theme == Theme::System || self.applied_look.is_none() {
            self.apply_appearance(&ctx);
        }
        self.handle_close(&ctx);
        self.dropped_files(&ctx);
        self.shortcuts(&ctx);
        let p = self.palette;
        let width = ui.available_width();
        let compact = width < 1180.0;
        let collapsed = self.sidebar_pref.unwrap_or(width < 940.0);

        egui::Panel::top("stewardget-menubar")
            .exact_size(34.0)
            .frame(Frame::new().fill(p.sidebar).inner_margin(Margin::symmetric(8, 3)))
            .show(ui, |ui| ui.push_id("menubar", |ui| self.menu_bar(ui)));

        egui::Panel::top("stewardget-toolbar")
            .exact_size(44.0)
            .frame(
                Frame::new().fill(p.sidebar).inner_margin(Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| ui.push_id("toolbar", |ui| self.toolbar(ui, compact, collapsed)));

        egui::Panel::bottom("stewardget-status")
            .exact_size(28.0)
            .frame(
                Frame::new().fill(p.sidebar).inner_margin(Margin::symmetric(12, 0)).stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| ui.push_id("status", |ui| self.status_bar(ui)));

        egui::Panel::left("stewardget-nav")
            .exact_size(if collapsed { 56.0 } else { 204.0 })
            .resizable(false)
            .frame(
                Frame::new().fill(p.sidebar).inner_margin(Margin::symmetric(6, 8)).stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| ui.push_id("nav", |ui| self.sidebar(ui, collapsed)));

        if self.page == Page::Downloads && self.show_details && !self.selection.is_empty() {
            egui::Panel::bottom("stewardget-details")
                .resizable(true)
                .default_size(262.0)
                .size_range(160.0..=540.0)
                .frame(
                    Frame::new()
                        .fill(p.sidebar)
                        .inner_margin(Margin::symmetric(12, 8))
                        .stroke(Stroke::new(1.0, p.border)),
                )
                .show(ui, |ui| ui.push_id("details", |ui| self.details_pane(ui)));
        }

        egui::CentralPanel::no_frame().frame(Frame::new().fill(p.bg).inner_margin(Margin::ZERO)).show(ui, |ui| {
            ui.push_id("central", |ui| match self.page {
                Page::Overview => self.overview_page(ui),
                Page::Downloads => self.downloads_page(ui),
                Page::History => self.history_page(ui),
                Page::Stats => self.stats_page(ui),
                Page::Tools => self.tools_page(ui),
                Page::Settings => self.settings_page(ui),
            })
        });

        self.add_dialog(&ctx);
        self.confirm_dialog(&ctx);
        self.restart_dialog(&ctx);
        self.refresh_dialog(&ctx);
        self.properties_dialog(&ctx);
        self.power_dialog(&ctx);
        self.welcome_dialog(&ctx);
        self.queue_dialog(&ctx);
        self.category_dialog(&ctx);
        self.grab_dialog(&ctx);
        self.media_dialog(&ctx);
        self.delete_queue_dialog(&ctx);
        self.close_prompt(&ctx);
        self.capture_window(&ctx);
        self.shortcuts_dialog(&ctx);
        self.progress_windows(&ctx);
        self.drag_overlay(&ctx);
        self.draw_toasts(&ctx);
    }

    /// Runs every pass, also while the window is minimised (when `ui` is skipped),
    /// so finished downloads, "when done" actions and show requests are never missed.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // At most ten snapshots a second: moving the mouse repaints far more often.
        let fresh = self.snap_taken.elapsed() >= Duration::from_millis(100);
        if fresh {
            self.snap = self.engine.snapshot();
            self.snap_gen += 1;
            self.snap_taken = Instant::now();
        }
        self.cfg = self.engine.config();
        let minimized =
            ctx.input(|i| i.raw.viewports.get(&egui::ViewportId::ROOT).and_then(|v| v.minimized).unwrap_or(false));
        if std::mem::take(&mut self.start_minimized) {
            self.park(ctx);
        } else if let Some(hwnd) = self.cloaked.filter(|_| self.hidden || minimized) {
            // Put away: it can be shown normally from now on.
            windows_hwnd::uncloak(hwnd);
            self.cloaked = None;
        } else if minimized && !self.hidden {
            // The minimise button: go to the tray instead of the taskbar.
            if self.tray.is_some() && self.cfg.minimize_to_tray {
                self.park(ctx);
            }
        }
        if !self.fitted && !self.hidden && !minimized {
            self.fitted = fit_to_work_area(ctx);
        }
        if self.show_requested.swap(false, Ordering::SeqCst) {
            self.show_window(ctx);
        }
        if self.quit_requested.swap(false, Ordering::SeqCst) {
            self.quit(ctx);
        }
        self.next_capture();
        if fresh {
            self.track_transitions(ctx);
        }
        // The small windows (new download, progress) are drawn from the main
        // window's pass, which eframe skips while that window is away. Ask for
        // the pass anyway, so they appear on their own, like IDM's, and the
        // main window stays where it is.
        let small_windows = self.capture.is_some() || !self.progress.is_empty();
        let away = self.hidden || minimized;
        if away && small_windows {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
        self.watch_clipboard(ctx);
        self.handle_tray(ctx);
        let counting = self.power_countdown.is_some();
        self.watch_queue_end();
        if !counting && self.power_countdown.is_some() {
            // Show the countdown so it can be seen and cancelled.
            self.show_window(ctx);
        }
        let busy = self.snap.downloads.iter().any(|d| d.status.is_active() || d.retry_in.is_some());
        // Out of sight there is little to redraw: wake rarely, and only about
        // once a second while the clipboard is watched. A progress window on
        // screen counts as in sight.
        let away = away && !small_windows;
        let wait = match (busy, away) {
            (true, false) => 120,
            (true, true) => 500,
            (false, false) => 600,
            (false, true) if self.cfg.clipboard_monitor => 1000,
            (false, true) => 5000,
        };
        ctx.request_repaint_after(Duration::from_millis(wait));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The browser already let go of downloads waiting in the new-download
        // window: keep single ones, paused, rather than lose them.
        let mut waiting: Vec<Vec<stewardget_core::NewDownload>> = self.inbox.lock().unwrap().drain(..).collect();
        if let Some(w) = self.capture.take() {
            waiting.push(w.into_downloads());
        }
        for batch in waiting.into_iter().filter(|b| b.len() == 1) {
            for mut new in batch {
                new.paused = true;
                let _ = self.engine.add(new);
            }
        }
        self.runtime.block_on(self.engine.shutdown());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_links() {
        let config = stewardget_core::Config::default();
        let file = |t: &str| super::looks_like_file_link(t, &config);
        assert!(file("https://example.com/a/b/setup.exe?x=1"));
        assert!(file("http://cdn.example.com/movie.mkv"));
        assert!(!file("https://example.com/page.html"));
        assert!(!file("https://example.com/"));
        assert!(!file("see https://example.com/a.zip"));
        // Extensions of custom categories count as files too.
        let config = stewardget_core::Config {
            custom_categories: vec![stewardget_core::CustomCategory {
                id: 1,
                name: "Fonts".into(),
                extensions: vec!["woff2".into()],
            }],
            ..stewardget_core::Config::default()
        };
        assert!(super::looks_like_file_link("https://example.com/inter.woff2", &config));
    }
}
