//! Interactive terminal dashboard.

mod ui;

use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::Context;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use stewardget_core::{DownloadView, Engine, EngineOptions, EngineSnapshot, NewDownload, Status};
use tokio::runtime::Runtime;

pub fn run(rt: &Runtime) -> anyhow::Result<()> {
    let engine = {
        let _guard = rt.enter();
        Engine::start(EngineOptions::persistent()).context("could not start the download engine")?
    };
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    let result = App::new(engine.clone()).run(&mut terminal);
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    rt.block_on(engine.shutdown());
    result
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Active,
    Queued,
    Completed,
    Failed,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::Active, Filter::Queued, Filter::Completed, Filter::Failed];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Active => "Active",
            Filter::Queued => "Waiting",
            Filter::Completed => "Completed",
            Filter::Failed => "Failed",
        }
    }

    pub fn matches(self, s: Status) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => s.is_active(),
            Filter::Queued => matches!(s, Status::Queued | Status::Paused),
            Filter::Completed => s == Status::Completed,
            Filter::Failed => s == Status::Failed,
        }
    }
}

pub enum InputKind {
    AddUrls,
    GlobalLimit,
    DownloadLimit(u64),
    NewUrl(u64),
}

pub enum Mode {
    Normal,
    Input { kind: InputKind, buffer: String },
    Confirm { id: u64, name: String, delete_file: bool },
    Help,
}

pub struct App {
    engine: Engine,
    pub snap: EngineSnapshot,
    pub filter: Filter,
    pub selected: Option<u64>,
    pub mode: Mode,
    pub toast: Option<(String, bool, Instant)>,
    quit: bool,
}

