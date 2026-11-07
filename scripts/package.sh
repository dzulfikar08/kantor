#!/usr/bin/env bash
# package.sh BINARY OUT_PREFIX  ->  OUT_PREFIX.tar.gz (+ .sha256)
set -euo pipefail
bin="$1"; out="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
stage="$(mktemp -d)/$(basename "$out")"
mkdir -p "$stage/bin" "$stage/share/applications" "$(dirname "$out")"
install -m755 "$bin" "$stage/bin/kantor"
install -m644 "$root"/packaging/*.desktop "$stage/share/applications/"
install -m644 "$root"/LICENSE-MIT "$root"/LICENSE-APACHE "$root"/README.md "$stage/"
tar -C "$(dirname "$stage")" -czf "$out.tar.gz" "$(basename "$stage")"
( cd "$(dirname "$out")" && sha256sum "$(basename "$out").tar.gz" > "$(basename "$out").tar.gz.sha256" )
