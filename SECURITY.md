# Security policy

## Supported versions

Security fixes go into the latest release.

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Report them privately through
GitHub: open the repository's **Security** tab and choose **Report a vulnerability**.

Include what you found, how to reproduce it, and which version and operating system you used.
You can expect an acknowledgement within a few days. A fix is released as soon as it is ready,
and you are credited in the release notes unless you prefer otherwise.

## Scope

Areas that matter most:

- The local browser-integration API on `127.0.0.1`. It must refuse requests from web pages
  (including through DNS rebinding), and it must enforce the access token when one is configured.
- The installers and install scripts: they must verify what they download and must not change
  anything they don't undo on uninstall.
- Handling of untrusted input from servers: file names, headers, and redirects.
- Anything that could write outside the chosen download folder.

StewardGet has no telemetry and makes no network requests besides the downloads you start. If you
find any other traffic, please report it.
