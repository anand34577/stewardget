# Contributing to StewardGet

Thanks for helping! A few guidelines keep the project healthy.

## Getting started

```sh
cargo build                 # all crates
cargo run -p stewardget-gui      # desktop app
cargo run -p stewardget-cli      # terminal UI
cargo test --workspace      # unit + end-to-end engine tests
```

Set `STEWARDGET_DATA_DIR=/tmp/stewardget-dev` while developing so you don't touch your real
download list.

## Before opening a pull request

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets` must be warning-free
- `cargo test --workspace` must pass
- New engine behaviour comes with a test in `crates/stewardget-core/tests/engine.rs`
  (the in-process test server can throttle, drop connections, cap connections…).

## Principles

- **Privacy first.** No telemetry, analytics or network calls other than the
  downloads the user asked for. PRs that add any will not be accepted.
- **Correctness over cleverness.** A download must never be silently corrupted:
  validate, verify, and fail loudly with a helpful message.
- **No emoji as UI icons.** Use the Lucide glyphs in `crates/stewardget-gui/src/icons.rs`. To add one, append its
  name to the subset list and regenerate `crates/stewardget-gui/assets/lucide-subset.ttf` with fontTools.
- The logo and every icon derived from it come from `tools/gen_icons.py` (needs Pillow).
- Controls share one height (`theme::CONTROL_H`); use `widgets::input`, `widgets::button` and
  `widgets::form_row` so forms line up in every theme.
- Keep dependencies lean and the code readable.
- Installers and packages are built by `.github/workflows/release.yml` from the scripts in `packaging/`
  (Inno Setup for Windows, `dpkg-deb`/`rpmbuild` for Linux, an app bundle and DMG for macOS). Pull
  requests that change them run the whole build and the install tests without publishing anything.
- When you change the extension, keep `extension/manifest.json` (Chromium) and
  `extension/manifest.firefox.json` in step: they differ only in `background` and
  `browser_specific_settings`, and both carry the crate version. Tests check both.

## Reporting bugs

Please include your OS, how you ran StewardGet (GUI/TUI/CLI), the URL type (no private
links!), and the error shown in the details pane.
