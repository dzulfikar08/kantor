#!/bin/sh
# Install a prebuilt Kantor from GitHub Releases into ~/.local (no compiling).
#   curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh
#   KANTOR_VERSION=v0.1.0 sh install.sh      # a tagged release instead of nightly
set -eu
repo="dzulfikar08/kantor"
tag="${KANTOR_VERSION:-nightly}"
case "$(uname -m)" in
  aarch64|arm64) arch=aarch64 ;;
  x86_64|amd64)  arch=x86_64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
name="kantor-${tag}-${arch}-linux"
url="https://github.com/${repo}/releases/download/${tag}/${name}.tar.gz"
prefix="${PREFIX:-$HOME/.local}"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

echo "Downloading $url"
curl -fL --progress-bar -o "$tmp/k.tar.gz" "$url"
curl -fsSL -o "$tmp/k.sha256" "$url.sha256"
( cd "$tmp" && echo "$(cut -d' ' -f1 k.sha256)  k.tar.gz" | sha256sum -c - >/dev/null )
tar -C "$tmp" -xzf "$tmp/k.tar.gz"

mkdir -p "$prefix/bin" "$prefix/share/applications" "$prefix/share/icons/hicolor/scalable/apps"
install -m755 "$tmp/$name/bin/kantor" "$prefix/bin/kantor"
for f in "$tmp/$name"/share/applications/*.desktop; do
  sed "s|^Exec=kantor|Exec=$prefix/bin/kantor|" "$f" > "$prefix/share/applications/$(basename "$f")"
done
install -m644 "$tmp/$name"/share/icons/hicolor/scalable/apps/*.svg "$prefix/share/icons/hicolor/scalable/apps/" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
echo "Installed $("$prefix/bin/kantor" --version) to $prefix/bin/kantor"
