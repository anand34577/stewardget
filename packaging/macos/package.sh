#!/usr/bin/env bash
# Builds the macOS release files from universal binaries:
#   StewardGet-macos.dmg              StewardGet.app, drag to Applications
#   stewardget-macos-universal.tar.gz the two binaries, for the terminal
#
#   packaging/macos/package.sh <version> <dir with stewardget and stewardget-gui> <output dir>
#
# Runs on macOS (sips, iconutil, codesign, hdiutil). The app gets an ad-hoc
# signature, which Apple silicon requires; it is not notarised.
set -euo pipefail

version=$1
bin=$(cd "$2" && pwd)
mkdir -p "$3"
out=$(cd "$3" && pwd)
root=$(cd "$(dirname "$0")/../.." && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

app="$work/dmg/StewardGet.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/stewardget-gui" "$bin/stewardget" "$app/Contents/MacOS/"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

# Icon set from the 512 px logo (the 1024 px slot is upscaled).
iconset="$work/stewardget.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}.png" > /dev/null
  double=$((size * 2))
  sips -z $double $double "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/stewardget.icns"

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

ln -s /Applications "$work/dmg/Applications"
hdiutil create -quiet -volname "StewardGet" -srcfolder "$work/dmg" -ov -format UDZO "$out/StewardGet-macos.dmg"

name="stewardget-$version-macos-universal"
mkdir -p "$work/$name"
cp "$bin/stewardget" "$bin/stewardget-gui" "$root/README.md" "$root/CHANGELOG.md" "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$work/$name/"
tar -C "$work" -czf "$out/stewardget-macos-universal.tar.gz" "$name"

ls -l "$out"
