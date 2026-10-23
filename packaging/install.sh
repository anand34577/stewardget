#!/bin/sh
# StewardGet installer for Linux and macOS.
#
#   curl -fsSL https://github.com/anand34577/stewardget/releases/latest/download/install.sh | sh
#
# Options (pass them with `sh -s -- <options>` when piping):
#   --uninstall     remove StewardGet (your settings and downloads stay)
#   --purge         with --uninstall: also delete settings, download list and statistics
#   --no-launch     don't open StewardGet after installing
#   --version X.Y.Z install that release instead of the latest
#
# Linux: installs into ~/.local (or /usr/local when run as root; override with
# STEWARDGET_PREFIX). macOS: installs StewardGet.app into /Applications (or
# ~/Applications) and links the `stewardget` command into /usr/local/bin or ~/.local/bin.
#
# Every download is checked against the release's SHA256SUMS.txt. STEWARDGET_MIRROR
# points the installer at another copy of the release files.

set -eu

REPO="anand34577/stewardget"
APP_ID="dev.stewardget.StewardGet"

say() { printf '%s\n' "$*"; }
info() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

uninstall=0
purge=0
launch=1
version="${STEWARDGET_VERSION:-}"
while [ $# -gt 0 ]; do
  case $1 in
    --uninstall) uninstall=1 ;;
    --purge) purge=1 ;;
    --no-launch) launch=0 ;;
    --version)
      [ $# -ge 2 ] || die "--version needs a value, e.g. --version 0.2.0"
      version=$2
      shift
      ;;
    --version=*) version=${1#--version=} ;;
    -h | --help)
      say "Usage: install.sh [--uninstall [--purge]] [--no-launch] [--version X.Y.Z]"
      exit 0
      ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
  shift
done
version=${version#v}

os=$(uname -s)
case $os in
  Linux) platform=linux ;;
  Darwin) platform=macos ;;
  *) die "this installer is for Linux and macOS. On Windows, run in PowerShell:
  irm https://github.com/$REPO/releases/latest/download/install.ps1 | iex" ;;
esac

case $(uname -m) in
  x86_64 | amd64) arch=x64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) die "unsupported processor: $(uname -m). StewardGet has builds for x86-64 and ARM64." ;;
esac

if [ -n "${STEWARDGET_MIRROR:-}" ]; then
  base=${STEWARDGET_MIRROR%/}
elif [ -n "$version" ]; then
  base="https://github.com/$REPO/releases/download/v$version"
else
  base="https://github.com/$REPO/releases/latest/download"
fi

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t stewardget)
cleanup() {
  if [ -n "${mounted:-}" ]; then hdiutil detach -quiet "$mounted" 2>/dev/null || true; fi
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

fetch() {
  if command -v curl > /dev/null 2>&1; then
    if [ -n "${STEWARDGET_MIRROR:-}" ]; then
      curl -fsSL --retry 3 -o "$2" "$1"
    else
      # HTTPS only, also after redirects.
      curl -fsSL --retry 3 --proto '=https' --tlsv1.2 -o "$2" "$1"
    fi
  elif command -v wget > /dev/null 2>&1; then
    wget -q --tries=3 -O "$2" "$1"
  else
    die "curl or wget is needed to download StewardGet"
  fi
}

sha256() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum > /dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "sha256sum or shasum is needed to verify the download"
  fi
}

# Downloads a release file into $tmp and checks it against SHA256SUMS.txt.
download() {
  file=$1
  if [ ! -f "$tmp/SHA256SUMS.txt" ]; then
    fetch "$base/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt" ||
      die "could not reach GitHub. Check your connection${version:+, and that version $version exists}."
  fi
  info "Downloading $file"
  fetch "$base/$file" "$tmp/$file" || die "could not download $base/$file"
  expected=$(awk -v f="$file" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS.txt")
  [ -n "$expected" ] || die "$file is not listed in SHA256SUMS.txt"
  actual=$(sha256 "$tmp/$file")
  [ "$expected" = "$actual" ] || die "checksum mismatch for $file; the download is damaged or was altered. Nothing was installed."
}

on_path() {
  case ":$PATH:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
  esac
}

path_hint() {
  on_path "$1" && return 0
  say ""
  say "To run \`stewardget\` from a terminal, add $1 to your PATH:"
  case ${SHELL:-} in
    */fish) say "  fish_add_path $1" ;;
    */zsh) say "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.zshrc" ;;
    *) say "  echo 'export PATH=\"$1:\$PATH\"' >> ~/.bashrc" ;;
  esac
  say "then open a new terminal."
}

