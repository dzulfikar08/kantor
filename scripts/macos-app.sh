#!/usr/bin/env bash
# macos-app.sh BINARY DEST_DIR  ->  DEST_DIR/Kantor.app (ad-hoc signed)
# Bundle id com.dzulfikar.kantor, icon, and the document types Kantor opens.
# Used by package-macos.sh (tarball) and build-dmg.sh (DMG, Developer ID).
# Needs rsvg-convert (brew install librsvg) and iconutil (Xcode).
# KANTOR_BUNDLE_VERSION sets CFBundleVersion and KANTOR_MAS=1 the App Store
# keys (build-mas.sh; see macos-info-plist.sh).
set -euo pipefail
bin="$1"; dest="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
app="$dest/Kantor.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m755 "$bin" "$app/Contents/MacOS/kantor"

# Icon: SVG -> iconset -> icns.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
iconset="$work/kantor.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  rsvg-convert -w "$s" -h "$s" "$root/assets/icons/kantor.svg" -o "$iconset/icon_${s}x${s}.png"
  rsvg-convert -w $((s * 2)) -h $((s * 2)) "$root/assets/icons/kantor.svg" -o "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/kantor.icns"
# Licences travel inside the bundle (signed with it).
tpl="$root/THIRD_PARTY_LICENSES.txt"; [ -f "$tpl" ] || python3 "$root/scripts/third-party-licenses.py" "$tpl"
install -m644 "$root/LICENSE" "$root/NOTICE" "$tpl" "$app/Contents/Resources/"

"$root/scripts/macos-info-plist.sh" "$version" "${KANTOR_BUNDLE_VERSION:-$version}" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Ad-hoc, so the bundle launches as built; build-dmg.sh re-signs it with a
# Developer ID when one is available.
codesign --force --sign - "$app"
echo "==> $app ($(lipo -archs "$app/Contents/MacOS/kantor"))"
