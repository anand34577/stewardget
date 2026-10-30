use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Category;
use crate::category::CustomCategory;

/// Default port of the local browser-integration API.
pub const DEFAULT_API_PORT: u16 = 7853;

/// Front-end colour scheme preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Light,
    Dark,
    /// True black, for OLED screens.
    Midnight,
    /// Deep blue dark theme.
    Ocean,
}

impl Theme {
    pub const ALL: [Theme; 5] = [Theme::System, Theme::Light, Theme::Dark, Theme::Midnight, Theme::Ocean];

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
            Theme::Midnight => "Midnight",
            Theme::Ocean => "Ocean",
        }
    }
}

/// Highlight colour of the graphical front-end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Violet,
    Blue,
    #[default]
    Teal,
    Green,
    Amber,
    Rose,
}

impl Accent {
    pub const ALL: [Accent; 6] =
        [Accent::Violet, Accent::Blue, Accent::Teal, Accent::Green, Accent::Amber, Accent::Rose];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Violet => "Violet",
            Accent::Blue => "Blue",
            Accent::Teal => "Teal",
            Accent::Green => "Green",
            Accent::Amber => "Amber",
            Accent::Rose => "Rose",
        }
    }
}

/// Identifies a [`Queue`]. The main queue is [`MAIN_QUEUE`].
pub type QueueId = u32;

/// The queue that always exists and that downloads join unless told otherwise.
pub const MAIN_QUEUE: QueueId = 0;

/// Most recently used download folders remembered for the add dialogs.
const RECENT_DIRS: usize = 8;

/// A named download queue with its own limit and schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub id: QueueId,
    pub name: String,
    /// How many of this queue's downloads may transfer at once; `0` leaves it
    /// to the global limit ([`Config::max_concurrent`]).
    pub max_concurrent: usize,
    /// Bandwidth shared by this queue's downloads, bytes/second; `0` = no own cap.
    pub speed_limit: u64,
    /// Starts and pauses this queue's downloads at set times.
    pub schedule: Schedule,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            id: MAIN_QUEUE,
            name: "Main queue".into(),
            max_concurrent: 0,
            speed_limit: 0,
            schedule: Schedule::default(),
        }
    }
}

/// What the window's close button does (desktop app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseAction {
    /// Ask the first time, then remember.
    #[default]
    Ask,
    /// Minimise and keep running, so browser downloads still reach StewardGet.
    Background,
    Quit,
}

/// Time-of-day window in which the queue runs automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    /// Start the queue at this local time, minutes after midnight.
    pub start_minute: u16,
    /// Pause everything at this local time, minutes after midnight.
    pub stop_minute: Option<u16>,
    /// Active weekdays, Monday first.
    pub days: [bool; 7],
}

impl Default for Schedule {
    fn default() -> Self {
        Self { enabled: false, start_minute: 2 * 60, stop_minute: Some(7 * 60), days: [true; 7] }
    }
}

/// What happens when a finished download's name is already taken in its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExistingFile {
    /// Keep both: the new file gets a numbered name, `setup (1).exe`.
    #[default]
    Rename,
    /// Replace the existing file.
    Overwrite,
}

/// A different global speed limit for set hours, e.g. to keep the connection
/// usable during work hours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeedSchedule {
    pub enabled: bool,
    /// Bytes/second while the schedule is active; `0` = unlimited.
    pub limit: u64,
    /// Local time the limit starts, minutes after midnight.
    pub start_minute: u16,
    /// Local time the limit ends; before `start_minute` means the next morning.
    pub end_minute: u16,
    /// Days the limit starts on, Monday first.
    pub days: [bool; 7],
}

impl Default for SpeedSchedule {
    fn default() -> Self {
        Self { enabled: false, limit: 512 * 1024, start_minute: 9 * 60, end_minute: 18 * 60, days: [true; 7] }
    }
}

