#!/bin/sh
# Install a prebuilt Kantor from GitHub Releases into ~/.local (no compiling).
#   curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | KANTOR_VERSION=v0.1.0 sh   # a tagged release instead of nightly
set -eu
repo="dzulfikar08/kantor"
tag="${KANTOR_VERSION:-nightly}"
case "$tag" in
  nightly) ;;
  v[0-9]*) ;;
  [0-9]*) tag="v$tag" ;;
  *) echo "KANTOR_VERSION must be 'nightly' or a release tag like v0.1.0 (got '$tag')" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  aarch64|arm64) arch=aarch64 ;;
  x86_64|amd64)  arch=x86_64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
case "$(uname -s)" in
  Darwin) os=macos ;;
  *)      os=linux ;;
esac
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
# Nightly assets are kantor-nightly-ARCH-OS.tar.gz with a .sha256 beside each;
# tagged releases (release.yml) are kantor-X.Y.Z-ARCH-linux.tar.gz with one SHA256SUMS.
if [ "$tag" = nightly ]; then name="kantor-nightly-${arch}-${os}"; else name="kantor-${tag#v}-${arch}-${os}"; fi
base="https://github.com/${repo}/releases/download/${tag}"
url="${base}/${name}.tar.gz"
prefix="${PREFIX:-$HOME/.local}"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

echo "Downloading $url"
curl -fL --progress-bar -o "$tmp/k.tar.gz" "$url"
if [ "$tag" = nightly ]; then
  curl -fsSL -o "$tmp/k.sha256" "$url.sha256"
  sum="$(cut -d' ' -f1 "$tmp/k.sha256")"
else
  curl -fsSL -o "$tmp/SHA256SUMS" "${base}/SHA256SUMS"
  sum="$(awk -v f="${name}.tar.gz" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")"
fi
[ -n "$sum" ] || { echo "no checksum for ${name}.tar.gz" >&2; exit 1; }
( cd "$tmp" && [ "$(sha256 k.tar.gz | cut -d' ' -f1)" = "$sum" ] ) || { echo "checksum mismatch for ${name}.tar.gz" >&2; exit 1; }
tar -C "$tmp" -xzf "$tmp/k.tar.gz"

if [ "$os" = macos ]; then
  # Kantor.app into ~/Applications (no sudo), plus a `kantor` command.
  apps="${KANTOR_APPS:-$HOME/Applications}"
  mkdir -p "$apps" "$prefix/bin"
  rm -rf "$apps/Kantor.app"
  cp -R "$tmp/$name/Kantor.app" "$apps/Kantor.app"
  xattr -dr com.apple.quarantine "$apps/Kantor.app" 2>/dev/null || true
  ln -sf "$apps/Kantor.app/Contents/MacOS/kantor" "$prefix/bin/kantor"
  echo "Installed $("$prefix/bin/kantor" --version) to $apps/Kantor.app"
  exit 0
fi

mkdir -p "$prefix/bin" "$prefix/share/applications" "$prefix/share/icons/hicolor/scalable/apps"
install -m755 "$tmp/$name/bin/kantor" "$prefix/bin/kantor"
for f in "$tmp/$name"/share/applications/*.desktop; do
  sed "s|^Exec=kantor|Exec=$prefix/bin/kantor|" "$f" > "$prefix/share/applications/$(basename "$f")"
done
install -m644 "$tmp/$name"/share/icons/hicolor/scalable/apps/*.svg "$prefix/share/icons/hicolor/scalable/apps/" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
echo "Installed $("$prefix/bin/kantor" --version) to $prefix/bin/kantor"