# ---- Velox -------------------------------------------------------------------
# StewardGet was called Velox before 0.7. A running Velox is closed (it saves its
# progress first) and its program files go; StewardGet takes over its data and
# login item the first time it starts.

quit_velox() {
  command -v pgrep > /dev/null 2>&1 && pgrep -x velox-gui > /dev/null 2>&1 || return 0
  info "Closing Velox (it saves its progress first)"
  if [ -x "$1" ]; then "$1" --quit > /dev/null 2>&1 || true; fi
  i=0
  while pgrep -x velox-gui > /dev/null 2>&1 && [ $i -lt 20 ]; do
    sleep 0.5
    i=$((i + 1))
    if [ $i -eq 10 ]; then pkill -TERM -x velox-gui 2>/dev/null || true; fi
  done
  return 0
}

# ---- Linux ------------------------------------------------------------------

linux_prefix() {
  if [ -n "${STEWARDGET_PREFIX:-}" ]; then
    printf '%s' "$STEWARDGET_PREFIX"
  elif [ "$(id -u)" = 0 ]; then
    printf '/usr/local'
  else
    printf '%s/.local' "$HOME"
  fi
}

linux_data_dir() {
  printf '%s/stewardget' "${XDG_DATA_HOME:-$HOME/.local/share}"
}

refresh_desktop() {
  if command -v update-desktop-database > /dev/null 2>&1; then
    update-desktop-database -q "$1/share/applications" 2>/dev/null || true
  fi
  if command -v gtk-update-icon-cache > /dev/null 2>&1; then
    gtk-update-icon-cache -q -t "$1/share/icons/hicolor" 2>/dev/null || true
  fi
}

linux_uninstall() {
  prefix=$(linux_prefix)
  info "Removing StewardGet from $prefix"
  rm -f "$prefix/bin/stewardget" "$prefix/bin/stewardget-gui" \
    "$prefix/share/applications/$APP_ID.desktop" \
    "$prefix/share/metainfo/$APP_ID.metainfo.xml"
  for size in 512 128 48 32 16; do
    rm -f "$prefix/share/icons/hicolor/${size}x$size/apps/$APP_ID.png"
  done
  rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/$APP_ID.desktop"
  refresh_desktop "$prefix"
  if [ "$purge" = 1 ]; then
    rm -rf "$(linux_data_dir)"
    say "Settings, download list and statistics deleted."
  fi
  say "StewardGet is removed. Your downloaded files were not touched."
}