impl SpeedSchedule {
    /// Whether the limit applies at `minute` on `weekday` (Monday = 0).
    pub fn is_active(&self, weekday: usize, minute: u16) -> bool {
        if !self.enabled || self.start_minute == self.end_minute {
            return false;
        }
        let (start, end) = (self.start_minute, self.end_minute);
        if start < end {
            self.days[weekday % 7] && (start..end).contains(&minute)
        } else {
            // Runs past midnight: tonight's part, or the morning after an active day.
            (self.days[weekday % 7] && minute >= start) || (self.days[(weekday + 6) % 7] && minute < end)
        }
    }
}

impl Schedule {
    /// Whether the stop time applies on `weekday` (Monday = 0). A window that
    /// runs past midnight (23:00–07:00) stops on the morning after an active
    /// day, so Friday night still stops on Saturday when Saturday is off.
    pub fn stops_on(&self, weekday: usize) -> bool {
        match self.stop_minute {
            Some(stop) if stop <= self.start_minute => self.days[(weekday + 6) % 7],
            _ => self.days[weekday % 7],
        }
    }
}

/// User-tunable engine settings. Persisted as `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where new downloads are saved unless a directory is given explicitly.
    pub download_dir: PathBuf,
    /// Sort downloads into sub-folders such as `Video/` or `Archives/`.
    pub auto_categorize: bool,
    /// How many downloads may transfer at the same time.
    pub max_concurrent: usize,
    /// Parallel connections per download.
    pub connections: usize,
    /// Never split a range below this size (bytes).
    pub min_split_size: u64,
    /// Global bandwidth cap in bytes/second; `0` means unlimited.
    pub speed_limit: u64,
    /// Retry attempts per connection before giving up.
    pub max_retries: u32,
    /// Connection timeout, seconds.
    pub connect_timeout_secs: u64,
    /// Abort a connection that delivers no data for this long, seconds.
    pub read_timeout_secs: u64,
    /// `User-Agent` header sent with every request.
    pub user_agent: String,
    /// Optional proxy URL (`http://`, `https://`, `socks5://`).
    pub proxy: Option<String>,
    /// Force HTTP/1.1 so every segment gets its own TCP connection. HTTP/2
    /// would multiplex all segments over one connection and lose most of the
    /// benefit of segmentation.
    pub http1_only: bool,
    /// Resume unfinished downloads automatically on start-up.
    pub resume_on_start: bool,
    /// Accept invalid TLS certificates. Dangerous; off by default.
    pub insecure_tls: bool,
    /// UI theme for the graphical front-ends.
    pub theme: Theme,
    /// Accept downloads from the browser extension on `127.0.0.1:api_port`.
    pub api_enabled: bool,
    /// Local port for the browser integration API.
    pub api_port: u16,
    /// Optional shared secret the extension must send (`Authorization: Bearer …`).
    pub api_token: Option<String>,
    /// Offer to download file links copied to the clipboard (desktop app).
    pub clipboard_monitor: bool,
    /// Look for a newer release on startup (one request to the project's release page). Off by default.
    pub check_updates: bool,
    /// Accent colour of the desktop app.
    pub accent: Accent,
    /// Interface zoom of the desktop app (1.0 = 100 %).
    pub ui_scale: f32,
    /// Use the operating system's window title bar instead of StewardGet's
    /// own (which holds the menus, search and window buttons). Takes effect
    /// on the next start.
    pub system_title_bar: bool,
    /// Messages to other devices (Gotify, ntfy, email) when downloads finish or fail.
    pub remote_notify: crate::notifiers::RemoteNotify,
    /// How often a failed download is retried automatically (network
    /// problems and server errors only). `0` disables it.
    pub auto_retry: u32,
    /// Delay before the first automatic retry, seconds; it doubles each time.
    pub auto_retry_delay_secs: u64,
    /// Schedule of StewardGet 0.2 and earlier, which had a single queue. It is moved
    /// into the main queue on load and no longer written.
    #[serde(skip_serializing)]
    pub schedule: Schedule,
    /// Download queues, the main queue first. Each has its own limit and schedule.
    pub queues: Vec<Queue>,
    /// Show the new-download window for downloads from the browser (desktop
    /// app), so name, folder and queue can be checked before it starts.
    pub capture_dialog: bool,
    /// What the window's close button does (desktop app).
    pub close_action: CloseAction,
    /// Folders recently chosen in the add dialogs, newest first.
    pub recent_dirs: Vec<PathBuf>,
    /// Custom folder per category; falls back to `download_dir` (+ sub-folder
    /// when `auto_categorize` is on).
    pub category_dirs: BTreeMap<Category, PathBuf>,
    /// Show a dialog when a download finishes (desktop app).
    pub completion_dialog: bool,
    /// Categories defined by the user, checked before the built-in ones.
    pub custom_categories: Vec<CustomCategory>,
    /// A different global speed limit for set hours.
    pub speed_schedule: SpeedSchedule,
    /// What to do when a finished file's name is already taken.
    pub existing_file: ExistingFile,
    /// Program to run after each successful download, e.g. a virus scanner.
    /// `{path}`, `{dir}`, `{name}` and `{url}` are replaced in each argument;
    /// no shell is involved, so file names can't inject commands.
    pub on_complete_command: Option<String>,
    /// System notifications when downloads finish or fail (desktop app).
    pub notifications: bool,
    /// Serve the Model Context Protocol on the local API (`/mcp`), so AI
    /// assistants can manage downloads. Needs `mcp_token`.
    pub mcp_enabled: bool,
    /// Bearer token MCP clients must send; created when MCP is switched on.
    pub mcp_token: Option<String>,
    /// Secret the command line and the installer use for the parts of the local
    /// API the browser extension doesn't need (listing, pausing, quitting…).
    /// Created automatically; never shown.
    pub control_token: Option<String>,
    /// Play the system sound when a download finishes while StewardGet is in the
    /// background (desktop app).
    pub completion_sound: bool,
    /// Minimise to the tray instead of the taskbar (desktop app).
    pub minimize_to_tray: bool,
    /// One-line rows in the download list instead of name plus details (desktop app).
    pub compact_rows: bool,
    /// Show downloads as cards with live figures instead of the list (desktop
    /// app). Off by default; a separate setting so `compact_rows` keeps its meaning.
    pub card_view: bool,
    /// Optional columns of the download list the user switched off: `size`,
    /// `speed`, `eta`, `added` (desktop app).
    pub hidden_columns: Vec<String>,
    /// Open a progress window for each download started on its own, like
    /// IDM (desktop app). Batches don't open one window per file.
    pub progress_window: bool,
    /// Which generation of default look the settings were saved with (files
    /// from before it existed read as 0).
    #[serde(default = "look_zero")]
    pub look_version: u8,
    /// Start a download with a few connections and add more while the speed
    /// still improves, up to `connections`. A download with its own connection
    /// count is never tuned.
    pub auto_tune: bool,
    /// Spread connections over several network adapters at once (Wi-Fi plus
    /// Ethernet, a phone's hotspot…), see `interfaces`.
    pub multi_link: bool,
    /// Adapters known to the user; used when `multi_link` is on.
    pub interfaces: Vec<crate::NetworkLink>,
    /// What to do with each finished file: unpack, scan, move, run a program,
    /// tag. Rules run in order, in the background.
    pub post_actions: Vec<crate::PostRule>,
    /// Where yt-dlp is, when it isn't on `PATH`.
    pub ytdlp_path: Option<PathBuf>,
    /// Where ffmpeg is, when it isn't on `PATH`; used to join video and audio.
    pub ffmpeg_path: Option<PathBuf>,
    /// Stop downloading for the rest of the calendar month once this many
    /// bytes have been downloaded in it (a metered or hotspot connection); `0` = no cap.
    pub monthly_cap: u64,
}

