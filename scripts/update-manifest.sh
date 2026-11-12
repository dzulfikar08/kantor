#!/usr/bin/env bash
# update-manifest.sh CHANNEL VERSION BUILD NOTES_URL BASE_URL OUT.json KEY=FILE...
#
# Writes OUT.json, the update manifest Kantor's in-app updater reads (crates/kantor_update,
# manifest.rs), and OUT.json.sig: base64 of the Ed25519 signature over OUT.json's exact bytes.
#
#   CHANNEL    stable | nightly
#   VERSION    the workspace version (0.2.1)
#   BUILD      0 for releases; nightlies: UTC commit time YYYYMMDDHHMMSS (the KANTOR_BUILD the
#              binaries were built with, so a nightly never offers itself as an update)
#   NOTES_URL  release page (also the "Download" link for installs that can't update themselves)
#   BASE_URL   where the assets are downloaded from; each asset's URL is BASE_URL/<file name>
#   KEY=FILE   one per asset: macos-universal-app, linux-{x86_64,aarch64}-{appimage,tarball},
#              windows-{x86_64,aarch64}-{setup,portable} (Kantor-Setup-*.exe, Kantor-*.zip)
#
# The signing key comes from UPDATE_SIGNING_KEY: base64 of the raw 32-byte Ed25519 seed (the
# GitHub `release` environment secret). Its public key must match UPDATE_PUBLIC_KEY, which
# defaults to the key the app embeds, so a wrong secret fails here instead of in the field.
# Needs openssl 3 (Ed25519 with -rawin). Never prints the key.
#
# Example:
#   UPDATE_SIGNING_KEY=... ./scripts/update-manifest.sh stable 0.2.1 0 \
#     https://github.com/dzulfikar08/kantor/releases/tag/v0.2.1 \
#     https://github.com/dzulfikar08/kantor/releases/download/v0.2.1 dist/update-stable.json \
#     linux-x86_64-appimage=dist/Kantor-0.2.1-x86_64.AppImage ...
set -euo pipefail
set +x

# The key in crates/kantor_update/src/manifest.rs (a test there keeps the two in sync).
EMBEDDED_PUBLIC_KEY="2sy5SODzbFlquPqJV9yDrc5bYyaZ+UY4X29N2GkB9tE="

[ $# -ge 7 ] || { sed -n '2,25p' "$0" >&2; exit 2; }
channel="$1"; version="$2"; build="$3"; notes="$4"; base="${5%/}"; out="$6"; shift 6

case "$channel" in stable|nightly) ;; *) echo "error: channel must be stable or nightly" >&2; exit 2 ;; esac
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' \
  || { echo "error: version '$version' is not semver" >&2; exit 2; }
echo "$build" | grep -Eq '^[0-9]+$' || { echo "error: build must be a number" >&2; exit 2; }
[ -n "${UPDATE_SIGNING_KEY:-}" ] || { echo "error: UPDATE_SIGNING_KEY is not set" >&2; exit 1; }

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
json_str() { # no quotes, backslashes or control characters in our URLs and names
  case "$1" in *[\"\\]*) echo "error: unexpected character in '$1'" >&2; exit 2 ;; esac
  printf '"%s"' "$1"
}

assets=""
seen=" "
for pair in "$@"; do
  key="${pair%%=*}"; file="${pair#*=}"
  case "$key" in
    macos-universal-app|linux-x86_64-appimage|linux-aarch64-appimage|linux-x86_64-tarball|linux-aarch64-tarball) ;;
    windows-x86_64-setup|windows-aarch64-setup|windows-x86_64-portable|windows-aarch64-portable) ;;
    *) echo "error: unknown asset key '$key'" >&2; exit 2 ;;
  esac
  case "$seen" in *" $key "*) echo "error: asset '$key' given twice" >&2; exit 2 ;; esac
  seen="$seen$key "
  [ -f "$file" ] || { echo "error: $file not found" >&2; exit 1; }
  size="$(wc -c < "$file" | tr -d ' ')"
  [ -n "$assets" ] && assets="$assets,"
  assets="$assets
    $(json_str "$key"): {
      \"url\": $(json_str "$base/$(basename "$file")"),
      \"sha256\": \"$(sha256 "$file")\",
      \"size\": $size
    }"
done
[ -n "$assets" ] || { echo "error: no assets" >&2; exit 2; }

mkdir -p "$(dirname "$out")"
cat > "$out" <<EOF
{
  "channel": $(json_str "$channel"),
  "version": $(json_str "$version"),
  "build": $build,
  "date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "notes_url": $(json_str "$notes"),
  "assets": {$assets
  }
}
EOF

# Sign. The key only ever lives in a private temp dir.
work="$(umask 077 && mktemp -d)"
trap 'rm -rf "$work"' EXIT
(
  umask 077
  # PKCS#8 DER for an Ed25519 private key = this fixed prefix + the 32-byte seed.
  printf '\x30\x2e\x02\x01\x00\x30\x05\x06\x03\x2b\x65\x70\x04\x22\x04\x20' > "$work/key.der"
  printf '%s' "$UPDATE_SIGNING_KEY" | openssl base64 -d -A > "$work/seed"
  [ "$(wc -c < "$work/seed" | tr -d ' ')" = 32 ] || { echo "error: UPDATE_SIGNING_KEY is not base64 of 32 bytes" >&2; exit 1; }
  cat "$work/seed" >> "$work/key.der"
  rm -f "$work/seed"
)
pub="$(openssl pkey -inform DER -in "$work/key.der" -pubout -outform DER | tail -c 32 | openssl base64 -A)"
want="${UPDATE_PUBLIC_KEY:-$EMBEDDED_PUBLIC_KEY}"
[ "$pub" = "$want" ] || { echo "error: UPDATE_SIGNING_KEY's public key is $pub, expected $want" >&2; exit 1; }

openssl pkeyutl -sign -rawin -inkey "$work/key.der" -keyform DER -in "$out" -out "$work/sig"
[ "$(wc -c < "$work/sig" | tr -d ' ')" = 64 ] || { echo "error: signing failed" >&2; exit 1; }
openssl base64 -A -in "$work/sig" > "$out.sig"
echo >> "$out.sig"

# Check it the way the app will: the public key alone, over the file as written.
openssl pkey -inform DER -in "$work/key.der" -pubout -out "$work/pub.pem"
openssl pkeyutl -verify -pubin -inkey "$work/pub.pem" -rawin -in "$out" -sigfile "$work/sig" >/dev/null \
  || { echo "error: the signature doesn't verify" >&2; exit 1; }

echo "wrote $out and $out.sig ($channel $version build $build, key $pub)"
cat "$out"
