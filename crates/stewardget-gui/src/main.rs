//! StewardGet desktop app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod capture;
mod categories;
mod details;
mod dialogs;
mod grab;
mod icons;
mod menubar;
mod notify;
mod progress;
mod queues;
mod settings;
mod stats;
mod table;
mod theme;
mod tray;
mod widgets;

use std::time::Duration;

use stewardget_core::remote::{self, RemoteError};
use stewardget_core::{Capture, Engine, EngineOptions, Error};

/// What the command line asked for.
#[derive(Debug, Default)]
pub struct Launch {
    /// `stewardget-gui <URL>...` queues the URLs (handy for "Open with" and scripts).
    pub urls: Vec<String>,
    /// `--background`: start minimised (used by "Start with your computer").
    pub background: bool,
    /// `--setup-extension`: open the browser-extension setup (used by the installer).
    pub setup_extension: bool,
    /// `--quit`: ask a running StewardGet to quit and wait for it (used by the installer).
    pub quit: bool,
}

impl Launch {
    fn from_args() -> Self {
        let mut launch = Launch::default();
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--background" | "--minimized" => launch.background = true,
                "--setup-extension" => launch.setup_extension = true,
                "--quit" => launch.quit = true,
                a if a.contains("://") => launch.urls.push(a.to_owned()),
                _ => {}
            }
        }
        launch
    }
}

fn main() -> eframe::Result {
    let launch = Launch::from_args();
    if launch.quit {
        let config = EngineOptions::peek_config(&EngineOptions::default_data_dir()).unwrap_or_default();
        // Nothing to report: the installer carries on either way.
        let _ = remote::quit(&config, Duration::from_secs(15));
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("stewardget-engine")
        .build()
        .expect("failed to start async runtime");
    let engine = {
        let _guard = runtime.enter();
        match Engine::start(EngineOptions::persistent()) {
            Ok(engine) => engine,
            Err(Error::Locked(_)) => {
                hand_over(&launch);
                std::process::exit(0);
            }
            Err(e) => {
                error_box(&format!("StewardGet could not start:\n\n{e}"));
                std::process::exit(1);
            }
        }
    };

    // A login start must never flash a window: create it hidden when the tray
    // is there to bring it back (`App::park` shows it minimised when it isn't).
    let start_hidden = launch.background && cfg!(any(windows, target_os = "macos")) && engine.config().minimize_to_tray;
    let options = eframe::NativeOptions {
        // Window size and position are remembered next to the other data.
        persistence_path: engine.data_dir().map(|d| d.join("window.ron")),
        viewport: egui::ViewportBuilder::default()
            .with_visible(!start_hidden)
            .with_title("StewardGet")
            .with_app_id("dev.stewardget.StewardGet")
            .with_inner_size([1180.0, 720.0])
            .with_min_inner_size([680.0, 460.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "StewardGet",
        options,
        Box::new(|cc| Ok(Box::new(app::StewardGetApp::new(cc, runtime, engine, launch)))),
    )
}

/// StewardGet is already running: pass the URLs on and bring its window forward,
/// instead of failing with "already in use".
fn hand_over(launch: &Launch) {
    let config = EngineOptions::peek_config(&EngineOptions::default_data_dir()).unwrap_or_default();
    #[cfg(windows)]
    // SAFETY: plain Win32 call without pointers. Lets the running instance take
    // the foreground, which Windows otherwise reserves for the process the user
    // just started (this one).
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    // The other instance may still be starting up (double-click, login item).
    let mut attempt = 0;
    let shown = loop {
        let result = if launch.urls.is_empty() {
            Ok(())
        } else {
            let captures: Vec<Capture> =
                launch.urls.iter().map(|url| Capture { url: url.clone(), ..Default::default() }).collect();
            remote::add(&config, &captures).map(drop)
        };
        let result = result.and_then(|()| if launch.background { Ok(true) } else { remote::show(&config) });
        match result {
            Err(RemoteError::NotRunning(_)) if attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(300));
            }
            other => break other,
        }
    };
    match shown {
        Ok(false) if launch.urls.is_empty() => error_box(
            "StewardGet is already running in a terminal (the `stewardget` dashboard).\n\nQuit it there (press q) to open the desktop app.",
        ),
        Err(RemoteError::Disabled) => error_box(
            "StewardGet is already running.\n\nIts window could not be brought forward because browser integration is turned off. Look for StewardGet in the taskbar.",
        ),
        Err(e) => error_box(&format!(
            "StewardGet is already running, but it did not answer ({e}).\n\nIf you can't find its window, end \"stewardget-gui\" in your system's task manager and start StewardGet again."
        )),
        Ok(_) => {}
    }
}

fn error_box(text: &str) {
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("StewardGet").set_description(text).show();
}

/// The app icon, 128×128 RGBA (rendered by `tools/gen_icons.py`).
pub const ICON_RGBA: &[u8] = include_bytes!("../assets/icon-128.rgba");

fn app_icon() -> egui::IconData {
    egui::IconData { rgba: ICON_RGBA.to_vec(), width: 128, height: 128 }
}