fn look_zero() -> u8 {
    0
}

/// The default `User-Agent`: browser-shaped, because many servers answer
/// anything else with a 403, plus the app name.
pub fn default_user_agent() -> String {
    let platform = if cfg!(windows) {
        "Windows NT 10.0; Win64; x64"
    } else if cfg!(target_os = "macos") {
        "Macintosh; Intel Mac OS X 10_15_7"
    } else {
        "X11; Linux x86_64"
    };
    format!(
        "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 StewardGet/{}",
        env!("CARGO_PKG_VERSION")
    )
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            auto_categorize: false,
            max_concurrent: 3,
            connections: 8,
            min_split_size: 1024 * 1024,
            speed_limit: 0,
            max_retries: 10,
            connect_timeout_secs: 15,
            read_timeout_secs: 30,
            user_agent: default_user_agent(),
            proxy: None,
            http1_only: true,
            resume_on_start: true,
            insecure_tls: false,
            theme: Theme::Dark,
            api_enabled: true,
            api_port: DEFAULT_API_PORT,
            api_token: None,
            clipboard_monitor: true,
            check_updates: false,
            accent: Accent::Teal,
            ui_scale: 1.0,
            system_title_bar: false,
            remote_notify: crate::notifiers::RemoteNotify::default(),
            auto_retry: 3,
            auto_retry_delay_secs: 20,
            schedule: Schedule::default(),
            queues: vec![Queue::default()],
            capture_dialog: true,
            close_action: CloseAction::Ask,
            recent_dirs: Vec::new(),
            category_dirs: BTreeMap::new(),
            completion_dialog: false,
            custom_categories: Vec::new(),
            speed_schedule: SpeedSchedule::default(),
            existing_file: ExistingFile::Rename,
            on_complete_command: None,
            notifications: true,
            mcp_enabled: false,
            mcp_token: None,
            control_token: None,
            completion_sound: true,
            minimize_to_tray: true,
            compact_rows: false,
            card_view: false,
            hidden_columns: Vec::new(),
            progress_window: true,
            look_version: 1,
            auto_tune: true,
            multi_link: false,
            interfaces: Vec::new(),
            post_actions: Vec::new(),
            ytdlp_path: None,
            ffmpeg_path: None,
            monthly_cap: 0,
        }
    }
}

