#!/usr/bin/env bash
# CI only (macOS): import the Developer ID Application identity into a temporary keychain and
# write the App Store Connect API key to disk, for scripts/build-dmg.sh --notarize.
#
#   scripts/ci-keychain.sh setup       # appends ASC_KEY_PATH to $GITHUB_ENV
#   scripts/ci-keychain.sh setup-mas   # Mac App Store: appends ASC_KEY_PATH and MAS_PROFILE
#   scripts/ci-keychain.sh cleanup     # run in an `if: always()` step
#
# setup reads DEVELOPER_ID_P12_BASE64, DEVELOPER_ID_P12_PASSWORD, ASC_KEY_ID and
# ASC_KEY_BASE64; setup-mas reads APPLE_DISTRIBUTION_P12_BASE64/_PASSWORD,
# MAC_INSTALLER_P12_BASE64/_PASSWORD, MAS_PROFILE_BASE64, ASC_KEY_ID and ASC_KEY_BASE64
# (for scripts/build-mas.sh package --upload; altool also finds the key in
# ~/.appstoreconnect/private_keys). All from the GitHub environment `release`. Never
# echoes them.
set -euo pipefail

tmp="${RUNNER_TEMP:?RUNNER_TEMP not set (CI only)}"
kc="$tmp/kantor-release.keychain-db"
key_dir="$tmp/asc"

need() {
  for v in "$@"; do
    [ -n "${!v:-}" ] || { echo "::error::$v is not set (secrets of the 'release' environment)" >&2; exit 1; }
  done
}

asc_key() {
  mkdir -p "$key_dir"
  key="$key_dir/AuthKey_${ASC_KEY_ID}.p8"
  printf '%s' "$ASC_KEY_BASE64" | base64 --decode > "$key"
  echo "ASC_KEY_PATH=$key" >> "$GITHUB_ENV"
}

new_keychain() {
  kcpass="$(uuidgen)"
  security create-keychain -p "$kcpass" "$kc"
  security set-keychain-settings -lut 21600 "$kc"
  security unlock-keychain -p "$kcpass" "$kc"
}

import_p12() { # BASE64_VAR PASSWORD_VAR
  local p12="$tmp/import.p12"
  printf '%s' "${!1}" | base64 --decode > "$p12"
  security import "$p12" -k "$kc" -f pkcs12 -P "${!2}" \
    -T /usr/bin/codesign -T /usr/bin/productbuild -T /usr/bin/security > /dev/null
  rm -f "$p12"
}

use_keychain() {
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kcpass" "$kc" > /dev/null
  # Search the new keychain first, keeping the existing ones.
  existing=()
  while IFS= read -r line; do
    line="${line#"${line%%[![:space:]]*}"}"; line="${line%\"}"; line="${line#\"}"
    [ -n "$line" ] && existing+=("$line")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "$kc" ${existing[@]+"${existing[@]}"}
  # Identity names only (no hashes): proves the import worked.
  security find-identity -v "$kc" | sed -n 's/^ *[0-9]*) [0-9A-F]* //p'
}

case "${1:-}" in
setup)
  umask 077
  need DEVELOPER_ID_P12_BASE64 DEVELOPER_ID_P12_PASSWORD ASC_KEY_ID ASC_KEY_BASE64
  asc_key
  new_keychain
  import_p12 DEVELOPER_ID_P12_BASE64 DEVELOPER_ID_P12_PASSWORD
  use_keychain
  security find-identity -v -p codesigning "$kc" | grep -q '"Developer ID Application' \
    || { echo "::error::no Developer ID Application identity in DEVELOPER_ID_P12_BASE64" >&2; exit 1; }
  ;;
setup-mas)
  umask 077
  need APPLE_DISTRIBUTION_P12_BASE64 APPLE_DISTRIBUTION_P12_PASSWORD \
    MAC_INSTALLER_P12_BASE64 MAC_INSTALLER_P12_PASSWORD MAS_PROFILE_BASE64 ASC_KEY_ID ASC_KEY_BASE64
  asc_key
  # altool looks for the key here (by --apiKey id).
  mkdir -p "$HOME/.appstoreconnect/private_keys"
  cp "$key" "$HOME/.appstoreconnect/private_keys/"
  profile="$tmp/kantor-mas.provisionprofile"
  printf '%s' "$MAS_PROFILE_BASE64" | base64 --decode > "$profile"
  echo "MAS_PROFILE=$profile" >> "$GITHUB_ENV"
  new_keychain
  import_p12 APPLE_DISTRIBUTION_P12_BASE64 APPLE_DISTRIBUTION_P12_PASSWORD
  import_p12 MAC_INSTALLER_P12_BASE64 MAC_INSTALLER_P12_PASSWORD
  use_keychain
  security find-identity -v -p codesigning "$kc" | grep -q '"Apple Distribution' \
    || { echo "::error::no Apple Distribution identity in APPLE_DISTRIBUTION_P12_BASE64" >&2; exit 1; }
  security find-identity -v "$kc" | grep -q '"3rd Party Mac Developer Installer\|"Mac Installer Distribution' \
    || { echo "::error::no Mac Installer Distribution identity in MAC_INSTALLER_P12_BASE64" >&2; exit 1; }
  ;;
cleanup)
  rm -rf "$key_dir" "$tmp/import.p12" "$tmp/kantor-mas.provisionprofile"
  if [ -n "${ASC_KEY_ID:-}" ]; then rm -f "$HOME/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID}.p8"; fi
  security delete-keychain "$kc" 2>/dev/null || true
  ;;
*)
  echo "usage: $0 setup | setup-mas | cleanup" >&2; exit 1 ;;
esac
