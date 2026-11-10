#!/usr/bin/env bash
# package-macos.sh BINARY|Kantor.app OUT_PREFIX  ->  OUT_PREFIX.tar.gz (+ .sha256)
# The tarball holds Kantor.app (what scripts/install.sh unpacks). Given a binary, the app is
# built by macos-app.sh and ad-hoc signed; given a Kantor.app (build-dmg.sh's, possibly
# Developer ID signed and notarised), it is packed as is.
set -euo pipefail
src="$1"; out="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
stage="$(mktemp -d)/$(basename "$out")"
mkdir -p "$stage" "$(dirname "$out")"
case "$src" in
  *.app|*.app/) ditto "$src" "$stage/Kantor.app" ;;
  *) "$root/scripts/macos-app.sh" "$src" "$stage" ;;
esac
install -m644 "$root"/LICENSE "$root"/README.md "$stage/"
tar -C "$(dirname "$stage")" -czf "$out.tar.gz" "$(basename "$stage")"
( cd "$(dirname "$out")" && shasum -a 256 "$(basename "$out").tar.gz" > "$(basename "$out").tar.gz.sha256" )
