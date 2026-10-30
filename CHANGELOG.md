# Changelog

All notable changes to StewardGet are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.11.0] - 2026-10-30

### Added
- **Phone and email notifications.** Send messages to **Gotify**, **ntfy** (ntfy.sh or your own server) and **email over SMTP**, also while the window is closed or with `stewardget serve`. Add as many destinations as you like, one per person, device or team, under *Settings > General*. Each picks its own events (a download finished, a download failed, a queue finished, everything finished) and, if you want, only some queues; the queue window shows and changes which destinations hear about that queue. Email can go to several addresses at once. Every destination has a *Send test* button that shows the server's answer. Tokens and the mail password stay on this computer and are left out of backups.

### Changed
- **StewardGet's own title bar.** The header with the menus, search and *Add download* is now the window's title bar, with minimise, maximise and close buttons on the right (macOS keeps its traffic lights). Drag it to move the window and double-click it to maximise; the edges resize the window. The separate system title bar is gone, so pages get that space. Prefer the system one? Turn on *Settings > Appearance > System title bar* (applies after a restart).
- **The list is the default view again**, with your saved choices kept. 0.10.0 reused the *Compact rows* setting for the new card view, so upgrading switched most people to cards. Cards now have a setting of their own, off by default: the *List | Cards* switch above the downloads, or *Settings > Appearance > Card view*. *Compact rows* means what it did before: one line per download, or the name with the file type and site under it.
- In card view, the cards and summary tiles are more compact, so more downloads fit on screen. A button next to *Sort by* hides or shows the summary tiles, and the choice is remembered.
- History shows its totals in one line under the title instead of four large tiles.
- Corners are rounded consistently: buttons, fields and drop-downs one way, cards and tiles a little more, dialogs and the page canvas the most. Some cards and dialogs were noticeably rounder than everything around them.
- Sliders have a slim rail and a small round handle in the accent colour instead of a large outlined circle, everywhere: settings (including the network timeouts), the Add and Properties windows, queues and Grab.
- Long file names and paths are shortened with an ellipsis instead of being cut off mid-letter or running into the sizes and buttons next to them. Paths are shortened in the middle, so the file name stays visible, and the full text shows on hover. This covers the list, cards, sidebar, details, Tools, Statistics, the Overview and the browser download window.
- Drop-down lists match the text fields: the same fill and outline, a thin chevron instead of the filled triangle, and roomier choices with the current one highlighted in the accent colour.
- In a narrow window the header folds File, Tasks, Tools and Help into one *Menu* and gives the search box the room that is left, and the status bar keeps only the essentials, so nothing overlaps.
- Statistics, History and Tools use the same page layout, spacing and titles as the rest of the app. Tools no longer shows three filled buttons at once.

### Fixed
- The last entry of the sidebar looked faded even when the whole list fit. The fade now only appears when there is more to scroll to.
- In a short window the details pane took most of the height; it now starts at about 40% of it, and its header no longer runs under its buttons.
- On Statistics, figures such as the peak speed could break onto several lines one character at a time, and the tiles ended up with different heights. The page now shows four tiles of equal height, in one row when there is room.

## [0.10.0] - 2026-10-30

### Changed
- **A redesigned window.** The sidebar now runs the full height, menus, search and *Add download* share one header, and pages sit on a rounded canvas instead of under three stacked toolbars.
- What you can do with the selection (Resume, Pause, Restart, Refresh link, Remove) now appears in the list's own header, and only while something is selected. With nothing selected, the same spot offers *Start all* and *Pause all*.
- **Downloads as cards.** Each download is a card with a file-type tile showing its extension, its source and when it was added, a progress bar, and the figures that matter right now (speed, time left, connections; or how long it took and the average speed once finished). Pause, resume, open, show in folder and the full menu are buttons on the card. *List* switches to the dense sortable list with columns.
- Above the cards, four live tiles: speed with a one-minute graph, what is in progress with the data still to come and an estimate, what arrived today with a two-week graph, and the free disk space, which turns amber when the unfinished downloads won't fit.
- **Inspector.** In a wide window the details of the selected download open in a column on the right, with a progress ring, the live figures, its actions, the connection map, the speed graph and all its information. Narrow windows keep the pane at the bottom.
- Primary buttons, the selected sidebar entry and live progress use a gradient of the accent colour, and a soft glow of the accent lights the top of each page. The Overview uses the same tiles.
- The sidebar shows counts as badges and keeps a live card with the current speed and a small graph; click it to see what is downloading.
- A clearer type scale with letter-spaced section labels, and refined colours in every theme.

