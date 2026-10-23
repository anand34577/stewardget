const ext = globalThis.browser ?? globalThis.chrome;
const DEFAULTS = { enabled: true, port: 7853, token: "", minSizeKb: 0, skipExtensions: "", skipSites: "" };
const ALL_SITES = { origins: ["<all_urls>"] };
const $ = (id) => document.getElementById(id);

async function load() {
  const s = { ...DEFAULTS, ...(await ext.storage.local.get(DEFAULTS)) };
  $("enabled").checked = s.enabled;
  $("port").value = s.port;
  $("token").value = s.token;
  $("minSizeKb").value = s.minSizeKb;
  $("skipExtensions").value = s.skipExtensions;
  $("skipSites").value = s.skipSites;
  $("version").textContent = `v${ext.runtime.getManifest().version}`;
  await showSiteButton();
  await showMedia();
  await checkAccess();
  refreshStatus();
}

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

/** Videos, audio and streams the current tab loaded, each with a Download button. */
async function showMedia() {
  const r = await ext.runtime.sendMessage({ type: "media" }).catch(() => null);
  const list = r?.media ?? [];
  $("media").hidden = list.length === 0;
  const ul = $("mediaList");
  ul.replaceChildren();
  for (const m of list) {
    const li = document.createElement("li");
    const info = document.createElement("div");
    info.className = "info";
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = m.name;
    name.title = m.url;
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = m.kind === "hls" ? "Stream (HLS)" : [m.type, sizeText(m.size)].filter(Boolean).join(" · ");
    info.append(name, meta);
    const button = document.createElement("button");
    button.textContent = "Download";
    button.addEventListener("click", async () => {
      button.disabled = true;
      const res = await ext.runtime.sendMessage({ type: "grab-media", url: m.url }).catch(() => null);
      button.textContent = res?.ok ? "Sent ✓" : "Failed";
      if (!res?.ok) button.disabled = false;
      showLast();
    });
    li.append(info, button);
    ul.append(li);
  }
}

/** Firefox asks the user to allow site access separately; without it nothing reaches StewardGet. */
async function checkAccess() {
  let granted = true;
  try {
    granted = await ext.permissions.contains(ALL_SITES);
  } catch {
    /* not supported: access was granted at install */
  }
  $("access").hidden = granted;
}

async function refreshStatus() {
  let r = null;
  try {
    r = await ext.runtime.sendMessage({ type: "ping" });
  } catch {
    /* the background script isn't running; treated like "not running" */
  }
  const mine = ext.runtime.getManifest().version;
  let text = "StewardGet is not running";
  let hint = "Start StewardGet on this computer. Until then, downloads stay in the browser.";
  if (r?.ok) {
    text = r.version ? `Connected to StewardGet ${r.version}` : "Connected";
    hint =
      r.version && r.version !== mine
        ? `This extension is version ${mine}. Reload it on the browser's extensions page to update it.`
        : "";
  } else if (r?.status === 401) {
    text = "Wrong token";
    hint = "The token must match Settings › Browser in StewardGet.";
  } else if (r?.status) {
    text = "StewardGet refused the connection";
    hint = `Check the port in Settings › Browser in StewardGet (answer: ${r.status}).`;
  }
  $("open").hidden = !r?.ok;
  $("dot").className = `dot ${r?.ok ? "on" : "off"}`;
  $("state").textContent = r?.ok ? "connected" : "not connected";
  $("message").textContent = text;
  $("hint").textContent = hint;
  $("hint").hidden = !hint;
  await showLast();
}

/** What happened to the latest download the extension handled. */
async function showLast() {
  const { lastCapture } = await ext.storage.local.get("lastCapture");
  const el = $("last");
  el.hidden = !lastCapture;
  if (!lastCapture) return;
  const mins = Math.max(0, Math.round((Date.now() - lastCapture.at) / 60000));
  const when = mins < 1 ? "just now" : mins < 60 ? `${mins} min ago` : `${Math.round(mins / 60)} h ago`;
  el.textContent = `Last: ${lastCapture.text} (${when})`;
  el.style.color = lastCapture.ok ? "" : "var(--red)";
}

async function save() {
  await ext.storage.local.set({
    enabled: $("enabled").checked,
    port: Math.min(65535, Math.max(1024, Number($("port").value) || DEFAULTS.port)),
    token: $("token").value.trim(),
    minSizeKb: Math.max(0, Number($("minSizeKb").value) || 0),
    skipExtensions: $("skipExtensions").value,
    skipSites: $("skipSites").value,
  });
  $("save").textContent = "Saved";
  setTimeout(() => ($("save").textContent = "Save"), 1200);
  refreshStatus();
}

/** Host of the active tab, when it is a web page. */
async function currentSite() {
  try {
    const [tab] = await ext.tabs.query({ active: true, currentWindow: true });
    const url = new URL(tab?.url ?? "");
    return url.protocol.startsWith("http") ? url.hostname.replace(/^www\./, "") : "";
  } catch {
    return "";
  }
}

function sites() {
  return $("skipSites")
    .value.split(/[\s,;]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/** "Don't capture on <site>" / "Capture on <site> again", for the active tab. */
async function showSiteButton() {
  const site = await currentSite();
  const button = $("site");
  button.hidden = !site;
  if (!site) return;
  const listed = sites().some((s) => s.toLowerCase() === site.toLowerCase());
  button.textContent = listed ? `Capture downloads on ${site} again` : `Don't capture downloads on ${site}`;
  button.onclick = async () => {
    const rest = sites().filter((s) => s.toLowerCase() !== site.toLowerCase());
    $("skipSites").value = (listed ? rest : [...rest, site]).join(", ");
    await save();
    await showSiteButton();
  };
}

$("save").addEventListener("click", save);
$("open").addEventListener("click", async () => {
  const r = await ext.runtime.sendMessage({ type: "show" }).catch(() => null);
  if (r?.shown) window.close();
});
$("all").addEventListener("click", async () => {
  $("all").disabled = true;
  const r = await ext.runtime.sendMessage({ type: "all-links" }).catch(() => null);
  const out = $("allResult");
  out.hidden = false;
  if (r?.sent) {
    out.textContent = `Sent ${r.sent} links. Pick the ones you want in StewardGet.`;
  } else if (r?.sent === 0) {
    out.textContent = "This page has no links.";
  } else {
    out.textContent = "Couldn't read this page or reach StewardGet. Browser pages and the web store can't be read.";
  }
  $("all").disabled = false;
});
$("enabled").addEventListener("change", save);
$("grant").addEventListener("click", async () => {
  try {
    await ext.permissions.request(ALL_SITES);
  } catch {
    /* the user closed the prompt */
  }
  await checkAccess();
  refreshStatus();
});
load();