linux_install() {
  if ldd --version 2>&1 | grep -qi musl; then
    die "this Linux uses musl (e.g. Alpine). StewardGet's builds need glibc; build from source instead: https://github.com/$REPO#build-from-source"
  fi
  if command -v dpkg-query > /dev/null 2>&1 && dpkg-query -W -f='${Status}' stewardget 2>/dev/null | grep -q "ok installed"; then
    warn "the stewardget .deb package is installed too. Remove it (sudo apt remove stewardget) to avoid two copies."
  fi
  if command -v dpkg-query > /dev/null 2>&1 && dpkg-query -W -f='${Status}' velox 2>/dev/null | grep -q "ok installed"; then
    warn "the velox .deb package (StewardGet's earlier name) is installed. Remove it with: sudo apt remove velox"
  fi

  file="stewardget-linux-$arch.tar.gz"
  download "$file"
  tar -xzf "$tmp/$file" -C "$tmp"
  src=$(find "$tmp" -maxdepth 1 -type d -name 'stewardget-*-linux-*' | head -n 1)
  [ -n "$src" ] || die "the archive has an unexpected layout"

  if [ -f "$src/GLIBC_MIN" ] && ldd --version > /dev/null 2>&1; then
    have=$(ldd --version 2>&1 | head -n 1 | grep -o '[0-9]\+\.[0-9]\+' | tail -n 1)
    need=$(cat "$src/GLIBC_MIN")
    if [ -n "$have" ] && [ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | head -n 1)" != "$need" ]; then
      die "StewardGet needs glibc $need or newer, this system has $have (e.g. Ubuntu 22.04+, Debian 12+, Fedora 36+)."
    fi
  fi

  prefix=$(linux_prefix)
  info "Installing into $prefix"
  mkdir -p "$prefix/bin" || die "can't write to $prefix (set STEWARDGET_PREFIX, or run with sudo for /usr/local)"
  if [ -e "$prefix/bin/velox-gui" ]; then
    quit_velox "$prefix/bin/velox-gui"
    info "Replacing Velox, StewardGet's earlier name"
    rm -f "$prefix/bin/velox" "$prefix/bin/velox-gui" \
      "$prefix/share/applications/dev.velox.Velox.desktop" \
      "$prefix/share/metainfo/dev.velox.Velox.metainfo.xml"
    for size in 512 128 48 32 16; do
      rm -f "$prefix/share/icons/hicolor/${size}x$size/apps/dev.velox.Velox.png"
    done
  fi
  # Copy then rename, so a running StewardGet keeps working until it restarts.
  for b in stewardget stewardget-gui; do
    cp "$src/bin/$b" "$prefix/bin/.$b.new"
    chmod 755 "$prefix/bin/.$b.new"
    mv -f "$prefix/bin/.$b.new" "$prefix/bin/$b"
  done
  mkdir -p "$prefix/share/applications" "$prefix/share/metainfo"
  # Absolute path: ~/.local/bin is often not on the PATH desktop sessions use.
  sed "s|^Exec=stewardget-gui|Exec=\"$prefix/bin/stewardget-gui\"|" "$src/share/applications/$APP_ID.desktop" \
    > "$prefix/share/applications/$APP_ID.desktop"
  cp "$src/share/metainfo/$APP_ID.metainfo.xml" "$prefix/share/metainfo/"
  for size in 512 128 48 32 16; do
    mkdir -p "$prefix/share/icons/hicolor/${size}x$size/apps"
    cp "$src/share/icons/hicolor/${size}x$size/apps/$APP_ID.png" "$prefix/share/icons/hicolor/${size}x$size/apps/"
  done
  refresh_desktop "$prefix"

  installed=$("$prefix/bin/stewardget" --version 2>/dev/null) ||
    die "StewardGet was copied to $prefix/bin but does not run on this system. Please report it: https://github.com/$REPO/issues"
  say ""
  info "$installed is installed"
  say "  Desktop app: StewardGet in your applications menu (or run stewardget-gui)"
  say "  Terminal:    stewardget"
  say "  Uninstall:   curl -fsSL https://github.com/$REPO/releases/latest/download/install.sh | sh -s -- --uninstall"
  path_hint "$prefix/bin"

  if [ "$launch" = 1 ] && { [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; }; then
    if command -v pgrep > /dev/null 2>&1 && pgrep -x stewardget-gui > /dev/null 2>&1; then
      say ""
      say "StewardGet is running. Quit and reopen it to use the new version."
    else
      nohup "$prefix/bin/stewardget-gui" > /dev/null 2>&1 &
    fi
  fi
}

# ---- macOS ------------------------------------------------------------------

mac_bin_dir() {
  if [ -w /usr/local/bin ]; then
    printf '/usr/local/bin'
  else
    printf '%s/.local/bin' "$HOME"
  fi
}

mac_quit() {
  if pgrep -x stewardget-gui > /dev/null 2>&1; then
    info "Closing StewardGet (it saves its progress first)"
    # `--quit` works even when the close button only minimises StewardGet.
    app_bin="/Applications/StewardGet.app/Contents/MacOS/stewardget-gui"
    [ -x "$app_bin" ] || app_bin="$HOME/Applications/StewardGet.app/Contents/MacOS/stewardget-gui"
    if [ -x "$app_bin" ]; then
      "$app_bin" --quit > /dev/null 2>&1 || true
    fi
    if pgrep -x stewardget-gui > /dev/null 2>&1; then
      osascript -e 'tell application id "dev.stewardget.StewardGet" to quit' > /dev/null 2>&1 || true
    fi
    i=0
    while pgrep -x stewardget-gui > /dev/null 2>&1 && [ $i -lt 20 ]; do
      sleep 0.5
      i=$((i + 1))
      # Progress is saved every second, so a plain TERM after 5 s loses nothing.
      if [ $i -eq 10 ]; then pkill -TERM -x stewardget-gui 2>/dev/null || true; fi
    done
    pgrep -x stewardget-gui > /dev/null 2>&1 && die "StewardGet is still running. Quit it, then run the installer again."
  fi
  return 0
}

mac_uninstall() {
  mac_quit
  for dir in /Applications "$HOME/Applications"; do
    if [ -d "$dir/StewardGet.app" ]; then
      info "Removing $dir/StewardGet.app"
      rm -rf "$dir/StewardGet.app"
    fi
  done
  for dir in /usr/local/bin "$HOME/.local/bin"; do
    if [ -L "$dir/stewardget" ]; then rm -f "$dir/stewardget"; fi
  done
  rm -f "$HOME/Library/LaunchAgents/$APP_ID.plist"
  if [ "$purge" = 1 ]; then
    rm -rf "$HOME/Library/Application Support/$APP_ID"
    say "Settings, download list and statistics deleted."
  fi
  say "StewardGet is removed. Your downloaded files were not touched."
}

mac_install() {
  major=$(sw_vers -productVersion | cut -d. -f1)
  [ "$major" -ge 11 ] 2>/dev/null || die "StewardGet needs macOS 11 (Big Sur) or newer."

  file="StewardGet-macos.dmg"
  download "$file"
  mac_quit
  for dir in /Applications "$HOME/Applications"; do
    if [ -d "$dir/Velox.app" ]; then
      quit_velox "$dir/Velox.app/Contents/MacOS/velox-gui"
      info "Replacing Velox, StewardGet's earlier name"
      rm -rf "$dir/Velox.app"
    fi
  done
  for dir in /usr/local/bin "$HOME/.local/bin"; do
    if [ -L "$dir/velox" ]; then rm -f "$dir/velox"; fi
  done

  if [ -w /Applications ]; then apps=/Applications; else apps="$HOME/Applications"; fi
  mkdir -p "$apps"
  mounted="$tmp/mnt"
  mkdir -p "$mounted"
  hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mounted" "$tmp/$file" ||
    die "could not open the disk image"
  info "Installing StewardGet.app into $apps"
  rm -rf "$apps/StewardGet.app.new"
  ditto "$mounted/StewardGet.app" "$apps/StewardGet.app.new"
  hdiutil detach -quiet "$mounted" || true
  mounted=""
  rm -rf "$apps/StewardGet.app"
  mv "$apps/StewardGet.app.new" "$apps/StewardGet.app"
  # Downloaded with curl, so there is no quarantine flag; clear one just in case.
  xattr -dr com.apple.quarantine "$apps/StewardGet.app" 2>/dev/null || true

  bindir=$(mac_bin_dir)
  mkdir -p "$bindir"
  ln -sf "$apps/StewardGet.app/Contents/MacOS/stewardget" "$bindir/stewardget"

  installed=$("$apps/StewardGet.app/Contents/MacOS/stewardget" --version 2>/dev/null) ||
    die "StewardGet was installed but does not run. Please report it: https://github.com/$REPO/issues"
  say ""
  info "$installed is installed"
  say "  Desktop app: StewardGet in Applications and Launchpad"
  say "  Terminal:    stewardget"
  say "  Uninstall:   curl -fsSL https://github.com/$REPO/releases/latest/download/install.sh | sh -s -- --uninstall"
  path_hint "$bindir"
  if [ "$launch" = 1 ]; then open "$apps/StewardGet.app" || true; fi
}

if [ "$uninstall" = 1 ]; then
  if [ "$platform" = linux ]; then linux_uninstall; else mac_uninstall; fi
else
  if [ "$platform" = linux ]; then linux_install; else mac_install; fi
fi