### Fixed
- Text fields and drop-down lists could keep the colours of the other theme when StewardGet's theme differed from the system's (for example, black fields in the light theme).

## [0.9.1] - 2026-10-30

### Fixed
- **Show in folder** opened Documents instead of the downloaded file when the path had spaces (Windows). It now opens the folder with the file selected; if the file is gone it opens the nearest folder that still exists, and says so when the folder can't be opened at all.
- Opening a file or folder from StewardGet no longer fails silently: if the system can't open it, a message says so.
- Linux: *Show in folder* now opens the file manager with the file selected (Nautilus, Dolphin, Nemo, Thunar and other freedesktop file managers); helper programs it starts are cleaned up instead of lingering.
- Linux and macOS: the completion sound now plays (when `canberra-gtk-play`, `paplay` or macOS `afplay` is available).
- *When all downloads finish → sleep / shut down* reports it when the system command fails (for example no systemd) instead of looking like it worked.
- Windows and macOS: two downloads whose names differ only in capitals (`Setup.exe`, `setup.exe`) no longer try to use the same file.
- The *move* step for finished files no longer leaves a second copy behind when the original is merely in use.
- The new-download window for browser downloads now notices that a file with that name is already in the folder and offers *Open existing*, *Save with another name* or *Replace it*, like the Add window.

## [0.9.0] - 2026-10-30

### Added
- **Optional update check** (Settings › Privacy & data), off by default. When on, StewardGet asks the project's public release page for the newest version number once at startup, or when you press *Check now*; it sends no identifiers and installs nothing.
- **Command palette** (Ctrl+K): jump to any page, setting, action or download from one box.
- **Settings search**: type in the box at the top of Settings to find an option in any tab.
- **Undo for Remove**: removing idle downloads no longer asks; an Undo bar stays for 8 seconds. Running downloads and deleting files still ask first.
- **Retry all** button in the Failed view for downloads that can simply be retried.
- Empty views now say why they are empty and offer the next step (Connect browser, Import links, Clear search, Show all).
- Plain-language hints for network, timeout, busy-server, disk and bad-link failures, also in the progress window.

### Changed
- Text is never smaller than 12 px. A test now checks text contrast in every theme and accent.
- The idle window redraws less often to save battery.
- The browser extension popup keeps filters and connection settings in a collapsible section.
- `stewardget add` mentions `stewardget serve` when the app isn't running.

## [0.8.0] - 2026-10-30

- Adding a link whose file is already downloaded now offers three choices: open the existing file in its folder, save with another name, or replace it (a per-download choice; the setting stays the default).
- The Add dialog is more compact, and the Statistics cards are the same height side by side.

### Added (more sources and smarter automation)
- **FTP**: `ftp://` links download in parallel, resumable segments like HTTP ones (passive mode, `REST`). Plain FTP is not encrypted; SFTP and FTPS are not supported.
- **Video and audio from a page**: File â€º Video or audio from a page asks yt-dlp (if installed) what a page offers and downloads the direct files with StewardGet's own engine. Separate picture and sound files are joined without re-encoding by ffmpeg (if installed); if joining fails, both files are kept. DASH manifests (`.mpd`) made of single files are read directly; segmented manifests are not supported.
- **Magnet links** are recognised and handed to your default torrent app. StewardGet has no BitTorrent engine of its own.
- **Steps for finished files** gained Rename and folder/name templates (`{category}`, `{year}`, `{month}`, `{day}`, `{host}`, `{name}`, `{ext}`).
- **Already downloaded?** The Add window says when the same link, or a file with the same name and size, finished earlier.
- **Quiet hours**: the Overview suggests the least-used hours and creates a scheduled queue for them.
- **MCP**: `search_history`, `history_summary`, `tag_download`, `add_downloads` and `grab_page` tools.
- **Headless mode**: `stewardget serve` runs downloads, queues, schedules, the browser API and MCP without a window.

### Added (tools)
- **Tools page**: duplicate-file finder (by content), leftover partial-file cleanup, link checker, backup and restore of settings, diagnostics report (no links or tokens).
- **Monthly data limit** for metered or hotspot connections, with a usage bar on the Overview.
- **yt-dlp and ffmpeg setup**: found automatically (also in winget, Scoop and Chocolatey folders), file picker with a real version check, one-click install through winget, and an advanced path field.