impl App {
    fn new(engine: Engine) -> Self {
        let snap = engine.snapshot();
        let selected = snap.downloads.first().map(|d| d.id);
        Self { engine, snap, filter: Filter::All, selected, mode: Mode::Normal, toast: None, quit: false }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.quit {
            self.snap = self.engine.snapshot();
            self.fix_selection();
            terminal.draw(|f| ui::draw(f, &self))?;
            if event::poll(Duration::from_millis(150))? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
                    Event::Paste(text) => self.on_paste(&text),
                    _ => {}
                }
            }
            if self.toast.as_ref().is_some_and(|(_, _, t)| t.elapsed() > Duration::from_secs(4)) {
                self.toast = None;
            }
        }
        Ok(())
    }

    pub fn visible(&self) -> Vec<&DownloadView> {
        self.snap.downloads.iter().filter(|d| self.filter.matches(d.status)).collect()
    }

    pub fn selected_view(&self) -> Option<&DownloadView> {
        self.selected.and_then(|id| self.visible().into_iter().find(|d| d.id == id))
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected.and_then(|id| self.visible().iter().position(|d| d.id == id))
    }

    fn fix_selection(&mut self) {
        let visible = self.visible();
        if self.selected.is_none_or(|id| !visible.iter().any(|d| d.id == id)) {
            self.selected = visible.first().map(|d| d.id);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let current = self.selected_index().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, visible.len() as isize - 1) as usize;
        self.selected = Some(visible[next].id);
    }

    fn notify(&mut self, msg: impl Into<String>, error: bool) {
        self.toast = Some((msg.into(), error, Instant::now()));
    }

    fn on_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Input { buffer, .. } => buffer.push_str(&text.replace(['\r', '\n'], " ")),
            Mode::Normal if text.contains("://") => {
                self.mode = Mode::Input { kind: InputKind::AddUrls, buffer: text.replace(['\r', '\n'], " ") };
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.on_normal_key(key),
            Mode::Help => {}
            Mode::Confirm { id, name, delete_file } => match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => match self.engine.remove(id, delete_file) {
                    Ok(()) => self.notify(format!("Removed {name}"), false),
                    Err(e) => self.notify(e.to_string(), true),
                },
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {}
                _ => self.mode = Mode::Confirm { id, name, delete_file },
            },
            Mode::Input { kind, mut buffer } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit(kind, buffer.trim()),
                KeyCode::Backspace => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        buffer.clear();
                    } else {
                        buffer.pop();
                    }
                    self.mode = Mode::Input { kind, buffer };
                }
                KeyCode::Char(c) => {
                    buffer.push(c);
                    self.mode = Mode::Input { kind, buffer };
                }
                _ => self.mode = Mode::Input { kind, buffer },
            },
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent) {
        let selected = self.selected_view().cloned();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX / 2),
            KeyCode::Tab | KeyCode::Right => self.cycle_filter(1),
            KeyCode::BackTab | KeyCode::Left => self.cycle_filter(-1),
            KeyCode::Char(c @ '1'..='5') => self.filter = Filter::ALL[(c as u8 - b'1') as usize],
            KeyCode::Char('a' | 'n') | KeyCode::Insert => {
                self.mode = Mode::Input { kind: InputKind::AddUrls, buffer: String::new() };
            }
            KeyCode::Char('?' | 'h') => self.mode = Mode::Help,
            KeyCode::Char('L') => {
                let current = self.snap.speed_limit;
                let buffer = if current == 0 { String::new() } else { stewardget_core::util::format_bytes(current) };
                self.mode = Mode::Input { kind: InputKind::GlobalLimit, buffer };
            }
            KeyCode::Char('P') => {
                self.engine.pause_all();
                self.notify("Paused everything", false);
            }
            KeyCode::Char('S') => {
                self.engine.resume_all();
                self.notify("Resumed everything", false);
            }
            KeyCode::Char('c') => {
                self.engine.clear_completed();
                self.notify("Cleared completed downloads (files kept)", false);
            }
            _ => {}
        }
        let Some(d) = selected else { return };
        let result = match key.code {
            KeyCode::Char(' ' | 'p') if d.status.can_pause() => self.engine.pause(d.id),
            KeyCode::Char(' ' | 'p' | 'r') if d.status.can_resume() => self.engine.resume(d.id),
            KeyCode::Char('R') => {
                self.notify(format!("Restarting {}", d.file_name), false);
                self.engine.restart(d.id)
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                self.mode = Mode::Confirm { id: d.id, name: d.file_name.clone(), delete_file: false };
                Ok(())
            }
            KeyCode::Char('X') => {
                self.mode = Mode::Confirm { id: d.id, name: d.file_name.clone(), delete_file: true };
                Ok(())
            }
            KeyCode::Char('u') if d.status != Status::Completed => {
                self.mode = Mode::Input { kind: InputKind::NewUrl(d.id), buffer: String::new() };
                Ok(())
            }
            KeyCode::Char('w') if d.status != Status::Completed => {
                let waiting = !d.awaiting_link;
                let r = self.engine.await_new_link(d.id, waiting);
                if waiting && r.is_ok() {
                    let page = d.options.referrer.clone().unwrap_or_else(|| d.url.clone());
                    let _ = stewardget_core::util::open_url(&page);
                    self.notify("Waiting for the browser: start the download again on the page", false);
                }
                r
            }
            KeyCode::Char('f') if d.status == Status::Failed => {
                self.notify("Resuming with the data already downloaded", false);
                self.engine.resume_anyway(d.id)
            }
            KeyCode::Char('K') => self.engine.reorder(d.id, -1),
            KeyCode::Char('J') => self.engine.reorder(d.id, 1),
            KeyCode::Char('l') => {
                let cur = d.options.speed_limit;
                let buffer = if cur == 0 { String::new() } else { stewardget_core::util::format_bytes(cur) };
                self.mode = Mode::Input { kind: InputKind::DownloadLimit(d.id), buffer };
                Ok(())
            }
            KeyCode::Char('o') => stewardget_core::util::reveal_in_file_manager(&d.path)
                .map_err(|e| stewardget_core::Error::Io(e.to_string())),
            KeyCode::Enter if d.status == Status::Completed => {
                stewardget_core::util::open_with_system(&d.path).map_err(|e| stewardget_core::Error::Io(e.to_string()))
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.notify(e.to_string(), true);
        }
    }

    fn cycle_filter(&mut self, delta: isize) {
        let i = Filter::ALL.iter().position(|f| *f == self.filter).unwrap_or(0) as isize;
        let n = Filter::ALL.len() as isize;
        self.filter = Filter::ALL[((i + delta).rem_euclid(n)) as usize];
    }

    fn submit(&mut self, kind: InputKind, text: &str) {
        match kind {
            InputKind::AddUrls => {
                let mut added = 0;
                let links = text.split_whitespace().flat_map(|t| stewardget_core::util::expand_pattern(t, 5000));
                for url in links {
                    match self.engine.add(NewDownload::new(url)) {
                        Ok(id) => {
                            added += 1;
                            self.selected = Some(id);
                        }
                        Err(e) => return self.notify(e.to_string(), true),
                    }
                }
                if added > 0 {
                    self.filter = Filter::All;
                    self.notify(format!("Added {added} download{}", if added == 1 { "" } else { "s" }), false);
                }
            }
            InputKind::NewUrl(id) => match self.engine.set_url(id, text) {
                Ok(()) => self.notify("Link updated — resuming", false),
                Err(e) => self.notify(e.to_string(), true),
            },
            InputKind::GlobalLimit | InputKind::DownloadLimit(_) => {
                let rate = if text.is_empty() { Some(0) } else { stewardget_core::util::parse_size(text) };
                let Some(rate) = rate else {
                    return self.notify(format!("`{text}` is not a rate like 500k or 2M"), true);
                };
                let label =
                    if rate == 0 { "unlimited".into() } else { stewardget_core::util::format_speed(rate as f64) };
                match kind {
                    InputKind::DownloadLimit(id) => {
                        if let Err(e) = self.engine.set_download_speed_limit(id, rate) {
                            return self.notify(e.to_string(), true);
                        }
                        self.notify(format!("Download limit: {label}"), false);
                    }
                    _ => {
                        self.engine.set_speed_limit(rate);
                        self.notify(format!("Global limit: {label}"), false);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use stewardget_core::{Engine, EngineOptions, NewDownload};

    use super::App;

    /// Renders the dashboard into an in-memory terminal at several sizes.
    #[tokio::test(flavor = "multi_thread")]
    async fn renders_at_all_sizes() {
        let engine = Engine::start(EngineOptions::in_memory()).unwrap();
        for i in 0..3 {
            engine
                .add(NewDownload { url: format!("http://127.0.0.1:9/file{i}.zip"), paused: true, ..Default::default() })
                .unwrap();
        }
        let mut app = App::new(engine);
        app.snap = app.engine.snapshot();
        for (w, h) in [(160, 40), (100, 30), (60, 16)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| super::ui::draw(f, &app)).unwrap();
            if std::env::var_os("STEWARDGET_PRINT_TUI").is_some() {
                println!("{}", terminal.backend());
            }
        }
    }
}
