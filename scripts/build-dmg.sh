#!/usr/bin/env bash
# build-dmg.sh [options] OUT_DIR  ->  OUT_DIR/Kantor.app and OUT_DIR/<name>.dmg
#
#   --binary PATH   use this kantor binary instead of building the universal one
#   --name NAME     DMG file name without .dmg (default Kantor-<version>)
#   --sign          sign with the "Developer ID Application" identity (hardened runtime,
#                   packaging/macos/Kantor.entitlements, secure timestamp); fails if missing
#   --notarize      also notarise the DMG with the App Store Connect API key, staple the
#                   ticket to the DMG and the app, and check both with spctl (implies --sign)
#
# Without --sign the app stays ad-hoc signed: it runs where it was built, but a downloaded
# copy is blocked by Gatekeeper until the quarantine flag is removed. Nightly and PR
# builds use that; release.yml uses --notarize.
#
# Environment:
#   KANTOR_SIGN_IDENTITY   codesign identity (name or SHA-1); default: the first valid
#                          "Developer ID Application" identity in the keychain search list
#   ASC_KEY_PATH, ASC_KEY_ID, ASC_ISSUER_ID   App Store Connect API key, for --notarize
#                          (scripts/ci-keychain.sh setup writes ASC_KEY_PATH in CI)
#
# Without --binary the universal binary is built with
#   cargo build --release --locked -p kantor --target {aarch64,x86_64}-apple-darwin
# and joined with lipo. Needs Xcode, rsvg-convert (brew install librsvg) and both Rust targets.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
usage="usage: $0 [--binary PATH] [--name NAME] [--sign] [--notarize] OUT_DIR"
bin="" name="" sign=0 notarize=0 out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --binary) bin="${2:?$usage}"; shift 2 ;;
    --name) name="${2:?$usage}"; shift 2 ;;
    --sign) sign=1; shift ;;
    --notarize) sign=1; notarize=1; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    -*) echo "$usage" >&2; exit 2 ;;
    *) [ -z "$out" ] || { echo "$usage" >&2; exit 2; }; out="$1"; shift ;;
  esac
done
[ -n "$out" ] || { echo "$usage" >&2; exit 2; }
[ "$(uname -s)" = Darwin ] || { echo "build-dmg.sh runs on macOS only" >&2; exit 1; }

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
name="${name:-Kantor-$version}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
dmg="$out/$name.dmg"
entitlements="$root/packaging/macos/Kantor.entitlements"

# Fail early, before a long build, when signing or notarising can't work.
identity=""
if [ "$sign" = 1 ]; then
  identity="${KANTOR_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk '/"Developer ID Application/ { print $2; exit }')}"
  [ -n "$identity" ] || { echo "error: --sign: no valid \"Developer ID Application\" identity in the keychain (set KANTOR_SIGN_IDENTITY or import one)" >&2; exit 1; }
fi
if [ "$notarize" = 1 ]; then
  for v in ASC_KEY_PATH ASC_KEY_ID ASC_ISSUER_ID; do
    [ -n "${!v:-}" ] || { echo "error: --notarize needs $v" >&2; exit 1; }
  done
  [ -f "$ASC_KEY_PATH" ] || { echo "error: ASC_KEY_PATH is not a file" >&2; exit 1; }
fi

# 1. Binary: universal unless one was given.
if [ -z "$bin" ]; then
  # Rust's x86_64 default is 10.12; match Info.plist's LSMinimumSystemVersion for both slices.
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
  slices=()
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd "$root" && cargo build --release --locked -p kantor --target "$target")
    slices+=("$root/target/$target/release/kantor")
  done
  bin="$out/kantor-universal"
  lipo -create -output "$bin" "${slices[@]}"
  archs="$(lipo -archs "$bin")"
  case "$archs" in *arm64*x86_64*|*x86_64*arm64*) ;; *) echo "error: not a universal binary: $archs" >&2; exit 1 ;; esac
fi

# 2. Kantor.app (ad-hoc signed by macos-app.sh).
"$root/scripts/macos-app.sh" "$bin" "$out"
[ "$bin" != "$out/kantor-universal" ] || rm -f "$bin"
app="$out/Kantor.app"

# 3. Developer ID signature: hardened runtime (required for notarisation), entitlements,
#    secure timestamp. One executable and no nested code, so no --deep.
if [ "$sign" = 1 ]; then
  xattr -cr "$app"
  codesign --force --options runtime --timestamp --entitlements "$entitlements" \
    --sign "$identity" "$app"
  codesign --verify --strict --verbose=2 "$app"
  codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Authority|TeamIdentifier|Identifier|Runtime Version|Timestamp)=' || true
else
  echo "::warning::Kantor.app is ad-hoc signed (no Developer ID): Gatekeeper blocks a downloaded copy until the quarantine flag is removed." >&2
fi

# 4. DMG: the app and a link to /Applications, so it is dragged across.
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
ditto "$app" "$staging/Kantor.app"
ln -s /Applications "$staging/Applications"
rm -f "$dmg"
# hdiutil guesses the size from -srcfolder and can guess too small ("No space left on
# device"): give it the folder size plus 20% and 20 MB. On CI runners it sometimes fails
# with "Resource busy"; retry.
size_mb=$(( $(du -sm "$staging" | cut -f1) * 12 / 10 + 20 ))
for attempt in 1 2 3 4 5; do
  if hdiutil create -volname Kantor -srcfolder "$staging" -fs HFS+ -size "${size_mb}m" \
      -format ULMO -ov "$dmg"; then
    break
  fi
  [ "$attempt" -lt 5 ] || { echo "error: hdiutil create failed" >&2; exit 1; }
  echo "hdiutil create failed (attempt $attempt), retrying" >&2
  sleep $((attempt * 5))
done
hdiutil verify "$dmg"

if [ "$sign" = 1 ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
  codesign --verify --strict --verbose=2 "$dmg"
fi

# 5. Notarise, staple, assess.
if [ "$notarize" = 1 ]; then
  result="$(mktemp)"
  # notarytool exits 0 for a finished but rejected submission, so read the status.
  xcrun notarytool submit "$dmg" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" \
    --issuer "$ASC_ISSUER_ID" --wait --timeout 1h --output-format json > "$result" || true
  cat "$result"; echo
  status="$(plutil -extract status raw -o - "$result" 2>/dev/null || true)"
  sub_id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
  if [ "$status" != Accepted ]; then
    echo "error: notarisation status '${status:-unknown}'" >&2
    if [ -n "$sub_id" ]; then
      xcrun notarytool log "$sub_id" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" \
        --issuer "$ASC_ISSUER_ID" >&2 || true
    fi
    exit 1
  fi
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
  # The DMG's ticket covers the app inside it too: staple it to Kantor.app as well, for the
  # tarball install.sh unpacks (checked online by Gatekeeper anyway if this fails).
  if xcrun stapler staple "$app"; then
    spctl --assess --type execute --verbose=2 "$app"
  else
    echo "::warning::could not staple the ticket to Kantor.app; Gatekeeper will check it online" >&2
  fi
fi

shasum -a 256 "$dmg"
echo "==> $dmg ($(if [ "$notarize" = 1 ]; then echo "Developer ID, notarised"; elif [ "$sign" = 1 ]; then echo "Developer ID, not notarised"; else echo "ad-hoc signed, not notarised"; fi))"
