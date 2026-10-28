# Architecture

StewardGet is a Cargo workspace with one engine library and three front-ends.

```
               ┌──────────────┐  ┌──────────────┐  ┌─────────────────────┐
               │  stewardget-gui   │  │  stewardget (TUI  │  │ browser extension / │
               │   (egui)     │  │   + CLI)     │  │   `stewardget add`       │
               └──────┬───────┘  └──────┬───────┘  └──────────┬──────────┘
                      │ Engine API       │                     │ HTTP, 127.0.0.1
                      ▼                  ▼                     ▼
               ┌─────────────────────────────────────────────────────────┐
               │                       stewardget-core                        │
               │  Engine ─ queue/scheduler ─ sampler ─ store ─ local API │
               │     │                                                   │
               │     └─ task (one per active download)                   │
               │          probe → segment table → N connection workers   │
               └─────────────────────────────────────────────────────────┘
```

## Engine (`engine.rs`)

`Engine` is a cheap, cloneable handle around shared state behind one mutex.
Every public method is non-blocking. Front-ends poll `snapshot()`, which copies
everything a dashboard needs, and can subscribe to `Event`s.

- **Scheduler**: starts `Queued` downloads while fewer than `max_concurrent` run.
- **Sampler**: every 500 ms it reads each running task's byte counter, computes an
  EWMA-smoothed speed, appends to the speed history and updates statistics.
  Every 2 s it persists dirty state.
- **Store**: `settings.json`, `downloads.json` and `stats.json`, written atomically
  (write temp file, then rename). A lock file ensures one engine per data directory.
  Each file keeps a rolling `.bak` copy (refreshed at most once a minute); a file
  that fails to parse is renamed to `*.corrupt-<time>` and the backup is loaded.

## A download (`task.rs`)

1. **Probe**: a single `GET` with `Range: bytes=0-0` reveals size, range support,
   validators (`ETag`, `Last-Modified`), the final URL after redirects and the
   file name. This is more reliable than `HEAD`, which many CDNs answer wrongly.
2. **Destination**: the engine resolves a unique, sanitised path (or reuses the
   one recorded earlier) and the task opens `<name>.vxpart`. Fresh files are
   pre-allocated as sparse files. On Windows the file is flagged sparse first,
   otherwise NTFS would zero-fill up to every far-away write.
3. **Resume check**: `<name>.vxmeta` stores the segment table. It is reused when
   the size matches and at least one validator (`ETag` or `Last-Modified`)
   agrees, or on size alone after the user refreshed the link or chose *Resume
   anyway*. If partial data exists but can't be used (different size, changed
   validators, ranges no longer supported) the download **stops with an error and
   keeps the data**; it is only discarded by an explicit *Start over*.
   A free-space check runs before any data is written.
4. **Workers**: up to `connections` tasks claim segments from the shared
   `SegmentTable`, stream with `Range` requests (`If-Range` guarded) and write
   with positional I/O (`pwrite` / `seek_write`) in 512 KiB batches, so no task
   ever shares a file cursor.
5. **Checkpoint** every 3 seconds (each one flushes the data to disk first); on pause, crash or shutdown the control file
   records exactly which bytes are on disk.
6. **Finalize**: verify size, optional checksum, `fsync`, rename into place,
   restore the server's modification time, delete the control file.

### Dynamic segmentation (`segment.rs`)

A segment is `(start, end, received, written)`. When a worker finishes, it
`claim`s new work: first any idle unfinished segment, otherwise it **halves the
active segment with the most bytes left** (if both halves are at least
`min_split_size`). The original worker sees its `end` shrink on the next chunk
and stops there. All connections stay busy until the very end, which is where
static splitting loses the most time.

### Failure handling

- Network errors, timeouts, 408/425/429/5xx: per-worker retry with exponential
  back-off (0.5 s · 2ⁿ, ≤ 30 s) plus jitter. The attempt counter resets whenever
  the connection made progress.
- **Connection-limited servers**: a worker that keeps failing without receiving
  data while siblings are healthy releases its range and exits; others steal it.
- **Expiring signed links**: when the link redirected (GitHub release assets,
  S3) and a range request is refused with 401/403/404/410, the original link is
  probed again for a fresh redirect; it is used when size and `ETag` still
  match (at most 50 times per run). A captured download also keeps the
  browser's final address (`fallback_url`), used when the original link no
  longer works at all (a one-time link).
- Fatal errors (4xx, range ignored, remote changed, disk errors) stop the whole
  download and are reported in plain language.
- **Automatic retry**: a download that still fails with a retryable error is
  re-queued after `auto_retry_delay_secs · 2ⁿ`, up to `auto_retry` times.

### Scheduler

