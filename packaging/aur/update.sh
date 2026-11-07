#!/usr/bin/env bash
# update.sh VERSION [SHA256SUMS-file]
# Points packaging/aur/kantor-bin at release vVERSION: sets pkgver (pkgrel=1 on a new
# version), fills sha256sums_{x86_64,aarch64} from the release's SHA256SUMS and
# regenerates .SRCINFO (needs makepkg, i.e. an Arch box).
# By default SHA256SUMS is downloaded from the GitHub release; pass a local file to test.
set -euo pipefail
version="${1:?usage: update.sh VERSION [SHA256SUMS-file]}"
version="${version#v}"
dir="$(cd "$(dirname "$0")/kantor-bin" && pwd)"
pkgbuild="$dir/PKGBUILD"
repo="dzulfikar08/kantor"

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
if [ -n "${2:-}" ]; then
  cp "$2" "$tmp/SHA256SUMS"
else
  url="https://github.com/${repo}/releases/download/v${version}/SHA256SUMS"
  echo "Downloading $url"
  curl -fsSL -o "$tmp/SHA256SUMS" "$url"
fi

sum_for() {
  local f="kantor-${version}-$1-linux.tar.gz" s
  s="$(awk -v f="$f" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")"
  if ! [[ "$s" =~ ^[0-9a-f]{64}$ ]]; then
    echo "no sha256 for $f in SHA256SUMS" >&2
    exit 1
  fi
  printf '%s' "$s"
}
x86="$(sum_for x86_64)"
arm="$(sum_for aarch64)"

old="$(sed -n 's/^pkgver=//p' "$pkgbuild")"
sed -i \
  -e "s/^pkgver=.*/pkgver=${version}/" \
  -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('${x86}')/" \
  -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('${arm}')/" \
  "$pkgbuild"
if [ "$old" != "$version" ]; then
  sed -i 's/^pkgrel=.*/pkgrel=1/' "$pkgbuild"
fi

( cd "$dir" && makepkg --printsrcinfo > .SRCINFO )
echo "kantor-bin -> ${version} (pkgrel $(sed -n 's/^pkgrel=//p' "$pkgbuild"))"
echo "  x86_64  ${x86}"
echo "  aarch64 ${arm}"
