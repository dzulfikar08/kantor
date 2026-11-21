#!/usr/bin/env bash
# build-mas.sh: the Mac App Store build of Kantor (cargo feature `app-store`, App Sandbox).
# See docs/app-store.md. The Developer ID download is scripts/build-dmg.sh.
#
#   build-mas.sh bundle OUT_DIR [--binary PATH]
#       Build the universal binary (aarch64 + x86_64) with --features app-store and make
#       OUT_DIR/Kantor.app: Info.plist like the DMG's plus CFBundleVersion = $BUILD_NUMBER
#       (default: Unix time) and ITSAppUsesNonExemptEncryption, ad-hoc signed with
#       packaging/macos/Kantor.mas.entitlements so it starts sandboxed for a smoke test;
#       zipped as OUT_DIR/Kantor-mas-<version>.zip. Checks for private API first.
#       MAS_SINGLE_ARCH=1 accepts a one-architecture --binary (CI's quick check only:
#       App Store Connect wants both).
#
#   build-mas.sh package APP OUT_DIR [--upload]
#       Sign APP for the store and wrap it in OUT_DIR/Kantor-<version>.pkg, then validate it
#       with App Store Connect (--upload: upload it instead). Needs:
#         MAS_PROFILE      the "Mac App Store" provisioning profile for com.dzulfikar.kantor
#         an "Apple Distribution" and a "3rd Party Mac Developer Installer" (Mac Installer
#         Distribution) identity in the keychain (override: MAS_APP_IDENTITY,
#         MAS_INSTALLER_IDENTITY)
#         ASC_KEY_ID, ASC_ISSUER_ID  App Store Connect API key; altool reads the key from
#                          ~/.appstoreconnect/private_keys/AuthKey_<ASC_KEY_ID>.p8
#       No notarisation: Apple signs the store copy again.
#
#   build-mas.sh check-private BINARY
#       Fail when BINARY links private symbols (CGS…/SLS…) or names private Objective-C
#       selectors (they start with "_"), which App Review rejects.
#
# macOS only (Xcode, rsvg-convert, both Rust targets).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
team="F6Q6UFUK69"
bundle_id="com.dzulfikar.kantor"
entitlements="$root/packaging/macos/Kantor.mas.entitlements"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
usage="usage: $0 bundle OUT_DIR [--binary PATH] | package APP OUT_DIR [--upload] | check-private BINARY"

die() { echo "error: $*" >&2; exit 1; }
macos_only() { [ "$(uname -s)" = Darwin ] || die "build-mas.sh runs on macOS only"; }

# Private API App Review rejects: CoreGraphics' private window-server calls, and
# Objective-C selectors that start with an underscore (methods Kantor defines or calls).
# Selectors from Apple's own Swift/ObjC runtime glue are not in Kantor's binary.
check_private() {
  local bin="$1" nm hits sels
  [ -f "$bin" ] || die "check-private: no file $bin"
  nm="$(command -v llvm-nm || xcrun --find llvm-nm 2>/dev/null || xcrun --find nm)"
  hits="$("$nm" -u "$bin" | grep -E '_(CGS|SLS)[A-Z]' || true)"
  # __objc_methname holds every selector the binary names.
  sels="$(for a in $(lipo -archs "$bin"); do
      otool -arch "$a" -v -s __TEXT __objc_methname "$bin" 2>/dev/null
    done | grep -oE '(^|[[:space:]])_[A-Za-z][A-Za-z0-9_:]*' | sed 's/^[[:space:]]*//' | sort -u || true)"
  if [ -n "$hits" ] || [ -n "$sels" ]; then
    [ -z "$hits" ] || { echo "private symbols:" >&2; echo "$hits" >&2; }
    [ -z "$sels" ] || { echo "private selectors:" >&2; echo "$sels" >&2; }
    die "$bin uses private API (see vendor/gpui-pre-macos/KANTOR_PATCHES.md)"
  fi
  echo "==> no private symbols or selectors in $bin"
}

bundle() {
  local out="" bin=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --binary) bin="${2:?$usage}"; shift 2 ;;
      -*) die "$usage" ;;
      *) [ -z "$out" ] || die "$usage"; out="$1"; shift ;;
    esac
  done
  [ -n "$out" ] || die "$usage"
  macos_only
  mkdir -p "$out"
  out="$(cd "$out" && pwd)"
  if [ -z "$bin" ]; then
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
    local target_dir="${CARGO_TARGET_DIR:-$root/target}" slices=() target
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
      (cd "$root" && cargo build --release --locked -p kantor --features app-store --target "$target")
      slices+=("$target_dir/$target/release/kantor")
    done
    bin="$out/kantor-universal"
    lipo -create -output "$bin" "${slices[@]}"
  fi
  case "$(lipo -archs "$bin")" in
    *arm64*x86_64*|*x86_64*arm64*) ;;
    *) [ "${MAS_SINGLE_ARCH:-0}" = 1 ] || die "not a universal binary: $(lipo -archs "$bin")" ;;
  esac
  check_private "$bin"

  KANTOR_MAS=1 KANTOR_BUNDLE_VERSION="${BUILD_NUMBER:-$(date +%s)}" \
    "$root/scripts/macos-app.sh" "$bin" "$out"
  [ "$bin" != "$out/kantor-universal" ] || rm -f "$bin"
  local app="$out/Kantor.app"
  # Ad-hoc, with the sandbox entitlements: it starts sandboxed (container
  # ~/Library/Containers/com.dzulfikar.kantor) for a smoke test on this Mac.
  codesign --force --sign - --entitlements "$entitlements" "$app"
  codesign --display --entitlements - "$app" | grep -q com.apple.security.app-sandbox \
    || die "the sandbox entitlement didn't stick"
  local zip="$out/Kantor-mas-$version.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$app" "$zip"
  echo "==> $app (CFBundleVersion $(plutil -extract CFBundleVersion raw -o - "$app/Contents/Info.plist"), ad-hoc, sandboxed)"
  echo "==> $zip"
}

