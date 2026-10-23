#!/usr/bin/env bash
# Builds the Linux release files from compiled binaries:
#   stewardget-linux-<arch>.tar.gz   (used by install.sh, or unpack anywhere)
#   stewardget-linux-<arch>.deb      (Debian, Ubuntu, Mint, Pop!_OS, ...)
#   stewardget-linux-<arch>.rpm      (Fedora, openSUSE, RHEL, ...)
#
#   packaging/linux/package.sh <version> <x64|arm64> <dir with stewardget and stewardget-gui> <output dir>
#
# Needs dpkg-deb, rpmbuild (apt install rpm) and objdump.
set -euo pipefail

version=$1
arch=$2
bin=$(realpath "$3")
out=$(realpath -m "$4")
root=$(realpath "$(dirname "$0")/../..")

case $arch in
  x64) deb_arch=amd64; rpm_arch=x86_64 ;;
  arm64) deb_arch=arm64; rpm_arch=aarch64 ;;
  *) echo "unknown architecture: $arch" >&2; exit 1 ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# The newest glibc symbol the binaries need decides the minimum distribution.
glibc=$(objdump -T "$bin/stewardget" "$bin/stewardget-gui" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
echo "Needs glibc $glibc or newer"

# Shared file tree under a prefix: bin/, share/applications, share/icons, ...
stage() {
  local prefix=$1
  install -Dm755 "$bin/stewardget" "$prefix/bin/stewardget"
  install -Dm755 "$bin/stewardget-gui" "$prefix/bin/stewardget-gui"
  install -Dm644 "$root/packaging/linux/dev.stewardget.StewardGet.desktop" "$prefix/share/applications/dev.stewardget.StewardGet.desktop"
  sed -e "s/@VERSION@/$version/" -e "s/@DATE@/$(date -u +%F)/" "$root/packaging/linux/dev.stewardget.StewardGet.metainfo.xml" \
    > "$work/metainfo.xml"
  install -Dm644 "$work/metainfo.xml" "$prefix/share/metainfo/dev.stewardget.StewardGet.metainfo.xml"
  install -Dm644 "$root/assets/icon.png" "$prefix/share/icons/hicolor/512x512/apps/dev.stewardget.StewardGet.png"
  for size in 128 48 32 16; do
    install -Dm644 "$root/extension/icons/icon$size.png" "$prefix/share/icons/hicolor/${size}x$size/apps/dev.stewardget.StewardGet.png"
  done
}

copyright() {
  printf 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\nUpstream-Name: stewardget\nSource: https://github.com/anand34577/stewardget\n\nFiles: *\nCopyright: StewardGet contributors\nLicense: MIT or Apache-2.0\n\n'
  cat "$root/LICENSE-MIT"
  printf '\n\n'
  cat "$root/LICENSE-APACHE"
}

# ---- tar.gz ---------------------------------------------------------------
name="stewardget-$version-linux-$arch"
stage "$work/$name"
cp "$root/README.md" "$root/CHANGELOG.md" "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$work/$name/"
echo "$glibc" > "$work/$name/GLIBC_MIN"
tar -C "$work" -czf "$out/stewardget-linux-$arch.tar.gz" "$name"

# ---- deb ------------------------------------------------------------------
deb="$work/deb"
stage "$deb/usr"
install -d "$deb/usr/share/doc/stewardget"
copyright > "$deb/usr/share/doc/stewardget/copyright"
gzip -9n -c "$root/CHANGELOG.md" > "$deb/usr/share/doc/stewardget/changelog.gz"
install -d "$deb/DEBIAN"
cat > "$deb/DEBIAN/control" <<EOF
Package: stewardget
Version: $version
Section: net
Priority: optional
Architecture: $deb_arch
Maintainer: StewardGet contributors <76194471+anand34577@users.noreply.github.com>
Installed-Size: $(du -sk "$deb/usr" | cut -f1)
Depends: libc6 (>= $glibc), libgcc-s1
Conflicts: velox
Replaces: velox
Recommends: libgl1, libegl1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libx11-6, libxcursor1, libxrandr2, libxi6, xdg-utils, xdg-desktop-portal
Homepage: https://github.com/anand34577/stewardget
Description: Fast, modern download manager
 StewardGet splits each download across several connections, resumes after
 crashes and restarts, and never throws partial data away. It has a desktop
 app (stewardget-gui), a terminal dashboard and command line (stewardget), and a
 browser extension. No telemetry.
EOF
dpkg-deb --root-owner-group -Zxz --build "$deb" "$out/stewardget-linux-$arch.deb"

# ---- rpm ------------------------------------------------------------------
rpmroot="$work/rpm"
mkdir -p "$rpmroot"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,stage}
stage "$rpmroot/stage/usr"
cat > "$rpmroot/SPECS/stewardget.spec" <<EOF
%global debug_package %{nil}
%global __strip /bin/true
%define _build_id_links none

Name:           stewardget
Version:        $version
Release:        1
Summary:        Fast, modern download manager
License:        MIT OR Apache-2.0
URL:            https://github.com/anand34577/stewardget
AutoReqProv:    no
Requires:       glibc >= $glibc
Obsoletes:      velox < 0.7.0
Recommends:     xdg-utils

%description
StewardGet splits each download across several connections, resumes after
crashes and restarts, and never throws partial data away. It has a desktop
app (stewardget-gui), a terminal dashboard and command line (stewardget), and a
browser extension. No telemetry.

%install
cp -a $rpmroot/stage/. %{buildroot}/

%files
/usr/bin/stewardget
/usr/bin/stewardget-gui
/usr/share/applications/dev.stewardget.StewardGet.desktop
/usr/share/metainfo/dev.stewardget.StewardGet.metainfo.xml
/usr/share/icons/hicolor/*/apps/dev.stewardget.StewardGet.png
EOF
rpmbuild --quiet --define "_topdir $rpmroot" --target "$rpm_arch" -bb "$rpmroot/SPECS/stewardget.spec"
cp "$rpmroot"/RPMS/"$rpm_arch"/stewardget-*.rpm "$out/stewardget-linux-$arch.rpm"

ls -l "$out"
