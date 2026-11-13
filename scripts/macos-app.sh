#!/usr/bin/env bash
# macos-app.sh BINARY DEST_DIR  ->  DEST_DIR/Kantor.app (ad-hoc signed)
# Bundle id com.dzulfikar.kantor, icon, and the document types Kantor opens.
# Used by package-macos.sh (tarball) and build-dmg.sh (DMG, Developer ID).
# Needs rsvg-convert (brew install librsvg) and iconutil (Xcode).
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

doc_type() { # name role exts...
  local name="$1" role="$2"; shift 2
  printf '    <dict>\n      <key>CFBundleTypeName</key><string>%s</string>\n      <key>CFBundleTypeRole</key><string>%s</string>\n      <key>LSHandlerRank</key><string>Alternate</string>\n      <key>CFBundleTypeExtensions</key><array>' "$name" "$role"
  for e in "$@"; do printf '<string>%s</string>' "$e"; done
  printf '</array>\n    </dict>\n'
}

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Kantor</string>
  <key>CFBundleDisplayName</key><string>Kantor</string>
  <key>CFBundleIdentifier</key><string>com.dzulfikar.kantor</string>
  <key>CFBundleExecutable</key><string>kantor</string>
  <key>CFBundleIconFile</key><string>kantor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>AGPL-3.0-or-later</string>
  <key>CFBundleDocumentTypes</key>
  <array>
$(doc_type "Spreadsheet" Editor xlsx csv tsv)
$(doc_type "Spreadsheet (import)" Viewer xls xlsb ods)
$(doc_type "Document" Editor docx)
$(doc_type "Document (import)" Viewer doc dot)
$(doc_type "Presentation" Editor pptx)
  </array>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Ad-hoc, so the bundle launches as built; build-dmg.sh re-signs it with a
# Developer ID when one is available.
codesign --force --sign - "$app"
echo "==> $app ($(lipo -archs "$app/Contents/MacOS/kantor"))"