package() {
  local app="" out="" upload=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --upload) upload=1; shift ;;
      -*) die "$usage" ;;
      *) if [ -z "$app" ]; then app="$1"; elif [ -z "$out" ]; then out="$1"; else die "$usage"; fi; shift ;;
    esac
  done
  [ -n "$app" ] && [ -n "$out" ] || die "$usage"
  macos_only
  [ -d "$app/Contents/MacOS" ] || die "$app is not an app bundle"
  [ -f "${MAS_PROFILE:-}" ] || die "MAS_PROFILE must name the Mac App Store provisioning profile"
  for v in ASC_KEY_ID ASC_ISSUER_ID; do
    [ -n "${!v:-}" ] || die "$v is not set"
  done
  [ -f "$HOME/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID}.p8" ] \
    || die "no ~/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID}.p8 (altool reads the key there)"
  local app_id inst_id
  app_id="${MAS_APP_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/"Apple Distribution/ { print $2; exit }')}"
  inst_id="${MAS_INSTALLER_IDENTITY:-$(security find-identity -v | awk -F'"' '/"(3rd Party Mac Developer Installer|Mac Installer Distribution)/ { print $2; exit }')}"
  [ -n "$app_id" ] || die "no \"Apple Distribution\" identity in the keychain (or set MAS_APP_IDENTITY)"
  [ -n "$inst_id" ] || die "no \"3rd Party Mac Developer Installer\" identity in the keychain (or set MAS_INSTALLER_IDENTITY)"
  mkdir -p "$out"
  out="$(cd "$out" && pwd)"

  local work
  work="$(mktemp -d)"
  # shellcheck disable=SC2064 # expand now: $work is local
  trap "rm -rf '$work'" EXIT

  # 1. The profile's identifiers. codesign doesn't copy them from the profile, and
  #    without them App Store Connect refuses the upload (error 90886).
  security cms -D -i "$MAS_PROFILE" > "$work/profile.plist"
  local p_app p_team
  p_app="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.application-identifier' "$work/profile.plist")"
  p_team="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.developer.team-identifier' "$work/profile.plist")"
  [ "$p_app" = "$team.$bundle_id" ] || die "the profile is for $p_app, not $team.$bundle_id"
  [ "$p_team" = "$team" ] || die "the profile's team is $p_team, not $team"
  cp "$entitlements" "$work/entitlements.plist"
  /usr/libexec/PlistBuddy -c "Add :com.apple.application-identifier string $p_app" "$work/entitlements.plist"
  /usr/libexec/PlistBuddy -c "Add :com.apple.developer.team-identifier string $p_team" "$work/entitlements.plist"
  plutil -lint "$work/entitlements.plist" > /dev/null

  # 2. Embed the profile and sign: hardened runtime, secure timestamp, one executable.
  local signed="$work/Kantor.app"
  ditto "$app" "$signed"
  cp "$MAS_PROFILE" "$signed/Contents/embedded.provisionprofile"
  xattr -cr "$signed"
  codesign --force --options runtime --timestamp --entitlements "$work/entitlements.plist" \
    --sign "$app_id" "$signed"
  codesign --verify --strict --verbose=2 "$signed"
  codesign --display --entitlements - "$signed"
  check_private "$signed/Contents/MacOS/kantor"

  # 3. The installer package App Store Connect takes.
  local pkg="$out/Kantor-$version.pkg"
  rm -f "$pkg"
  productbuild --component "$signed" /Applications --sign "$inst_id" "$pkg"
  pkgutil --check-signature "$pkg"

  # 4. Validate (or upload). altool reads the API key from ~/.appstoreconnect/private_keys.
  local action=--validate-app
  [ "$upload" = 0 ] || action=--upload-app
  xcrun altool "$action" --type macos --file "$pkg" \
    --apiKey "$ASC_KEY_ID" --apiIssuer "$ASC_ISSUER_ID"
  echo "==> $pkg ($( [ "$upload" = 1 ] && echo uploaded || echo validated ))"
}

case "${1:-}" in
  bundle) shift; bundle "$@" ;;
  package) shift; package "$@" ;;
  check-private) shift; [ $# -eq 1 ] || die "$usage"; macos_only; check_private "$1" ;;
  -h|--help) sed -n '2,30p' "$0" ;;
  *) die "$usage" ;;
esac
