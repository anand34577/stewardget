//! The notification-area (tray) icon on Windows and the menu-bar icon on
//! macOS: shows the speed, brings the window back, and pauses, resumes or
//! quits without opening it. Linux has none (it would need GTK).

/// What the user picked from the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Never produced where there is no tray.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub enum TrayAction {
    Show,
    Add,
    PauseAll,
    ResumeAll,
    Quit,
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use std::sync::mpsc::{Receiver, channel};

    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::TrayAction;

    enum Raw {
        Icon(TrayIconEvent),
        Menu(MenuEvent),
    }

    pub struct Tray {
        icon: TrayIcon,
        events: Receiver<Raw>,
        items: Vec<(MenuItem, TrayAction)>,
        tooltip: String,
    }

    impl Tray {
        pub fn new(ctx: &egui::Context, rgba: Vec<u8>, size: u32) -> Option<Self> {
            let items = vec![
                (MenuItem::new("Show StewardGet", true, None), TrayAction::Show),
                (MenuItem::new("Add URL…", true, None), TrayAction::Add),
                (MenuItem::new("Resume all", true, None), TrayAction::ResumeAll),
                (MenuItem::new("Pause all", true, None), TrayAction::PauseAll),
                (MenuItem::new("Quit StewardGet", true, None), TrayAction::Quit),
            ];
            let menu = Menu::new();
            for (i, (item, _)) in items.iter().enumerate() {
                if i == 2 || i == 4 {
                    menu.append(&PredefinedMenuItem::separator()).ok()?;
                }
                menu.append(item).ok()?;
            }
            let icon = TrayIconBuilder::new()
                .with_icon(Icon::from_rgba(rgba, size, size).ok()?)
                .with_tooltip("StewardGet")
                .with_menu(Box::new(menu))
                // Windows convention: left click opens the window, right click the menu.
                .with_menu_on_left_click(cfg!(target_os = "macos"))
                .build()
                .ok()?;
            // Events arrive on the UI thread's message loop; wake egui so they
            // are handled even while the window is minimised.
            let (tx, events) = channel();
            {
                let (tx, ctx) = (tx.clone(), ctx.clone());
                TrayIconEvent::set_event_handler(Some(move |e| {
                    let _ = tx.send(Raw::Icon(e));
                    ctx.request_repaint();
                }));
            }
            {
                let ctx = ctx.clone();
                MenuEvent::set_event_handler(Some(move |e| {
                    let _ = tx.send(Raw::Menu(e));
                    ctx.request_repaint();
                }));
            }
            Some(Self { icon, events, items, tooltip: String::new() })
        }

        pub fn poll(&self) -> Vec<TrayAction> {
            self.events
                .try_iter()
                .filter_map(|raw| match raw {
                    Raw::Icon(TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    })
                    | Raw::Icon(TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }) => {
                        (!cfg!(target_os = "macos")).then_some(TrayAction::Show)
                    }
                    Raw::Icon(_) => None,
                    Raw::Menu(e) => self.items.iter().find(|(item, _)| item.id() == e.id()).map(|(_, a)| *a),
                })
                .collect()
        }

        pub fn set_tooltip(&mut self, text: String) {
            if text != self.tooltip {
                let _ = self.icon.set_tooltip(Some(&text));
                self.tooltip = text;
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::TrayAction;

    pub struct Tray;

    impl Tray {
        pub fn new(_ctx: &egui::Context, _rgba: Vec<u8>, _size: u32) -> Option<Self> {
            None
        }

        pub fn poll(&self) -> Vec<TrayAction> {
            Vec::new()
        }

        pub fn set_tooltip(&mut self, _text: String) {}
    }
}

pub use imp::Tray;
