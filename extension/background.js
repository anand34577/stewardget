// StewardGet browser integration.
//
// Hands browser downloads to the StewardGet app over its loopback-only API
// (http://127.0.0.1:<port>). Nothing is ever sent anywhere else.

const ext = globalThis.browser ?? globalThis.chrome;
const VERSION = ext.runtime.getManifest().version;
const BROWSER = detectBrowser();

const DEFAULTS = {
  enabled: true,
  port: 7853,
  token: "",
  // Smaller files stay in the browser (0 = capture everything).
  minSizeKb: 0,
  // Comma-separated extensions the browser should keep handling.
  skipExtensions: "",
  // Comma-separated sites whose downloads stay in the browser ("example.com"
  // also covers its subdomains).
  skipSites: "",
};

/** Links Alt+clicked in a page (see content.js), kept in the browser for a few seconds. */
const bypass = new Map();
const BYPASS_MS = 15000;

function stripHash(url) {
  return (url || "").split("#")[0];
}

function bypassed(item) {
  const now = Date.now();
  for (const [url, until] of bypass) if (until < now) bypass.delete(url);
  return [item.url, item.finalUrl].some((u) => u && bypass.has(stripHash(u)));
}

/** Host name of a URL, or "" when it has none. */
function hostOf(url) {
  try {
    return new URL(url).hostname.toLowerCase();
  } catch {
    return "";
  }
}