The sampler checks the local time once a minute: at the start time (on enabled
weekdays) paused and queued downloads start; at the stop time everything pauses.

### Rate limiting (`limiter.rs`)

A token bucket with debt: callers take tokens first and sleep off any deficit.
One global and one per-download bucket. Rates change live.

## Browser integration (`api.rs`)

A minimal HTTP/1.1 server on `127.0.0.1:<api_port>`, started only by the
persistent (single-instance) engine:

- `GET /api/v1/ping`, `GET /api/v1/downloads`, `POST /api/v1/downloads`,
  `POST /api/v1/show`, plus `GET /api/v1/downloads/{id}`,
  `POST /api/v1/downloads/{id}/pause|resume|restart`,
  `DELETE /api/v1/downloads/{id}`, `GET /api/v1/stats`, `GET /api/v1/queues`,
  `POST /api/v1/pause-all|resume-all|speed-limit`. The same listener serves
  `POST /mcp` (below) and runs when either the browser API or MCP is on.
- Requests with a web `Origin` are rejected, and so are requests whose `Host`
  isn't `127.0.0.1`, `localhost` or `[::1]` (DNS rebinding). Everything but
  `ping` needs an `X-StewardGet-Client` header, which forces a CORS pre-flight that
  is never approved for web pages. An optional bearer token can be required.

### MCP (`mcp.rs`)

`POST /mcp` takes JSON-RPC 2.0 messages (the Model Context Protocol's HTTP
transport, answered with plain JSON). It is off unless `mcp_enabled`, needs
its own bearer token (`mcp_token`, created when switched on) and, like the rest
of the API, refuses web origins and non-loopback hosts. The tools call the
same `Engine` methods as the GUI. `add_download` only accepts a plain
sub-folder of the download folder. `stewardget mcp` relays stdin/stdout to this
endpoint for clients that start a program.

### Site grabber (`crawl.rs`)

`Engine::grab_links` reads static HTML, one page at a time with a short pause,
honours `robots.txt` (`User-agent: *`), stays on the starting site unless told
otherwise and stops at 100 pages or 5000 files. It returns file URLs and the
page each came from; the desktop app feeds them to the new-download window.

`Engine::capture` implements **link recapture**: if any download is marked
`awaiting_link` and the captured file has the same size (or the same name when a
size is unknown), its URL is replaced (`set_url`) and it resumes with relaxed
validation; otherwise a new download is queued.

In Chromium the extension takes downloads in `downloads.onDeterminingFilename`,
which the browser waits for before it shows a Save As dialog or its download
bubble; it also carries the browser's file name. Firefox has no such event, so
there the download is paused in `onCreated` while StewardGet is asked. The
extension sends the link as clicked (`url`) and where it landed
(`fallback_url`). A browser name that is only the link's last segment is not
used, so the server's `Content-Disposition` name wins.

The extension pings once a minute with `X-StewardGet-Browser` and
`X-StewardGet-Extension-Version`, which the snapshot exposes as `extension`, so the
app can show "Connected". Its files are compiled into the binaries
(`extension.rs`) and written to `<data dir>/browser-extension` (Chromium) and
`<data dir>/browser-extension-firefox` on every start when they changed (the
two differ only in `manifest.json`: Chromium rejects `background.scripts` in
Manifest V3 and Firefox does not run a service worker), so every kind of install has the same folder to load and the
browser picks up the new version after an update.

## Single instance

The data directory holds an exclusive lock (`stewardget.lock`), so only one engine
owns the download list. When the desktop app starts while another instance holds
the lock, it hands its URLs to the running one (`remote::add`) and asks it to
show its window (`remote::show`), then exits. The running app registers a show
handler with the engine; eframe's `App::logic` handles the request because it
also runs while the window is minimised, when `App::ui` is skipped.

## Front-ends

- **GUI** (`stewardget-gui`, egui/eframe with the OpenGL backend): toolbar, category
  sidebar, virtualised sortable table, details pane with a connection map and
  speed graph, settings and statistics pages. Text uses the bundled Inter
  typeface and icons come from a subset of the Lucide icon font (`icons.rs`),
  so nothing depends on system or emoji fonts. Themes and accents live in
  `theme.rs`; every control shares one height (`CONTROL_H`) so rows line up.
- **TUI** (`stewardget`, ratatui): the same model in a terminal. Columns adapt to width.
- **CLI** (`stewardget get`): an in-memory engine plus indicatif progress bars;
  partial files make re-runs resume.

## Testing

`crates/stewardget-core/tests/engine.rs` runs the real engine against an in-process
HTTP server that can throttle, drop connections, return 503s, cap concurrent
connections and change ETags. It covers segmentation, fallback without ranges,
pause/resume, restart persistence, speed limits, checksums, link refresh and the
API's security rules.