### Fixed
- The menu bar was too short.
- Settings controls are aligned to the right edge and the page uses the full width.

### Added (speed and automation)
- **Mirrors**: give a file several links; connections are spread over all of them. A mirror must answer a range request with the same size, and one that fails or serves something else is dropped without stopping the download. `stewardget get --mirror`.
- **Several networks at once**: bind connections to Wi-Fi, Ethernet or a phone's hotspot together, with a switch and a speed cap per adapter and live speed per link on the Overview. `stewardget get --bind`, `stewardget links`.
- **Connection auto-tuner**: starts with a few connections and adds more while the speed keeps improving. Off per file when you set its connection count.
- **Sources and links** card in the details pane shows each lane's speed, share and failures.
- **Steps for each finished file**: unpack (.zip, .tar, .tar.gz), scan (Defender or ClamAV), move, run a program, tag; filtered by kind, extension and site. Results show in the History.

### Added
- **History**: every finished or failed download is kept in a permanent,
  searchable log that outlives the download list. Filter by status, period
  and type; open, reveal, re-download, copy the link or forget an entry;
  export to CSV. Stored locally in `history.json`; lists from earlier
  versions seed it from their completed downloads.
- **Overview** page: live speed over its graph, what is downloading with the
  reason for its speed, recently finished files, when you download and which
  sources have been fastest.
- **"Why this speed?"** explainer for running downloads (details pane and
  Overview): names the cause â€” your own limits, the server allowing fewer
  connections or no ranges, dropping connections, sharing the line, or a
  source slower or faster than usual â€” and offers a one-click fix.
- A six-month calendar heat-map on the Statistics page.

### Changed
- A tighter, more compact interface: less padding and spacing in the sidebar,
  toolbar, tables, cards and pages, so more of each screen shows content.

## [0.7.4] - 2026-10-28

### Fixed
- Chromium browsers no longer show their own Save As dialog or download
  bubble some of the time: the extension takes the download before the
  browser asks for a file name (`downloads.onDeterminingFilename`), instead of
  racing the browser after it started.
- Downloads from the browser get the server's file name instead of an id from
  the link (`5a26b454-bf1a-â€¦`): a name that is only the link's last segment no
  longer overrides `Content-Disposition`, also when the new-download window is
  accepted before the server answered.
- Accepting the new-download window opens the progress window right away, and
  downloads that arrive together (up to three) each get one.
- Signed links that expire mid-download (GitHub release assets, S3) are
  replaced with a fresh redirect instead of failing with HTTP 403. The
  extension sends the link as clicked, with the final address as a fallback
  for one-time links.
- Pausing, restarting or refreshing a link wins over an automatic retry when
  the download fails at that moment; a download paused just before quitting
  stays paused.
- Finishing or saving progress no longer fails with "Access is denied" while
  a virus scanner or the search indexer briefly holds the file (Windows).
- Lifting a speed limit wakes connections that were waiting under the old one.
- A download of unknown size that had to start over no longer keeps bytes from
  the earlier attempt at its end.
- HLS: pieces cut from one file are correct when the server ignores `Range`;
  a stalled playlist can be paused and times out; oversized playlists are
  refused while reading; a refreshed link resumes the stream; variants with
  their sound inside are preferred over ones with a separate audio track.
- Site grabber: relative links follow redirects and `<base href>`, and the
  search stops once the file budget is reached.
- The after-download program no longer expands placeholders that appear in a
  file name.
- `stewardget get` saves into `--dir` (or the current folder), not the desktop
  app's category folders, and ignores its time-of-day speed limit.
- The parts table in the progress window uses the full width.
- The extension's video list survives the browser stopping its background
  script; "skip these extensions" accepts spaces as separators.

## [0.7.3] - 2026-10-25

### Security
- Updated `tokio` (1.54.1) and `rustls-webpki` (0.103.16), which had
  published security advisories.

### Changed
- The minimum Rust version is 1.95, which is what the interface library
  already needed.

## [0.7.2] - 2026-10-25

### Security
- Clients without a token can no longer skip the new-download question
  (`direct`) or add request headers through the local API.
- Settings, the download list and partial-download files are created readable
  by their owner only (Linux and macOS); the data folder is private too.
- HLS streams no longer send cookies or passwords to other hosts named in a
  playlist.
- `stewardget config` hides tokens and proxy credentials.

