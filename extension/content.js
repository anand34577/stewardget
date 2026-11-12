// StewardGet in the page. Two small things, nothing is read or sent anywhere
// except to the StewardGet app on this computer, and only when you click:
//
// 1. Alt+click on a link keeps that download in the browser (like IDM).
// 2. A "Download" button over videos, like IDM's: click it to pick a format
//    (found by yt-dlp in StewardGet, plus the streams this page played), or
//    close it with ×. "Hide on this site" turns it off for a site; the
//    extension popup turns it off everywhere.

(() => {
  const ext = globalThis.browser ?? globalThis.chrome;
  if (!ext?.runtime?.id) return;

  document.addEventListener(
    "click",
    (e) => {
      if (!e.altKey) return;
      const link = e.target instanceof Element ? e.target.closest("a[href], area[href]") : null;
      if (!link?.href) return;
      send({ type: "bypass", url: link.href });
    },
    true,
  );

  /** Messages the background script; resolves to null when the extension was reloaded. */
  function send(msg) {
    try {
      return Promise.resolve(ext.runtime.sendMessage(msg)).catch(() => null);
    } catch {
      return Promise.resolve(null);
    }
  }

  // ---- Settings ---------------------------------------------------------------

  const SETTINGS = { videoButton: true, videoButtonSkipSites: "" };
  let settings = { ...SETTINGS };
  const host = location.hostname.toLowerCase();

  function siteList(text) {
    return (text || "")
      .split(/[\s,;]+/)
      .map((s) => s.trim().toLowerCase().replace(/^\*\./, "").replace(/^https?:\/\//, "").replace(/\/.*$/, ""))
      .filter(Boolean);
  }

  function enabledHere() {
    if (!settings.videoButton) return false;
    return !siteList(settings.videoButtonSkipSites).some((s) => host === s || host.endsWith(`.${s}`));
  }

  try {
    ext.storage.local.get(SETTINGS).then((s) => {
      settings = { ...SETTINGS, ...s };
      if (!enabledHere()) hide(true);
    });
    ext.storage.onChanged.addListener((changes, area) => {
      if (area !== "local") return;
      for (const k of Object.keys(SETTINGS)) if (changes[k]) settings[k] = changes[k].newValue;
      if (!enabledHere()) hide(true);
    });
  } catch {
    return;
  }

  // ---- Which video ------------------------------------------------------------

  const MIN_W = 200;
  const MIN_H = 110;
  /** Videos whose button was closed with ×, for as long as the page lives. */
  const dismissed = new WeakSet();
  let current = null; // the video the button belongs to
  let lastNear = 0; // when the pointer was last over it (ms)
  let pointerX = -1; // where the pointer is, -1 when it left the page
  let pointerY = -1;
  let panelOpen = false;

  function bigEnough(v) {
    const r = v.getBoundingClientRect();
    return r.width >= MIN_W && r.height >= MIN_H && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth;
  }

  function usable(v) {
    return v instanceof HTMLVideoElement && v.isConnected && !dismissed.has(v) && bigEnough(v) && (v.currentSrc || v.src || v.querySelector("source"));
  }

  /** The video under a point; players cover it with their own controls, so look through them. */
  function videoAt(x, y) {
    let stack = [];
    try {
      stack = document.elementsFromPoint(x, y);
    } catch {
      return null;
    }
    for (const el of stack) {
      if (el instanceof HTMLVideoElement) return el;
    }
    // Some players keep the video behind a sibling layer that isn't hit: any
    // big video whose box holds the point.
    for (const v of document.getElementsByTagName("video")) {
      const r = v.getBoundingClientRect();
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return v;
    }
    return null;
  }

  let moveQueued = false;
  document.addEventListener(
    "pointermove",
    (e) => {
      pointerX = e.clientX;
      pointerY = e.clientY;
      if (moveQueued || !enabledHere()) return;
      moveQueued = true;
      const { clientX: x, clientY: y } = e;
      requestAnimationFrame(() => {
        moveQueued = false;
        if (hostEl && hostEl.matches(":hover")) {
          lastNear = Date.now();
          return;
        }
        const v = videoAt(x, y);
        if (v && usable(v)) {
          lastNear = Date.now();
          attach(v); // also brings it back after it faded out
        }
      });
    },
    { capture: true, passive: true },
  );

  // The pointer left the window: the button may fade.
  addEventListener(
    "mouseout",
    (e) => {
      if (!e.relatedTarget) pointerX = pointerY = -1;
    },
    true,
  );

  // A video starting to play (autoplay, or started from the keyboard) gets the button too.
  document.addEventListener(
    "play",
    (e) => {
      const v = e.target;
      if (!enabledHere() || !(v instanceof HTMLVideoElement) || !usable(v)) return;
      if (!current || current === v || !current.isConnected || current.paused) {
        lastNear = Date.now();
        attach(v);
      }
    },
    true,
  );

  // ---- The button -------------------------------------------------------------

  let hostEl = null;
  let root = null;
  let ui = null;

  const STYLE = `
    :host { all: initial; position: fixed; z-index: 2147483647; top: 0; left: 0; pointer-events: none; }
    * { box-sizing: border-box; font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
    .wrap { position: absolute; display: flex; flex-direction: column; align-items: flex-end; gap: 6px; pointer-events: none;
      opacity: 0; transform: translateY(-4px); transition: opacity .15s ease, transform .15s ease; }
    /* Only a visible button takes clicks: a faded one must not swallow clicks meant for the player. */
    .wrap.on { opacity: 1; transform: none; pointer-events: auto; }
    .bar { display: flex; align-items: stretch; border-radius: 9px; overflow: hidden; background: rgba(17,20,27,.92);
      box-shadow: 0 2px 10px rgba(0,0,0,.35); border: 1px solid rgba(255,255,255,.12); backdrop-filter: blur(6px); }
    button { all: unset; box-sizing: border-box; cursor: pointer; color: #f2f4f8; font-size: 13px; line-height: 1; display: flex; align-items: center; gap: 7px; }
    button:focus-visible { outline: 2px solid #2dd4bf; outline-offset: -2px; }
    .main { padding: 8px 11px 8px 10px; font-weight: 600; }
    .main:hover { background: rgba(45,212,191,.18); }
    .main svg { width: 16px; height: 16px; flex: none; }
    .close { padding: 0 9px; color: #9aa3b5; border-left: 1px solid rgba(255,255,255,.1); font-size: 15px; }
    .close:hover { color: #fff; background: rgba(255,255,255,.08); }
    .panel { width: 340px; max-width: calc(100vw - 24px); max-height: min(420px, calc(100vh - 80px)); overflow: hidden auto;
      background: #151922; color: #e8eaf0; border-radius: 12px; border: 1px solid rgba(255,255,255,.12);
      box-shadow: 0 10px 34px rgba(0,0,0,.45); padding: 10px; display: none; font-size: 13px; }
    .panel.on { display: block; }
    .title { font-weight: 600; margin: 2px 4px 2px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .sub { color: #8f98aa; font-size: 12px; margin: 0 4px 8px; }
    .group { color: #8f98aa; font-size: 11px; font-weight: 600; letter-spacing: .03em; margin: 10px 4px 4px; }
    .item { width: 100%; padding: 8px 8px; border-radius: 8px; justify-content: space-between; gap: 10px; }
    .item:hover { background: rgba(45,212,191,.14); }
    .item[aria-disabled="true"] { opacity: .45; cursor: default; }
    .item[aria-disabled="true"]:hover { background: none; }
    .item .l { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .item .r { color: #8f98aa; font-size: 12px; flex: none; }
    .msg { padding: 6px 4px; color: #c9cfdb; line-height: 1.4; }
    .msg.err { color: #f6a5a5; }
    .msg.ok { color: #6ee7b7; }
    .spin { display: inline-block; width: 12px; height: 12px; border: 2px solid #2dd4bf; border-right-color: transparent;
      border-radius: 50%; animation: s .8s linear infinite; vertical-align: -2px; margin-right: 6px; }
    @keyframes s { to { transform: rotate(360deg); } }
    .foot { display: flex; justify-content: space-between; margin-top: 8px; padding-top: 8px; border-top: 1px solid rgba(255,255,255,.08); }
    .link { color: #8f98aa; font-size: 12px; padding: 4px; }
    .link:hover { color: #fff; }
    @media (prefers-reduced-motion: reduce) { .wrap { transition: none; } .spin { animation: none; } }
  `;

  const ICON =
    '<svg viewBox="0 0 24 24" fill="none" stroke="#2dd4bf" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/></svg>';

  function build() {
    hostEl = document.createElement("stewardget-video-button");
    root = hostEl.attachShadow({ mode: "closed" });
    root.innerHTML = `<style>${STYLE}</style>
      <div class="wrap" part="wrap">
        <div class="bar">
          <button class="main" title="Download this video with StewardGet">${ICON}<span>Download</span></button>
          <button class="close" title="Hide for this video" aria-label="Hide">×</button>
        </div>
        <div class="panel" role="menu"></div>
      </div>`;
    ui = {
      wrap: root.querySelector(".wrap"),
      main: root.querySelector(".main"),
      close: root.querySelector(".close"),
      panel: root.querySelector(".panel"),
    };
    ui.main.addEventListener("click", (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (panelOpen) closePanel();
      else openPanel();
    });
    ui.close.addEventListener("click", (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (current) dismissed.add(current);
      hide(true);
    });
    // Clicks inside must not reach the player (play/pause) below.
    for (const t of ["pointerdown", "mousedown", "mouseup", "dblclick"]) {
      hostEl.addEventListener(t, (e) => e.stopPropagation());
    }
    document.addEventListener(
      "pointerdown",
      (e) => {
        if (panelOpen && !e.composedPath().includes(hostEl)) closePanel();
      },
      true,
    );
    document.addEventListener("keydown", (e) => {
      if (panelOpen && e.key === "Escape") closePanel();
    });
    document.addEventListener("fullscreenchange", place, true);
    document.addEventListener("webkitfullscreenchange", place, true);
  }

  /** The button lives in the fullscreen element while there is one, or it can't be seen. */
  function place() {
    if (!hostEl) return;
    const parent = document.fullscreenElement || document.webkitFullscreenElement || document.documentElement;
    if (hostEl.parentNode !== parent) {
      try {
        parent.appendChild(hostEl);
      } catch {
        document.documentElement.appendChild(hostEl);
      }
    }
  }

  function attach(v) {
    if (!enabledHere()) return;
    if (!hostEl) build();
    if (current !== v) {
      current = v;
      closePanel();
    }
    place();
    ui.wrap.classList.add("on");
    track();
  }

  function hide(now) {
    if (!ui) return;
    closePanel();
    ui.wrap.classList.remove("on");
    if (now) current = null;
  }

  let tracking = false;
  /** Follows the video while the button shows; stops when it hides. */
  function track() {
    if (tracking) return;
    tracking = true;
    const step = () => {
      if (!current || !ui) {
        tracking = false;
        return;
      }
      const r = current.getBoundingClientRect();
      // Stays while the pointer rests on the video (or the button), and a
      // moment after it leaves.
      const over = pointerX >= r.left && pointerX <= r.right && pointerY >= r.top && pointerY <= r.bottom;
      if (over) lastNear = Date.now();
      const near = panelOpen || over || Date.now() - lastNear < 2000 || (hostEl && hostEl.matches(":hover"));
      if (!current.isConnected || !bigEnough(current) || !near || !enabledHere()) {
        hide(!current.isConnected);
        tracking = false;
        return;
      }
      const w = ui.wrap.offsetWidth || 140;
      const top = Math.max(8, r.top + 10);
      const left = Math.min(innerWidth - w - 8, Math.max(8, r.right - w - 10));
      ui.wrap.style.top = `${Math.round(top)}px`;
      ui.wrap.style.left = `${Math.round(left)}px`;
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }

  // ---- The list of formats ----------------------------------------------------

  /** pageUrl → { at, promise } so reopening the list doesn't ask again. */
  const cache = new Map();
  const CACHE_MS = 10 * 60 * 1000;

  function sizeText(bytes) {
    if (!bytes) return "";
    const units = ["B", "KB", "MB", "GB"];
    let i = 0;
    let v = bytes;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
  }

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function closePanel() {
    panelOpen = false;
    if (ui) {
      ui.panel.classList.remove("on");
      ui.panel.replaceChildren();
    }
  }

  function formatsFor(pageUrl) {
    const hit = cache.get(pageUrl);
    if (hit && Date.now() - hit.at < CACHE_MS) return hit.promise;
    const promise = send({ type: "video-formats", pageUrl, title: document.title }).then((r) => {
      // Failures aren't kept: the next click tries again.
      if (!r?.token) cache.delete(pageUrl);
      return r;
    });
    cache.set(pageUrl, { at: Date.now(), promise });
    return promise;
  }

  /** The video's own file, when it plays from a plain address (not a stream in a blob). */
  function directSource(v) {
    const src = v?.currentSrc || v?.src || "";
    return /^https?:/i.test(src) ? src : "";
  }

  async function openPanel() {
    panelOpen = true;
    const panel = ui.panel;
    panel.classList.add("on");
    panel.replaceChildren();
    const pageUrl = location.href.split("#")[0];
    const title = el("div", "title", document.title || host);
    const status = el("div", "msg");
    status.append(el("span", "spin"), document.createTextNode("Finding formats… this can take a few seconds."));
    panel.append(title, status);
    const video = current;
    const r = await formatsFor(pageUrl);
    if (!panelOpen || current !== video) return;
    panel.replaceChildren();
    render(panel, r, video);
  }

  function render(panel, r, video) {
    panel.append(el("div", "title", r?.title || document.title || host));
    if (r?.offline) {
      panel.append(el("div", "msg err", "StewardGet isn't running. Start it, then try again."));
    } else if (r?.error) {
      panel.append(el("div", "msg err", r.error));
      if (r.ytdlp === false) {
        panel.append(el("div", "msg", "Install yt-dlp in StewardGet: Settings › Integrations › Install yt-dlp (one click)."));
      }
    }
    const options = r?.options ?? [];
    if (options.length) {
      panel.append(el("div", "sub", r.ffmpeg ? `${options.length} formats` : `${options.length} formats · install ffmpeg in StewardGet for the sharpest ones`));
      const videos = options.filter((o) => o.height > 0);
      const audio = options.filter((o) => o.height === 0);
      if (videos.length) panel.append(el("div", "group", "VIDEO"));
      for (const o of videos) panel.append(item(o.label.replace(/ \((picture and sound|joined with ffmpeg)\)$/, ""), o.size, o.available, () => pick({ type: "video-download", token: r.token, index: o.index }), o.available ? "" : "Needs ffmpeg (install it in StewardGet › Settings › Integrations)"));
      if (audio.length) panel.append(el("div", "group", "AUDIO"));
      for (const o of audio) panel.append(item(o.label.replace(/^Audio only · /, ""), o.size, o.available, () => pick({ type: "video-download", token: r.token, index: o.index })));
    }
    // What the page itself played: direct files and streams, found without yt-dlp.
    const seen = new Set();
    const played = [];
    const direct = directSource(video);
    if (direct) played.push({ url: direct, kind: "file", name: "This video's file", size: 0 });
    for (const m of r?.sniffed ?? []) played.push(m);
    const fresh = played.filter((m) => !seen.has(m.url) && seen.add(m.url));
    if (fresh.length) {
      panel.append(el("div", "group", "PLAYED ON THIS PAGE"));
      for (const m of fresh.slice(0, 8)) {
        const label = m.kind === "hls" ? `Stream (HLS) · ${m.name || "video"}` : m.name || "Video file";
        panel.append(item(label, m.size, true, () => pick({ type: "grab-media", url: m.url })));
      }
    }
    if (!options.length && !fresh.length && !r?.error && !r?.offline) {
      panel.append(el("div", "msg", "Nothing to download was found. Play the video for a moment and try again."));
    }
    const foot = el("div", "foot");
    const skip = el("button", "link", `Hide on ${host.replace(/^www\./, "")}`);
    skip.addEventListener("click", async (e) => {
      e.stopPropagation();
      const list = siteList(settings.videoButtonSkipSites);
      list.push(host.replace(/^www\./, ""));
      await ext.storage.local.set({ videoButtonSkipSites: [...new Set(list)].join(", ") }).catch(() => {});
      hide(true);
    });
    const again = el("button", "link", "Look again");
    again.addEventListener("click", (e) => {
      e.stopPropagation();
      cache.delete(location.href.split("#")[0]);
      openPanel();
    });
    foot.append(skip, again);
    panel.append(foot);
  }

  function item(label, size, enabled, onPick, why) {
    const b = el("button", "item");
    b.setAttribute("role", "menuitem");
    b.title = why || label;
    if (!enabled) b.setAttribute("aria-disabled", "true");
    b.append(el("span", "l", label), el("span", "r", sizeText(size)));
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (enabled) onPick();
    });
    return b;
  }

  async function pick(msg) {
    const panel = ui.panel;
    panel.replaceChildren(el("div", "msg", "Sending to StewardGet…"));
    const r = await send(msg);
    if (!panelOpen) return;
    panel.replaceChildren();
    if (r?.ok) {
      panel.append(el("div", "msg ok", "✓ Downloading in StewardGet"));
      setTimeout(() => panelOpen && closePanel(), 1600);
    } else {
      panel.append(el("div", "msg err", r?.error || "StewardGet couldn't take it. Is it running?"));
    }
  }
})();