/** Parses "example.com, *.bank.com" into bare host names. */
function siteList(text) {
  return (text || "")
    .split(/[\s,;]+/)
    .map((s) => s.trim().toLowerCase().replace(/^\*\./, "").replace(/^https?:\/\//, "").replace(/\/.*$/, ""))
    .filter(Boolean);
}

/** `host` is `site` or one of its subdomains. */
function onSite(host, site) {
  return host === site || host.endsWith(`.${site}`);
}

/** Best-effort browser name, shown in StewardGet's settings ("Connected to Edge"). */
function detectBrowser() {
  const ua = navigator.userAgent;
  if (/Firefox\//.test(ua)) return "Firefox";
  if (/Edg\//.test(ua)) return "Edge";
  if (/OPR\//.test(ua)) return "Opera";
  if (/Vivaldi\//.test(ua)) return "Vivaldi";
  const brands = (navigator.userAgentData?.brands ?? []).map((b) => b.brand);
  if (navigator.brave || brands.includes("Brave")) return "Brave";
  if (brands.includes("Google Chrome")) return "Chrome";
  return "Chromium";
}

async function settings() {
  return { ...DEFAULTS, ...(await ext.storage.local.get(DEFAULTS)) };
}

function apiUrl(s, path) {
  return `http://127.0.0.1:${s.port}/api/v1/${path}`;
}

function headers(s) {
  const h = {
    "Content-Type": "application/json",
    "X-StewardGet-Client": "extension",
    "X-StewardGet-Browser": BROWSER,
    "X-StewardGet-Extension-Version": VERSION,
  };
  if (s.token) h.Authorization = `Bearer ${s.token}`;
  return h;
}

/** Resolves to `{ ok, version, status }`: whether StewardGet answered, and how. */
async function ping() {
  const s = await settings();
  try {
    const r = await fetch(apiUrl(s, "ping"), { headers: headers(s), signal: AbortSignal.timeout(1500) });
    if (!r.ok) return { ok: false, status: r.status };
    const body = await r.json().catch(() => ({}));
    return { ok: true, version: body.version ?? "" };
  } catch {
    return { ok: false, status: 0 };
  }
}

/** Cookies for `url`; `storeId` picks a Firefox container (or private) cookie jar. */
async function cookieHeader(url, storeId) {
  try {
    const cookies = await ext.cookies.getAll(storeId ? { url, storeId } : { url });
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ") || null;
  } catch {
    return null;
  }
}

/** Most links "Download all links" sends at once. */
const MAX_LINKS = 1500;

/** Why the last request to StewardGet failed, in words for the popup. */
let lastFailure = "StewardGet did not answer";

/** Remembers what happened to the latest download, for the popup. */
function note(ok, text) {
  ext.storage.local.set({ lastCapture: { at: Date.now(), ok, text } }).catch(() => {});
}

function describeFailure(status) {
  if (status === 401) return "StewardGet refused the token; it must match Settings > Browser in StewardGet";
  if (status === 403) return "StewardGet refused the request";
  if (status) return `StewardGet answered with an error (${status})`;
  return "StewardGet is not running or the port is wrong";
}

function shortName(item) {
  const name = (item.filename || "").split(/[\\/]/).pop();
  if (name) return name;
  try {
    return decodeURIComponent(new URL(item.finalUrl || item.url).pathname.split("/").pop()) || "a download";
  } catch {
    return "a download";
  }
}

/** Sends one download to StewardGet. Resolves to the API result, or null if StewardGet is unreachable. */
async function sendToStewardGet({ url, fallbackUrl, fileName, size, referrer, storeId }) {
  const s = await settings();
  const body = {
    url,
    fallback_url: fallbackUrl && fallbackUrl !== url ? fallbackUrl : null,
    file_name: fileName || null,
    size: size > 0 ? size : null,
    referrer: referrer || null,
    user_agent: navigator.userAgent,
    cookies: await cookieHeader(url, storeId),
  };
  try {
    const r = await fetch(apiUrl(s, "downloads"), {
      method: "POST",
      headers: headers(s),
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(4000),
    });
    if (!r.ok) {
      lastFailure = describeFailure(r.status);
      return null;
    }
    const [result] = await r.json();
    return result ?? null;
  } catch {
    lastFailure = describeFailure(0);
    return null;
  }
}

/**
 * Sends every link of a tab to StewardGet, which shows them in a list to pick from.
 * Resolves to the number of links sent, 0 when there were none, or null when
 * the page can't be read (browser pages, the web store) or StewardGet is unreachable.
 */
async function sendAllLinks(tab) {
  if (!tab?.id) return null;
  let links;
  try {
    const [frame] = await ext.scripting.executeScript({ target: { tabId: tab.id }, func: collectLinks });
    links = frame?.result ?? [];
  } catch {
    return null;
  }
  links = links.slice(0, MAX_LINKS);
  if (!links.length) return 0;
  const s = await settings();
  // One cookie lookup per site, not per link.
  const cookies = new Map();
  const items = [];
  for (const url of links) {
    let origin = "";
    try {
      origin = new URL(url).origin;
    } catch {
      continue;
    }
    if (!cookies.has(origin)) cookies.set(origin, await cookieHeader(url, tab.cookieStoreId));
    items.push({ url, referrer: tab.url || null, user_agent: navigator.userAgent, cookies: cookies.get(origin) });
  }
  try {
    const r = await fetch(apiUrl(s, "downloads"), {
      method: "POST",
      headers: headers(s),
      body: JSON.stringify({ items }),
      signal: AbortSignal.timeout(15000),
    });
    return r.ok ? items.length : null;
  } catch {
    return null;
  }
}

/** Runs in the page: its http(s) links and media sources, without duplicates. */
function collectLinks() {
  const page = location.href.split("#")[0];
  const seen = new Set();
  const out = [];
  const add = (raw) => {
    if (!raw) return;
    let url;
    try {
      url = new URL(raw, document.baseURI);
    } catch {
      return;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return;
    url.hash = "";
    const href = url.href;
    if (href === page || seen.has(href)) return;
    seen.add(href);
    out.push(href);
  };
  document.querySelectorAll("a[href], area[href]").forEach((a) => add(a.getAttribute("href")));
  document.querySelectorAll("img[src], video[src], audio[src], source[src]").forEach((m) => add(m.getAttribute("src")));
  return out;
}

function extensionOf(name) {
  const base = (name || "").split(/[\\/]/).pop().toLowerCase();
  const i = base.lastIndexOf(".");
  return i > 0 ? base.slice(i + 1) : "";
}

function shouldCapture(item, s) {
  if (!s.enabled) return false;
  // Private windows stay private: those downloads remain in the browser.
  if (item.incognito) return false;
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url)) return false; // blob:, data:, file: stay in the browser
  if (s.minSizeKb > 0 && item.totalBytes > 0 && item.totalBytes < s.minSizeKb * 1024) return false;
  const skip = s.skipExtensions
    .split(/[\s,;]+/)
    .map((e) => e.trim().replace(/^\./, "").toLowerCase())
    .filter(Boolean);
  let path = "";
  try {
    path = new URL(url).pathname;
  } catch {
    return false;
  }
  const ext_ = extensionOf(item.filename) || extensionOf(path);
  if (ext_ && skip.includes(ext_)) return false;
  // Sites the user excluded: the file's own host or the page it came from.
  const sites = siteList(s.skipSites);
  const hosts = [hostOf(url), hostOf(item.referrer)].filter(Boolean);
  if (sites.some((site) => hosts.some((h) => onSite(h, site)))) return false;
  return !bypassed(item);
}

// ---- Downloads that answer a form (POST) ---------------------------------------
//
// StewardGet can only repeat a plain GET. A file returned for a submitted form
// (search exports, invoices, "generate report") would come back wrong or as an
// error, so those downloads stay in the browser.

/** Page and frame URLs recently requested with POST → when that stops mattering. */
const posted = new Map();
const POST_MS = 60000;

ext.webRequest.onBeforeRequest.addListener(
  (d) => {
    if (d.method !== "POST") return;
    const now = Date.now();
    for (const [url, until] of posted) if (until < now) posted.delete(url);
    posted.set(stripHash(d.url), now + POST_MS);
  },
  { urls: ["http://*/*", "https://*/*"], types: ["main_frame", "sub_frame"] },
);

function fromForm(item) {
  const now = Date.now();
  return [item.url, item.finalUrl].some((u) => u && (posted.get(stripHash(u)) ?? 0) > now);
}

// ---- Media on the page (video and audio files, HLS streams) ---------------------

/** Most media entries kept per tab. */
const MAX_MEDIA = 40;
/** tabId → Map(url → { url, kind: "file" | "hls", type, size, at }) */
const media = new Map();

const MEDIA_EXT = ["mp4", "m4v", "webm", "mkv", "mov", "mp3", "m4a", "ogg", "oga", "opus", "flac", "wav", "aac"];
/** Pieces of a stream: never worth listing on their own. */
const SEGMENT_EXT = ["ts", "m4s", "cmfv", "cmfa", "fmp4"];

function headerValue(headers, name) {
  return headers?.find((h) => h.name.toLowerCase() === name)?.value ?? "";
}

/** What a response is, as far as the media list is concerned, or null. */
function classify(url, headers) {
  const type = headerValue(headers, "content-type").split(";")[0].trim().toLowerCase();
  let path = "";
  try {
    path = new URL(url).pathname.toLowerCase();
  } catch {
    return null;
  }
  const ext_ = extensionOf(path);
  if (type.includes("mpegurl") || ext_ === "m3u8") return { kind: "hls", type: "HLS stream" };
  if (SEGMENT_EXT.includes(ext_) || type === "video/mp2t" || type === "video/iso.segment") return null;
  const range = headerValue(headers, "content-range");
  const total = Number(range.split("/")[1]) || Number(headerValue(headers, "content-length")) || 0;
  const isMedia = type.startsWith("video/") || type.startsWith("audio/") || MEDIA_EXT.includes(ext_);
  // Tiny responses are previews, sounds and probes, not something to save.
  if (!isMedia || (total > 0 && total < 300 * 1024)) return null;
  return { kind: "file", type: type || ext_, size: total };
}

// The service worker is stopped when idle, which emptied the list (and the
// badge) after half a minute; it is kept in session storage as well.
const session = ext.storage.session;
const saveTimers = new Map();

/** The media entries of a tab, reloaded after the service worker restarted. */
async function mediaOf(tabId) {
  if (!media.has(tabId) && session) {
    const key = `media:${tabId}`;
    const saved = (await session.get(key).catch(() => ({})))[key];
    if (Array.isArray(saved) && !media.has(tabId)) media.set(tabId, new Map(saved.map((m) => [m.url, m])));
  }
  return media.get(tabId);
}

function saveMedia(tabId) {
  if (!session) return;
  clearTimeout(saveTimers.get(tabId));
  saveTimers.set(
    tabId,
    setTimeout(() => {
      saveTimers.delete(tabId);
      const key = `media:${tabId}`;
      const list = [...(media.get(tabId)?.values() ?? [])];
      (list.length ? session.set({ [key]: list }) : session.remove(key)).catch(() => {});
    }, 300),
  );
}

ext.webRequest.onHeadersReceived.addListener(
  async (d) => {
    if (d.tabId < 0 || d.statusCode >= 400) return;
    const found = classify(d.url, d.responseHeaders);
    if (!found) return;
    const url = stripHash(d.url);
    const list = (await mediaOf(d.tabId)) ?? new Map();
    const known = list.get(url);
    // Players fetch the same file in many ranges; keep one entry with the real size.
    list.set(url, { url, ...found, size: Math.max(found.size ?? 0, known?.size ?? 0), at: known?.at ?? Date.now() });
    while (list.size > MAX_MEDIA) list.delete(list.keys().next().value);
    media.set(d.tabId, list);
    saveMedia(d.tabId);
    showMediaCount(d.tabId);
  },
  { urls: ["http://*/*", "https://*/*"], types: ["media", "xmlhttprequest", "object", "other"] },
  ["responseHeaders"],
);

function showMediaCount(tabId) {
  const n = media.get(tabId)?.size ?? 0;
  ext.action.setBadgeBackgroundColor({ color: "#6366f1", tabId }).catch?.(() => {});
  ext.action.setBadgeText({ text: n ? String(n) : "", tabId }).catch?.(() => {});
}

// A new page starts with an empty list.
ext.tabs.onUpdated.addListener((tabId, change) => {
  if (change.status === "loading" && change.url) {
    media.set(tabId, new Map());
    saveMedia(tabId);
    showMediaCount(tabId);
  }
});
ext.tabs.onRemoved.addListener((tabId) => {
  media.set(tabId, new Map());
  saveMedia(tabId);
  media.delete(tabId);
});

/** A file name for a media entry: the URL's own name when it has one, else the page title. */
function mediaName(entry, tab) {
  let base = "";
  try {
    base = decodeURIComponent(new URL(entry.url).pathname.split("/").pop() || "");
  } catch {
    /* keep empty */
  }
  const generic = /^(index|master|playlist|manifest|videoplayback|video|media|stream|chunklist.*)\.\w+$/i;
  const title = (tab?.title || "video").replace(/[\\/:*?"<>|]+/g, " ").replace(/\s+/g, " ").trim().slice(0, 120);
  if (entry.kind === "hls") return `${title}.m3u8`; // StewardGet picks .ts or .mp4 for the result
  if (base && extensionOf(base) && !generic.test(base)) return base;
  const ext_ = MEDIA_EXT.find((e) => entry.type.includes(e)) || (entry.type.startsWith("audio/") ? "mp3" : "mp4");
  return `${title}.${ext_}`;
}

/** Downloads already handled, so one is never sent twice. */
const handled = new Set();

/** Sends a browser download to StewardGet. Resolves to whether StewardGet took it. */
async function handOver(item) {
  // The link as clicked, not where it redirected to: signed redirects
  // (GitHub releases, S3, CDNs) expire within minutes, which would break a
  // long or resumed download. StewardGet falls back to the final address when
  // the link was a one-time one.
  const original = /^https?:/i.test(item.url || "") ? item.url : item.finalUrl;
  const result = await sendToStewardGet({
    url: original,
    fallbackUrl: item.finalUrl,
    fileName: item.filename,
    size: item.totalBytes > 0 ? item.totalBytes : item.fileSize,
    referrer: item.referrer,
    storeId: item.cookieStoreId, // Firefox containers; undefined in Chromium
  });
  if (result) {
    setBadge(result.action === "refreshed" ? "↻" : "✓");
    note(true, `Sent ${shortName(item)} to StewardGet`);
  } else {
    setBadge("!");
    note(false, `${lastFailure}; the browser kept ${shortName(item)}`);
  }
  return Boolean(result);
}

async function dropBrowserCopy(id) {
  try {
    await ext.downloads.cancel(id);
  } catch {
    /* already gone */
  }
  try {
    await ext.downloads.erase({ id });
  } catch {
    /* ignore */
  }
}

/** Whether this download should go to StewardGet at all. */
async function wanted(item) {
  if (handled.has(item.id)) return false;
  const s = await settings();
  if (item.state !== "in_progress" || !shouldCapture(item, s)) return false;
  if (fromForm(item)) {
    note(true, `Kept ${shortName(item)} in the browser: it came from a form, which StewardGet can't repeat`);
    return false;
  }
  handled.add(item.id);
  return true;
}

if (ext.downloads.onDeterminingFilename) {
  // Chromium: the browser waits for this listener before it shows a Save As
  // dialog or its download bubble, however long the service worker took to
  // wake up. (Reacting in `onCreated` raced with both, so the browser's own
  // dialog appeared some of the time.) The item also carries the name the
  // browser settled on, from `Content-Disposition`, instead of an empty one.
  ext.downloads.onDeterminingFilename.addListener((item, suggest) => {
    const proceed = () => {
      try {
        suggest();
      } catch {
        /* the download is gone */
      }
    };
    (async () => {
      if (!(await wanted(item))) return proceed();
      if (await handOver(item)) {
        // Cancelled before any name was confirmed: no dialog, no bubble.
        await dropBrowserCopy(item.id);
      } else {
        proceed(); // StewardGet could not take it: the browser carries on, nothing is lost.
      }
    })().catch(proceed);
    return true; // answering asynchronously
  });
} else {
  // Firefox has no such event: hold the download while StewardGet is asked.
  ext.downloads.onCreated.addListener(async (item) => {
    if (!(await wanted(item))) return;
    try {
      await ext.downloads.pause(item.id);
    } catch {
      /* already finished or not pausable */
    }
    // A tiny file can finish before the pause lands; then the browser already has it.
    try {
      const [now] = await ext.downloads.search({ id: item.id });
      if (now && now.state !== "in_progress") return;
    } catch {
      /* keep going */
    }
    if (await handOver(item)) {
      await dropBrowserCopy(item.id);
    } else {
      try {
        await ext.downloads.resume(item.id);
      } catch {
        /* ignore */
      }
    }
  });
}

function setBadge(text) {
  ext.action.setBadgeBackgroundColor({ color: "#14b8a6" });
  ext.action.setBadgeText({ text });
  setTimeout(() => ext.action.setBadgeText({ text: "" }), 2500);
}

function setup() {
  ext.contextMenus.removeAll(() => {
    ext.contextMenus.create({
      id: "stewardget-link",
      title: "Download with StewardGet",
      contexts: ["link", "image", "video", "audio"],
    });
    ext.contextMenus.create({
      id: "stewardget-all",
      title: "Download all links with StewardGet…",
      contexts: ["page"],
    });
  });
  // A ping every minute lets StewardGet show "Connected" in its settings.
  ext.alarms.create("stewardget-ping", { periodInMinutes: 1 });
  ping();
}
ext.runtime.onInstalled.addListener(setup);
ext.runtime.onStartup?.addListener(setup);
ext.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === "stewardget-ping") ping();
});

ext.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "stewardget-all") {
    const sent = await sendAllLinks(tab);
    setBadge(sent ? "✓" : sent === 0 ? "0" : "!");
    return;
  }
  const url = info.linkUrl || info.srcUrl;
  if (!url) return;
  const result = await sendToStewardGet({ url, referrer: info.pageUrl || tab?.url, storeId: tab?.cookieStoreId });
  setBadge(result ? "✓" : "!");
  note(Boolean(result), result ? `Sent ${url.split("/").pop() || "a link"} to StewardGet` : lastFailure);
});

/** Asks StewardGet to bring its window forward. Resolves to whether it did. */
async function showStewardGet() {
  const s = await settings();
  try {
    const r = await fetch(apiUrl(s, "show"), {
      method: "POST",
      headers: headers(s),
      body: "{}",
      signal: AbortSignal.timeout(2000),
    });
    return r.ok && (await r.json()).shown === true;
  } catch {
    return false;
  }
}

// Messages from the popup and from content.js.
ext.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg?.type === "bypass" && typeof msg.url === "string") {
    bypass.set(stripHash(msg.url), Date.now() + BYPASS_MS);
    return false;
  }
  if (msg?.type === "show") {
    showStewardGet().then((shown) => reply({ shown }));
    return true;
  }
  if (msg?.type === "ping") {
    ping().then(reply);
    return true;
  }
  if (msg?.type === "media") {
    ext.tabs.query({ active: true, currentWindow: true }).then(async ([tab]) => {
      const list = [...((await mediaOf(tab?.id))?.values() ?? [])].map((m) => ({ ...m, name: mediaName(m, tab) }));
      // Streams first (usually what you want), then the largest files.
      list.sort((a, b) => (a.kind === b.kind ? (b.size || 0) - (a.size || 0) : a.kind === "hls" ? -1 : 1));
      reply({ media: list });
    });
    return true;
  }
  if (msg?.type === "grab-media" && typeof msg.url === "string") {
    ext.tabs.query({ active: true, currentWindow: true }).then(async ([tab]) => {
      const entry = (await mediaOf(tab?.id))?.get(msg.url) ?? { url: msg.url, kind: "file", type: "" };
      const result = await sendToStewardGet({
        url: msg.url,
        fileName: mediaName(entry, tab),
        referrer: tab?.url,
        storeId: tab?.cookieStoreId,
      });
      note(Boolean(result), result ? `Sent ${mediaName(entry, tab)} to StewardGet` : lastFailure);
      reply({ ok: Boolean(result) });
    });
    return true;
  }
  if (msg?.type === "all-links") {
    ext.tabs
      .query({ active: true, currentWindow: true })
      .then(([tab]) => sendAllLinks(tab))
      .then((sent) => reply({ sent }));
    return true;
  }
  return false;
});