### Fixed
- A partial file that was cut short is reported instead of finishing with holes.
- A fresh browser link attaches to a waiting download only when the file name
  also matches, not on size alone.
- Batch patterns such as `[a-b][a-b]â€¦` no longer take exponential time, and
  `[A-z]` is not a range.
- `CON .txt`, `CONIN$` and `CONOUT$` are made safe on Windows.
- A file name starting with `-` is passed as `./-name` to the after-download
  program.
- Enter no longer accepts a new-download window in its first moments.
- Changing the speed limit keeps open connections; settings changes cannot
  overwrite each other.
- `%` in the program path works in the Linux login item.

### Changed
- CI checks dependencies with `cargo audit` and builds with the minimum Rust
  version.

## [0.7.1] - 2026-10-23

Downloads from the browser now feel like they do in IDM and AB Download
Manager: small windows of their own, with the main window left alone.

### Fixed
- A download from the browser no longer flashes the main window before the
  new-download window: only the small window opens, also while StewardGet is
  in the tray or minimised.
- The progress window now opens while StewardGet is in the tray or minimised.
  It used to appear only when the main window was on screen.
- A download that finishes or fails within a second still gets its window,
  instead of nothing.
- The *Download complete* window shows while StewardGet is in the tray; it
  was drawn inside the main window, so it waited there unseen.

### Changed
- The progress window is laid out like IDM's: status, file size, downloaded,
  transfer rate, time left and resume support each on their own line, the
  link, and a bar with each connection's progress. *Show details* lists the
  parts with what each one is doing; the choice is kept for the next window.
- It sets the download's own speed limit, and turns into *Download complete*
  (Open, Show in folder) when the file is done.
- Pausing a download whose server can't resume asks first, since it would
  have to start over.
- A failed download's window says what went wrong and offers *Retry*, or
  *Start over* when the server stopped supporting resume.
- Small windows open centred on the screen, a little apart when there are
  several.
- The new-download window offers *Show it* for a link that is already in your
  list.

## [0.7.0] - 2026-10-23

Velox is now **StewardGet**. Another download manager already used the name
Velox, so the app has its own name now, and a new icon to match: a download
arrow over a wave.

### Changed
- The app, its commands and its packages are called StewardGet: `stewardget`
  and `stewardget-gui`, `StewardGet-Setup.exe`, `StewardGet-macos.dmg`,
  `stewardget-linux-*.deb`/`.rpm`, the app ID `dev.stewardget.StewardGet`, and
  `$STEWARDGET_DATA_DIR` / `$STEWARDGET_MIRROR` / `$STEWARDGET_PREFIX`.
