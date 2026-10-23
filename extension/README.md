# StewardGet browser extension

Hands downloads from your browser to the StewardGet app. Works in Chrome, Edge, Brave,
Opera, Vivaldi and Firefox (121+) from a single Manifest V3 codebase.
`manifest.json` is for Chromium browsers, `manifest.firefox.json` for Firefox
(Firefox runs background scripts instead of a service worker); keep the two in
step.

## What it does

- Takes over downloads started in the browser and sends them to StewardGet together
  with the page's cookies, the referrer and your user agent, so downloads that
  need a login keep working.
- Adds **Download with StewardGet** to the context menu of links, images, video and audio.
- Leaves the download to the browser when StewardGet isn't running, so nothing is lost.
- Leaves downloads from private (incognito) windows to the browser.
- **Alt+click** a link to keep that one download in the browser.
- Powers **Refresh link**: when StewardGet is waiting for a fresh link, the next
  matching download you start is attached to the partial file and resumes.

## Install

The easiest way is from the app: **Settings › Browser** shows the steps for
your browser, the folder to choose, and turns green once the extension connects.
From a terminal, `stewardget extension` prints the same steps.

StewardGet keeps a copy of this extension in its data folder and updates it when the
app updates, so choose that folder (not this one) when you install:
`browser-extension` for Chromium browsers, `browser-extension-firefox` for
Firefox.

- **Chrome, Edge, Brave, Opera, Vivaldi**: open the browser's extensions page
  (`chrome://extensions`, `edge://extensions`, …), turn on *Developer mode*,
  click *Load unpacked* and select the folder.
- **Firefox**: install `stewardget-firefox.xpi` from the release page when it has
  one. Otherwise open `about:debugging#/runtime/this-firefox`, click *Load
  Temporary Add-on…* and select `manifest.json` in the
  `browser-extension-firefox` folder (it lasts until Firefox restarts). Then allow access to all websites on the add-on's Permissions tab.

## Settings (toolbar popup)

| Setting | Meaning |
|---|---|
| Capture browser downloads | Master switch |
| Leave files smaller than | Small files stay in the browser |
| Never capture these extensions | e.g. `pdf, jpg` |
| Never capture on these sites | e.g. `mybank.com` (covers subdomains); the popup has a one-click button for the current site |
| Port / Token | Must match *Settings › Browser* in StewardGet |

## Privacy

The extension talks only to `http://127.0.0.1:<port>`, the StewardGet app on your own
machine. It collects nothing and sends nothing anywhere else. Cookies are read
only for the URL being downloaded, and only to pass them to StewardGet. A ping once a
minute tells the app that the extension is installed; it carries the browser's
name and the extension's version, nothing else.

## Status in the popup

The popup shows whether StewardGet answers, the extension and app versions, and
what happened to the last download it handled ("Sent file.zip to StewardGet", or why
StewardGet could not take it, in which case the browser keeps the download).
