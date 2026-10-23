# Vendored crates

## eframe 0.37.0

[eframe](https://github.com/emilk/egui/tree/main/crates/eframe) 0.37.0 by Emil
Ernerfeldt and the egui contributors, MIT OR Apache-2.0, unchanged except for
one fix in `src/native/run.rs` (`new_events`, marked "StewardGet patch").

On Windows, eframe paints hidden windows from `new_events` without the event
loop context. A hidden main window (StewardGet in the tray) that opens a small
window then panics: the new window can't be created, so its ui is never run.
The fix gives `new_events` the same context as the other event handlers.

Drop this folder and the `[patch.crates-io]` entry in the root `Cargo.toml`
once a release of eframe has the fix.