- The repository is now
  [github.com/anand34577/stewardget](https://github.com/anand34577/stewardget).
- The browser extension sends `X-StewardGet-â€¦` headers.

### Upgrading from Velox
Nothing is lost, and nothing has to be set up again:
- Settings, the download list and statistics move from Velox's data folder to
  StewardGet's the first time StewardGet starts. While a copy of Velox is still
  running, StewardGet keeps using the old folder rather than share it.
- `$VELOX_DATA_DIR` still works for portable setups.
- "Start with your computer" carries over.
- A browser extension loaded from Velox's folder keeps working, and is kept
  up to date there. Extensions from Velox are still accepted by the app.
- The Windows installer removes Velox (keeping your data) and installs
  StewardGet in its place.

## [0.6.0] - 2026-10-23

Progress windows, video and stream downloads, a cleaner download list, and
fixes from a full audit.

### Added
- **Progress window** for each download, like IDM's: status, downloaded,
  speed and time left, resume support, the parts, Pause and Close. It opens
  by itself for a download started on its own (not for each file of a batch);
  double-click or Enter opens it for any download. Settings â€º General.
- **Video and audio on a page.** The browser extension lists the media a page
  played, with a count on its icon, and sends any of them to StewardGet. HLS
  streams (`.m3u8`) are saved as one `.ts` or `.mp4` file in the best quality,
  AES-128 encrypted streams included, and resume where they stopped. Live and
  protected (DRM) streams are refused with a clear message.
- **Menu bar** (File, Tasks, Tools, Help) and a keyboard shortcuts sheet (F1).
- **Keyboard navigation** in the list: arrows, Page Up/Down, Home/End, and
  Shift to select a range.
- Right-click the list's header to choose columns or switch to compact rows.

### Changed
- Rows show the kind of file and the site under each name, and when it was
  added as "5 min ago" (the full time on hover). Downloads that haven't
  started show no empty bar or dashes.
- Sidebar sections fold away, so the categories fit on a laptop screen.
- A download whose connections are retrying says *Reconnectingâ€¦* instead of
  showing 0 B/s.
- "Finished in" counts the time spent downloading, not waiting in the queue.
- **Faster with long lists.** Only the rows on screen are copied each frame,
  the list is read at most ten times a second, and the checks for missing
  files and free space are cached.
- Darker green and amber in the light theme, so status text is easy to read.
- The browser extension now asks for the `webRequest` permission. It only
  watches requests (to find media and form downloads), never changes them.

### Fixed
- A link with spaces (`â€¦/Firefox Setup 131.0.exe`) was split into several
  links, and the pieces were added as bogus downloads.
- The window could open taller than the screen, hiding the status bar behind
  the taskbar.
- Downloads that answer a submitted form are kept in the browser instead of
  being fetched again and failing.
- File names with non-English characters sent by the server were ignored.
- In Firefox container tabs the extension sent the wrong cookies.
- A checksum entered for several links at once failed all but one of them; it
  is now refused with an explanation. A checksum added in Properties while a
  download runs is now checked.
- Renaming a file only by case (`file.zip` â†’ `File.zip`) failed on Windows and
  macOS.
- The statistics chart's last two dates overlapped, and finished downloads
  showed "peak 0 B/s".
- Docs and messages said progress is saved every second (it is every 3 s) and
  pointed to a settings page by an old name.

## [0.5.1] - 2026-10-22

A quiet start, a lighter idle, and a tighter local API.

### Fixed
- **No window at sign-in.** Starting with the computer (`--background`) no
  longer flashes or opens the window: it is created hidden and stays in the
  tray (or the taskbar when the tray is off).
- A server that accepts a connection and never answers no longer leaves a
  download on *Connecting* forever; the first request now times out.
- The `202` reply to MCP notifications used bare line feeds instead of CRLF,
  which strict HTTP clients refuse.
- Progress is recorded in the control file *before* the data is flushed, so a
  crash can no longer leave it ahead of what reached the disk.
- Cookies and passwords are no longer sent to a different host when a download
  redirects to a CDN or storage bucket.
- The sidebar's *Queued* entry said "Queued" but counted paused downloads too;
  it is now *Queued & paused*, and the status bar counts paused downloads.
  "1 items" is "1 item".
- Settings saved by an older version kept their old `StewardGet/0.2.0` user agent;
  untouched defaults now follow the version (custom ones are kept).

### Changed
- **Lighter when idle.** Hidden and idle, StewardGet wakes about once a second (every
  5 s with clipboard monitoring off) instead of twice, reads the clipboard only
  when Windows says it changed, and no longer copies the whole list every frame.
  Measured: about 60 % less CPU while hidden.
- **Less disk work while downloading.** The download list is saved off the UI
  and engine threads, statistics every 10 s instead of 2 s, and the flush to
  disk happens every 3 s on a worker thread, not on the async runtime.
- Copying a link while StewardGet is in the tray opens the small new-download window
  and leaves the main window where it is.
- **Local API.** Without an access token, only what the browser extension needs
  (ping, add a download, show StewardGet) is open. Listing, pausing, the speed limit
  and quitting need a key the command line and installer read from the settings
  file. A client can no longer choose the folder a download is saved to unless it
  holds a token. Tokens come from the system's random generator and are compared
  in constant time.
- The default user agent looks like a browser's (with `StewardGet/x.y` appended); many
  servers refuse anything else.
- The site grabber understands `*` and `$` in `robots.txt` rules.
- The program run after each download is stopped if it takes over ten minutes.

### Added
- **Completion sound**, on by default (Settings â€º General).

## [0.5.0] - 2026-10-21

Fixes for pausing, the tray and browser hand-over, an MCP server for AI
assistants, a site grabber, a fuller command line, and a new default look.

### Fixed
- **Pausing is immediate.** The size and speed stop moving the moment you
  click Pause; before, they kept rising for a few seconds while the
  connections closed. A speed limit no longer delays the pause either.
- **The tray icon now does its job.** The minimise button and *Keep running*
  hide StewardGet in the notification area (Windows) or menu bar (macOS) instead of
  leaving a taskbar button; click the icon to bring it back. Turn it off in
  **Settings â€º General â€º Minimise to the tray**.
- **Browser downloads while StewardGet was hidden.** StewardGet could close itself when
  a browser download arrived while its window was hidden, and the browser had
  already handed the file over. The new-download window now opens reliably
  from the tray, from the taskbar and while the window is minimised.
- Servers that answer *429 Too Many Requests* or *503* with a `Retry-After`
  are obeyed instead of being retried immediately.
- Progress is flushed to disk before it is recorded, so a power cut can no
  longer leave the saved progress ahead of the data.

### Added
- **MCP server** (off by default): **Settings â€º AI & automation**. An AI
  assistant can add, list, pause, resume, restart and remove downloads and
  set the speed limit. It runs on the local address only, needs a token, keeps
  new files below your download folder, and cannot change settings or run
  programs. `stewardget mcp --config` prints the settings for Claude Desktop and
  other clients; `stewardget mcp` bridges clients that start a program.
- **Site grabber**: **Grab** in the toolbar, or `stewardget grab <url>`. Finds the
  files linked from a web page, optionally following links a few levels deep,
  respecting `robots.txt`, and lists them in the new-download window.
- **About** page and the version in the sidebar; the browser extension shows
  its version and what happened to the last download it handled, and why when
  StewardGet could not take it.
- A warning for programs disguised as documents or pictures, such as
  `invoice.pdf.exe`, in the new-download window and when the download finishes.
- More command line: `stewardget pause|resume <id|all>`, `restart`, `remove`,
  `limit <rate>`, `list --json`, and `stewardget completions <shell>`.
- More of the local API for scripts: `GET /api/v1/downloads/{id}`, pause,
  resume, restart and delete, `GET /api/v1/stats`, `GET /api/v1/queues`,
  pause-all, resume-all and `POST /api/v1/speed-limit`.

### Changed
- New default colour: teal, instead of violet, so the app and its violet icon
  are easy to tell apart. Settings saved with the old default move to teal
  once; pick any accent in **Settings â€º Appearance**.
- The extension's colours follow the app. The extension is now version 0.5.0.

## [0.4.0] - 2026-10-20

Your own categories, speed limits per queue and by time of day, a tray icon,
notifications, and more control over what happens when a download finishes.

### Categories
- Add your own categories in **Settings â€º Categories**: a name and the file
  extensions that belong to it (for example *Disk images*: `iso, img`). Each
  gets a sidebar entry, a colour and its own folder, and takes priority over
  the built-in categories. Edit or delete them at any time; existing downloads
  are sorted again straight away.

### Speed
- **Speed limit per queue**, shared by the queue's downloads (edit the queue).
- **A different limit during set hours**, for example slower during work
  hours (**Settings â€º Downloads**). It can run past midnight and is set per
  weekday; the status bar shows when it is in force.
- Typing a limit such as `2M` now works; before, the field turned `2` into
  `2 B` before the `M` could be typed.

### When a download finishes
- Choose what happens when a file with the same name already exists: keep
  both (the new one gets a number) or replace it.
- **Run a program after each download**, such as a virus scanner or an
  unpacker, with `{path}`, `{dir}`, `{name}` and `{url}` filled in. On Windows
  one click sets up a Microsoft Defender scan. No shell is involved, so a file
  name can never run commands.
- **System notifications** when downloads finish or fail while StewardGet is in the
  background (turn them off in **Settings â€º General**).

### Desktop app
- **Tray icon** on Windows and a menu-bar icon on macOS: shows the current
  speed; click it to open StewardGet, or use its menu to add a link, pause or
  resume everything, or quit.
- Copying a file link is now noticed while StewardGet is minimised, too; StewardGet
  comes forward with the link ready to add.
- The window remembers its size and position.
- Back up all settings, queues and categories to a file and restore them, in
  **Settings â€º Privacy & data**.

### Browser
- **Never capture on these sites**, with a one-click button in the popup for
  the site you are on.
- Hold **Alt** while clicking a link to keep that one download in the browser.
- The popup shows the extension's version, warns when it is older than the
  app, has an **Open StewardGet** button, and no longer hangs on "checkingâ€¦".

### Fixed
- Theme previews in **Settings â€º Appearance** no longer stick out past their
  rounded corners.
- A queue's schedule that runs past midnight (say 23:00â€“07:00 on Fridays) now
  stops in the morning even when that day is not selected.
- Start and stop times that pass while the computer sleeps are caught up when
  it wakes.
- Category folders can be typed with spaces in their names.
- Browser cookies are no longer kept after a download completes.
- A very small file could end up downloaded by both the browser and StewardGet.
- Pasting a huge batch pattern such as `[1-999999]` no longer freezes the Add
  dialog.
- A queue named like a category (e.g. "Video") no longer reacts to the
  category's clicks in the sidebar.
- Hidden right-to-left characters are removed from file names, so a file can't
  disguise its real extension.
- **Sleep** after all downloads no longer hibernates on Windows PCs with
  hibernation enabled.
- Moving a download no longer leaves a second copy behind when the original
  can't be removed.

### For developers
- Building from source needs Rust 1.90 or newer.

## [0.3.0] - 2026-10-20

Queues, a new-download window for browser downloads, and "download all links".

### Queues
- Create as many download queues as you like (sidebar **+**, or **â‹¯ â€º New
  queueâ€¦**). Each has its own name, how many of its downloads run at once, and
  its own schedule (start time, pause time, weekdays).
- Move downloads between queues by dragging them onto a queue in the sidebar,
  or with right-click â€º **Move to queue**. Move to top or bottom of a queue.
- With a queue open, the toolbar's **Start queue** and **Stop queue** act on it
  alone. Right-click a queue to start, stop, edit or delete it. Deleting a
  queue moves its downloads to the main queue; nothing is deleted.
- **Settings â€º Queues & schedule** lists every queue. The schedule of StewardGet 0.2
  becomes the main queue's schedule.
- `stewardget queues` lists them; `stewardget add --queue NAME [--paused]` adds to one.

### New-download window
- Downloads from the browser now open a small window first, like IDM's: the
  real file name and size, whether it can resume, the folder (with recently
  used folders), free space and the queue. **Download**, **Later** (added
  paused to the queue) or **Cancel**; Enter and Esc work too. It stays on top
  and appears even while StewardGet is minimised. Turn it off with *Don't ask
  again* or in **Settings â€º General**.
- Several downloads at once (see below) show a list with checkboxes, filters
  by file type and a search box. Files are preselected, web pages are not, and
  links already in your list are marked.
- Downloads waiting in the window when StewardGet quits are kept, paused.

### Browser
- **Download all links with StewardGetâ€¦** in the page's right-click menu and in the
  extension's popup sends every link and media file on the page to the list
  window, to pick what you want.
- Chrome, Edge, Brave, Opera and Vivaldi no longer report
  "'background.scripts' requires manifest version of 2 or lower" when you load
  the extension. Chromium browsers and Firefox now each get a manifest they
  accept: the folder for Chromium browsers stays where it was, and Firefox
  loads its own `browser-extension-firefox` folder (Settings â€º Browser and
  `stewardget extension` show the right one). If you already loaded the extension,
  restart StewardGet, then click the reload button on the extensions page; the error
  entry can be cleared.

### Also new
- The close button can keep StewardGet running in the background (minimised), so
  downloads continue and the browser can still hand it new ones. StewardGet asks
  the first time; change it in **Settings â€º General**. **â‹¯ â€º Quit StewardGet** or
  Ctrl+Q quits.
- Recently used folders in the Add dialog and the new-download window.
- **Download later** in the Add dialog, with a queue to add to.
- Drop text files with links onto the window to add them.
- The details pane shows the download's queue.

### Fixed
- The sidebar's scroll bar no longer covers the counts and buttons.
- Installers and the one-line install scripts close StewardGet properly before an
  update (`stewardget-gui --quit`), even when its close button only minimises it.

## [0.2.0] - 2026-10-20

Easy installation on every platform, and browser setup built into the app.

### Install
- **Windows**: one `StewardGet-Setup.exe` for x64 and ARM64 PCs. It needs no
  administrator rights, adds StewardGet to the Start menu, and can add a desktop
  shortcut, start StewardGet when you sign in and put `stewardget` on the PATH. It
  updates an existing installation in place and closes a running StewardGet first.
  The uninstaller asks before deleting your settings and never touches your
  downloads. A portable zip keeps everything in its own folder.
- **macOS**: `StewardGet-macos.dmg` with a proper app for Apple silicon and Intel
  Macs.
- **Linux**: `.deb` and `.rpm` packages for x64 and ARM64, with a menu entry
  and icons, plus a tarball.
- One-line installers for every system (`install.sh` for Linux and macOS,
  `install.ps1` for Windows). They pick the right build, verify its checksum,
  install, and can uninstall again.

### First start
- A welcome dialog asks where to save downloads, offers to start StewardGet with
  your computer and leads to the browser setup.
- **Start with your computer** (Settings â€º General) opens StewardGet minimised when
  you sign in, on Windows, macOS and Linux.
- Opening StewardGet while it is already running brings the running window forward
  and hands over any links, instead of showing an error.

### Browser extension
- **Settings â€º Browser** now has a guided setup for Chrome, Edge, Brave,
  Opera, Vivaldi and Firefox, with buttons to copy the page address and the
  folder, and a live status that turns green once the extension connects.
- The extension ships inside the app and is kept up to date with it.
  `stewardget extension` prints the setup steps in a terminal.
- The status bar only says "Browser connected" when an extension really is
  connected.
- The extension's popup explains what is wrong when it can't reach StewardGet
  (not running, wrong token or port), and asks Firefox for site access when
  that is missing.
- Downloads from private windows stay in the browser.

### Fixes
- Opening files on Windows no longer flashes a console window, and file names
  containing `&` or `%` open correctly.
- "When all downloads finish" actions and finished-download notices now also
  happen while the window is minimised.

## [0.1.0] - 2026-10-20

The first public release.

### Downloading
- Splits each file across up to 32 connections. When one connection finishes,
  it takes over half of the largest remaining part, so all of them stay busy
  until the last byte.
- Writes straight into the final file. There is no separate "assembling" step.
- Pause, resume and restart. Downloads continue after a restart or a crash,
  because progress is saved every second.
- Backs off automatically when a server limits connections, retries network
  errors, and retries failed downloads again later.
- Global and per-download speed limits, a queue with a configurable number of
  simultaneous downloads, and a scheduler with start and pause times.
- Checksum verification (MD5, SHA-1, SHA-256, SHA-512), proxy support, custom
  headers, cookies, referrer and user agent.

### Your data is safe
- Partial data is never thrown away silently. If the file on the server
  changes, StewardGet stops and lets you resume anyway, refresh the link or start
  over.
- Removing a download keeps its partial data unless you choose to delete it.
  Adding the same link again continues it.
- Checks free disk space before downloading.
- Keeps backups of the download list.

### Desktop app
- Layout in the spirit of IDM: categories, a sortable list, a details pane
  with a live connection map and speed graph, and statistics.
- The Add dialog checks a link before downloading it. It shows the real file
  name, size, resume support and free space, warns about duplicates and
  expands batch patterns such as `img[001-120].jpg`.
- **Refresh link**: when a link expires, StewardGet opens the page, catches the
  new link from the browser and continues the same file.
- Properties to rename or move files on disk and to change the link, the
  connections or the limits.
- Optional "Download complete" dialog. After the queue finishes, StewardGet can
  quit, sleep or shut down, with a countdown you can cancel.
- Five themes (System, Light, Dark, Midnight, Ocean), six accent colours and
  interface zoom. Uses the Inter typeface and Lucide icons.

### Browser extension
- Takes over downloads in Chrome, Edge, Brave, Opera and Firefox, and adds
  "Download with StewardGet" to the context menu.
- Passes cookies, the referrer and the user agent, so downloads that need a
  login keep working.

### Terminal
- `stewardget`: an interactive terminal dashboard.
- `stewardget get`: one-off downloads with progress bars.
- `stewardget add`: sends links to the running app.

[0.7.4]: https://github.com/anand34577/stewardget/releases/tag/v0.7.4
[0.7.3]: https://github.com/anand34577/stewardget/releases/tag/v0.7.3
[0.7.2]: https://github.com/anand34577/stewardget/releases/tag/v0.7.2
[0.7.1]: https://github.com/anand34577/stewardget/releases/tag/v0.7.1
[0.7.0]: https://github.com/anand34577/stewardget/releases/tag/v0.7.0
[0.6.0]: https://github.com/anand34577/stewardget/releases/tag/v0.6.0
[0.5.1]: https://github.com/anand34577/stewardget/releases/tag/v0.5.1
[0.5.0]: https://github.com/anand34577/stewardget/releases/tag/v0.5.0
[0.4.0]: https://github.com/anand34577/stewardget/releases/tag/v0.4.0
[0.3.0]: https://github.com/anand34577/stewardget/releases/tag/v0.3.0
[0.2.0]: https://github.com/anand34577/stewardget/releases/tag/v0.2.0
[0.1.0]: https://github.com/anand34577/stewardget/releases/tag/v0.1.0