/// `StewardGet/0.2.0`, or a default of an earlier release: not something the user typed.
/// Releases before 0.7 went by the name Velox.
fn is_stock_user_agent(ua: &str) -> bool {
    let ua = ua.trim();
    ["StewardGet/", "Velox/"].iter().any(|name| {
        ua.starts_with(name) || (ua.starts_with("Mozilla/5.0 (") && ua.contains(&format!(" Safari/537.36 {name}")))
    })
}

impl Config {
    /// Clamps every value into a sane range so a hand-edited file can't break the engine.
    pub fn sanitized(mut self) -> Self {
        self.max_concurrent = self.max_concurrent.clamp(1, 64);
        self.connections = self.connections.clamp(1, 64);
        self.min_split_size = self.min_split_size.clamp(64 * 1024, 1 << 30);
        self.max_retries = self.max_retries.min(1000);
        self.connect_timeout_secs = self.connect_timeout_secs.clamp(1, 600);
        self.read_timeout_secs = self.read_timeout_secs.clamp(1, 3600);
        // An untouched default follows the app version; a custom one is kept.
        if self.user_agent.trim().is_empty() || is_stock_user_agent(&self.user_agent) {
            self.user_agent = default_user_agent();
        }
        if self.proxy.as_deref().is_some_and(|p| p.trim().is_empty()) {
            self.proxy = None;
        }
        if self.api_token.as_deref().is_some_and(|t| t.trim().is_empty()) {
            self.api_token = None;
        }
        if self.look_version < 1 {
            // The new default accent differs from the app icon's violet.
            if self.accent == Accent::Violet {
                self.accent = Accent::Teal;
            }
            self.look_version = 1;
        }
        if self.mcp_token.as_deref().is_some_and(|t| t.trim().len() < 16) {
            self.mcp_token = None;
        }
        if self.control_token.as_deref().is_none_or(|t| t.trim().len() < 16) {
            self.control_token = Some(crate::util::random_token());
        }
        if self.mcp_enabled && self.mcp_token.is_none() {
            self.mcp_token = Some(crate::util::random_token());
        }
        self.ui_scale = if self.ui_scale.is_finite() { self.ui_scale.clamp(0.7, 2.0) } else { 1.0 };
        self.auto_retry = self.auto_retry.min(100);
        self.auto_retry_delay_secs = self.auto_retry_delay_secs.clamp(1, 3600);
        // The main queue always exists and comes first.
        let mut seen = std::collections::BTreeSet::new();
        self.queues.retain(|q| seen.insert(q.id));
        if !self.queues.iter().any(|q| q.id == MAIN_QUEUE) {
            self.queues.insert(0, Queue::default());
        }
        self.queues.sort_by_key(|q| q.id != MAIN_QUEUE);
        // Only a 0.2 settings file can fill `schedule`, since it is never written.
        let legacy = std::mem::take(&mut self.schedule);
        if legacy != Schedule::default() {
            self.queues[0].schedule = legacy;
        }
        for q in &mut self.queues {
            q.name = q.name.trim().chars().take(60).collect();
            if q.name.is_empty() {
                q.name = if q.id == MAIN_QUEUE { "Main queue".into() } else { format!("Queue {}", q.id) };
            }
            q.max_concurrent = q.max_concurrent.min(64);
            q.schedule.start_minute = q.schedule.start_minute.min(24 * 60 - 1);
            q.schedule.stop_minute = q.schedule.stop_minute.map(|m| m.min(24 * 60 - 1));
        }
        self.recent_dirs.retain(|d| !d.as_os_str().is_empty());
        let mut seen = std::collections::BTreeSet::new();
        self.recent_dirs.retain(|d| seen.insert(d.clone()));
        self.recent_dirs.truncate(RECENT_DIRS);
        for dir in self.category_dirs.values_mut() {
            if let Some(s) = dir.to_str().filter(|s| s.trim() != *s) {
                *dir = PathBuf::from(s.trim());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        self.custom_categories.retain(|c| c.id > 0 && seen.insert(c.id));
        for c in &mut self.custom_categories {
            c.name = c.name.trim().chars().take(40).collect();
            if c.name.is_empty() {
                c.name = format!("Category {}", c.id);
            }
            c.extensions = CustomCategory::parse_extensions(&c.extensions.join(","));
        }
        let ids: Vec<u32> = self.custom_categories.iter().map(|c| c.id).collect();
        // Folders of deleted custom categories go with them.
        self.category_dirs
            .retain(|c, dir| !dir.as_os_str().is_empty() && !matches!(c, Category::Custom(id) if !ids.contains(id)));
        if self.on_complete_command.as_deref().is_some_and(|c| c.trim().is_empty()) {
            self.on_complete_command = None;
        }
        self.speed_schedule.start_minute = self.speed_schedule.start_minute.min(24 * 60 - 1);
        self.speed_schedule.end_minute = self.speed_schedule.end_minute.min(24 * 60 - 1);
        if self.api_port < 1024 {
            self.api_port = DEFAULT_API_PORT;
        }
        self
    }
}

impl Config {
    /// The queue with this id, or the main queue when it no longer exists.
    pub fn queue(&self, id: QueueId) -> &Queue {
        static MAIN: std::sync::OnceLock<Queue> = std::sync::OnceLock::new();
        self.queues
            .iter()
            .find(|q| q.id == id)
            .or_else(|| self.queues.iter().find(|q| q.id == MAIN_QUEUE))
            .unwrap_or_else(|| MAIN.get_or_init(Queue::default))
    }

    /// An id no queue uses yet.
    pub fn next_queue_id(&self) -> QueueId {
        self.queues.iter().map(|q| q.id).max().unwrap_or(MAIN_QUEUE) + 1
    }

    /// The category of a file name: the first custom category listing its
    /// extension, otherwise the built-in one.
    pub fn categorize(&self, file_name: &str) -> Category {
        let lower = file_name.to_lowercase();
        self.custom_categories
            .iter()
            .find(|c| c.matches(&lower))
            .map_or_else(|| Category::from_file_name(file_name), |c| Category::Custom(c.id))
    }

    /// Every category in sidebar order: built-in ones, custom ones, `Other` last.
    pub fn categories(&self) -> Vec<Category> {
        let mut all: Vec<Category> = Category::ALL.into_iter().filter(|c| *c != Category::Other).collect();
        all.extend(self.custom_categories.iter().map(|c| Category::Custom(c.id)));
        all.push(Category::Other);
        all
    }

    /// Display name of a category (a deleted custom one reads as "Other").
    pub fn category_label(&self, category: Category) -> String {
        match category {
            Category::Custom(id) => self
                .custom_categories
                .iter()
                .find(|c| c.id == id)
                .map_or_else(|| Category::Other.label().to_owned(), |c| c.name.clone()),
            c => c.label().to_owned(),
        }
    }

    /// Sub-folder used when "sort into category folders" is on; `Other` stays in the root.
    pub fn category_folder(&self, category: Category) -> Option<String> {
        match category {
            Category::Custom(id) => {
                self.custom_categories.iter().find(|c| c.id == id).map(|c| crate::filename::sanitize(&c.name))
            }
            c => c.folder().map(str::to_owned),
        }
    }

    /// An id no custom category uses yet.
    pub fn next_category_id(&self) -> u32 {
        self.custom_categories.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    /// The global speed limit in force at this local time.
    pub fn effective_speed_limit(&self, weekday: usize, minute: u16) -> u64 {
        if self.speed_schedule.is_active(weekday, minute) { self.speed_schedule.limit } else { self.speed_limit }
    }

    /// Puts `dir` at the front of the recently used folders.
    pub fn remember_dir(&mut self, dir: PathBuf) {
        if dir.as_os_str().is_empty() {
            return;
        }
        self.recent_dirs.retain(|d| d != &dir);
        self.recent_dirs.insert(0, dir);
        self.recent_dirs.truncate(RECENT_DIRS);
    }
}

fn default_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(PathBuf::from).or_else(|| Some(u.home_dir().join("Downloads"))))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_schedule_moves_into_the_main_queue() {
        let old = r#"{ "schedule": { "enabled": true, "start_minute": 60 } }"#;
        let config = serde_json::from_str::<Config>(old).unwrap().sanitized();
        assert_eq!(config.queues.len(), 1);
        assert!(config.queues[0].schedule.enabled);
        assert_eq!(config.queues[0].schedule.start_minute, 60);
        // Saving and loading again keeps it, now in the queue.
        let again = serde_json::from_str::<Config>(&serde_json::to_string(&config).unwrap()).unwrap().sanitized();
        assert_eq!(again, config);
    }

    #[test]
    fn queues_are_cleaned_up() {
        let config = Config {
            queues: vec![
                Queue { id: 3, name: "  Night  ".into(), max_concurrent: 500, ..Queue::default() },
                Queue { id: 3, name: "dupe".into(), ..Queue::default() },
                Queue { id: 0, name: String::new(), ..Queue::default() },
            ],
            ..Config::default()
        }
        .sanitized();
        let names: Vec<_> = config.queues.iter().map(|q| (q.id, q.name.as_str())).collect();
        assert_eq!(names, [(0, "Main queue"), (3, "Night")]);
        assert_eq!(config.queues[1].max_concurrent, 64);
        assert_eq!(config.queue(42).id, MAIN_QUEUE);
        assert_eq!(config.next_queue_id(), 4);
    }

    #[test]
    fn overnight_windows_stop_the_morning_after() {
        let mut days = [false; 7];
        days[4] = true; // Friday only
        let night = Schedule { enabled: true, start_minute: 23 * 60, stop_minute: Some(7 * 60), days };
        assert!(night.stops_on(5), "Friday night stops on Saturday morning");
        assert!(!night.stops_on(4), "no window ran into Friday morning");
        let day = Schedule { start_minute: 9 * 60, stop_minute: Some(17 * 60), ..night };
        assert!(day.stops_on(4) && !day.stops_on(5));
    }

    #[test]
    fn custom_categories_come_first_and_get_cleaned() {
        let mut config = Config {
            custom_categories: vec![
                CustomCategory { id: 2, name: "  Disk images ".into(), extensions: vec![".ISO".into(), "vhdx".into()] },
                CustomCategory { id: 2, name: "dupe".into(), extensions: vec![] },
                CustomCategory { id: 0, name: "bad id".into(), extensions: vec![] },
            ],
            ..Config::default()
        };
        config.category_dirs.insert(Category::Custom(9), PathBuf::from("/gone"));
        config.category_dirs.insert(Category::Custom(2), PathBuf::from("/images"));
        let config = config.sanitized();
        assert_eq!(config.custom_categories.len(), 1);
        assert_eq!(config.custom_categories[0].name, "Disk images");
        assert_eq!(config.custom_categories[0].extensions, ["iso", "vhdx"]);
        assert_eq!(config.categorize("Ubuntu.ISO"), Category::Custom(2));
        assert_eq!(config.categorize("movie.mkv"), Category::Video);
        assert_eq!(config.category_label(Category::Custom(2)), "Disk images");
        assert_eq!(config.category_label(Category::Custom(5)), "Other");
        assert_eq!(config.category_folder(Category::Custom(2)).as_deref(), Some("Disk images"));
        assert_eq!(*config.categories().last().unwrap(), Category::Other);
        assert!(config.categories().contains(&Category::Custom(2)));
        assert!(!config.category_dirs.contains_key(&Category::Custom(9)), "folders of deleted categories go");
        assert!(config.category_dirs.contains_key(&Category::Custom(2)));
        assert_eq!(config.next_category_id(), 3);
        // Survives a save and load.
        let again = serde_json::from_str::<Config>(&serde_json::to_string(&config).unwrap()).unwrap().sanitized();
        assert_eq!(again, config);
    }

    #[test]
    fn speed_schedule_windows() {
        let mut days = [false; 7];
        days[0] = true; // Monday
        let s = SpeedSchedule { enabled: true, limit: 1, start_minute: 9 * 60, end_minute: 17 * 60, days };
        assert!(s.is_active(0, 9 * 60) && s.is_active(0, 16 * 60 + 59));
        assert!(!s.is_active(0, 17 * 60) && !s.is_active(1, 10 * 60));
        let night = SpeedSchedule { start_minute: 22 * 60, end_minute: 6 * 60, ..s.clone() };
        assert!(night.is_active(0, 23 * 60) && night.is_active(1, 5 * 60));
        assert!(!night.is_active(1, 23 * 60) && !night.is_active(0, 5 * 60));
        let config = Config { speed_limit: 7, speed_schedule: s, ..Config::default() };
        assert_eq!(config.effective_speed_limit(0, 10 * 60), 1);
        assert_eq!(config.effective_speed_limit(0, 20 * 60), 7);
    }

    #[test]
    fn category_dirs_are_trimmed_on_save() {
        let mut config = Config::default();
        config.category_dirs.insert(Category::Video, PathBuf::from("  /My Videos "));
        config.category_dirs.insert(Category::Audio, PathBuf::from("   "));
        let config = config.sanitized();
        assert_eq!(config.category_dirs.get(&Category::Video), Some(&PathBuf::from("/My Videos")));
        assert!(!config.category_dirs.contains_key(&Category::Audio));
    }

    #[test]
    fn recent_dirs_are_unique_and_short() {
        let mut config = Config::default();
        for i in 0..12 {
            config.remember_dir(PathBuf::from(format!("/d{}", i % 10)));
        }
        config.remember_dir(PathBuf::from("/d5"));
        assert_eq!(config.recent_dirs.len(), RECENT_DIRS);
        assert_eq!(config.recent_dirs[0], PathBuf::from("/d5"));
        assert_eq!(config.recent_dirs.iter().filter(|d| d.as_path() == std::path::Path::new("/d5")).count(), 1);
    }

    #[test]
    fn stock_user_agents_follow_the_version_but_custom_ones_stay() {
        let old = Config { user_agent: "StewardGet/0.2.0".into(), ..Config::default() }.sanitized();
        assert_eq!(old.user_agent, default_user_agent());
        for velox in [
            "Velox/0.2.0",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Velox/0.6.0",
        ] {
            let old = Config { user_agent: velox.into(), ..Config::default() }.sanitized();
            assert_eq!(old.user_agent, default_user_agent());
        }
        let custom = Config { user_agent: "MyAgent/1".into(), ..Config::default() }.sanitized();
        assert_eq!(custom.user_agent, "MyAgent/1");
        assert!(default_user_agent().starts_with("Mozilla/5.0 ("));
    }

    #[test]
    fn a_control_token_is_always_there() {
        let config = Config::default().sanitized();
        let token = config.control_token.clone().expect("created");
        assert!(token.len() >= 16);
        assert_eq!(config.sanitized().control_token, Some(token), "kept once made");
    }
}
