#!/bin/sh
# Install a prebuilt Kantor from GitHub Releases into ~/.local (no compiling, no sudo).
#   curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | KANTOR_VERSION=v0.2.0 sh   # a tagged release instead of nightly
#   curl -fsSL .../install.sh | sh -s -- --uninstall        # remove what this script installed
#
# Linux: $PREFIX/bin/kantor, the Kantor and Kantor Sheets launchers in
#   $PREFIX/share/applications and their icons in $PREFIX/share/icons/hicolor.
# macOS: $KANTOR_APPS/Kantor.app and a $PREFIX/bin/kantor link to it. Tagged releases from
#   v0.2.0 on have a macOS build (universal, notarised); v0.1.0 was Linux only.
# PREFIX defaults to ~/.local, KANTOR_APPS to ~/Applications.
# --uninstall removes exactly those files (not settings in ~/.config/kantor or the session in
# ~/.local/state/kantor; delete those yourself if you want them gone).
set -eu
repo="dzulfikar08/kantor"
prefix="${PREFIX:-$HOME/.local}"
apps="${KANTOR_APPS:-$HOME/Applications}"
case "$(uname -s)" in
  Darwin) os=macos ;;
  *)      os=linux ;;
esac

uninstall=0
for arg in "$@"; do
  case "$arg" in
    --uninstall) uninstall=1 ;;
    -h|--help) sed -n '2,14p' "$0" 2>/dev/null || echo "usage: install.sh [--uninstall]"; exit 0 ;;
    *) echo "unknown option: $arg (usage: install.sh [--uninstall])" >&2; exit 1 ;;
  esac
done

if [ "$uninstall" = 1 ]; then
  removed=0
  rm_if() { # path...
    for p in "$@"; do
      if [ -e "$p" ] || [ -L "$p" ]; then rm -rf "$p" && echo "removed $p" && removed=1; fi
    done
  }
  if [ "$os" = macos ]; then
    rm_if "$apps/Kantor.app"
    # Only our link (or a binary in the macOS layout), never someone else's kantor.
    if [ -L "$prefix/bin/kantor" ]; then
      case "$(readlink "$prefix/bin/kantor")" in *Kantor.app/Contents/MacOS/kantor) rm_if "$prefix/bin/kantor" ;; esac
    fi
  else
    rm_if "$prefix/bin/kantor" \
      "$prefix/share/applications/kantor.desktop" \
      "$prefix/share/applications/kantor-sheets.desktop" \
      "$prefix/share/icons/hicolor/scalable/apps/kantor.svg" \
      "$prefix/share/icons/hicolor/scalable/apps/kantor-sheets.svg"
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
    command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
  fi
  if [ "$removed" = 1 ]; then
    echo "Kantor uninstalled. Settings (~/.config/kantor) and session (~/.local/state/kantor) were kept."
  else
    echo "Nothing to remove: no Kantor installed by this script under $prefix$( [ "$os" = macos ] && echo " or $apps")."
  fi
  exit 0
fi

tag="${KANTOR_VERSION:-nightly}"
case "$tag" in
  nightly) ;;
  v[0-9]*) ;;
  [0-9]*) tag="v$tag" ;;
  *) echo "KANTOR_VERSION must be 'nightly' or a release tag like v0.2.0 (got '$tag')" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  aarch64|arm64) arch=aarch64 ;;
  x86_64|amd64)  arch=x86_64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
# Nightly assets: kantor-nightly-ARCH-OS.tar.gz with a .sha256 beside each (macOS: aarch64 only).
# Tagged releases (release.yml): kantor-X.Y.Z-ARCH-linux.tar.gz and, from v0.2.0,
# kantor-X.Y.Z-universal-macos.tar.gz, with one SHA256SUMS.
if [ "$tag" = nightly ]; then
  name="kantor-nightly-${arch}-${os}"
elif [ "$os" = macos ]; then
  name="kantor-${tag#v}-universal-macos"
else
  name="kantor-${tag#v}-${arch}-${os}"
fi
base="https://github.com/${repo}/releases/download/${tag}"
url="${base}/${name}.tar.gz"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

# Check the asset exists first, so a missing one gets a clear message instead of a 404.
if ! curl -fsIL -o /dev/null "$url"; then
  if ! curl -fsIL -o /dev/null "https://github.com/${repo}/releases/tag/${tag}"; then
    echo "error: no Kantor release '$tag' (see https://github.com/${repo}/releases)" >&2
  elif [ "$os" = macos ] && [ "$tag" != nightly ]; then
    echo "error: Kantor $tag has no macOS build (macOS builds start with v0.2.0)." >&2
    echo "Install the latest release or the nightly instead:  curl -fsSL https://raw.githubusercontent.com/${repo}/main/scripts/install.sh | sh" >&2
  elif [ "$os" = macos ]; then
    echo "error: the nightly has no macOS build for $arch (it is built for Apple Silicon only)." >&2
    echo "Use a tagged release (universal): KANTOR_VERSION=<tag> from https://github.com/${repo}/releases" >&2
  else
    echo "error: Kantor $tag has no build for $arch Linux ($name.tar.gz)" >&2
  fi
  exit 1
fi

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

# If $prefix/bin is not on PATH, say exactly what to add for the user's shell.
path_hint() {
  case ":$PATH:" in *":$prefix/bin:"*) return 0 ;; esac
  echo
  echo "$prefix/bin is not on your PATH, so the 'kantor' command won't be found yet. Add it:"
  case "$(basename "${SHELL:-sh}")" in
    zsh)  echo "  echo 'export PATH=\"$prefix/bin:\$PATH\"' >> ~/.zshrc && exec zsh" ;;
    bash) if [ "$os" = macos ]; then rc=.bash_profile; else rc=.bashrc; fi
          echo "  echo 'export PATH=\"$prefix/bin:\$PATH\"' >> ~/$rc && exec bash" ;;
    fish) echo "  fish_add_path $prefix/bin" ;;
    *)    echo "  export PATH=\"$prefix/bin:\$PATH\"   (in your shell's startup file)" ;;
  esac
}

if [ "$os" = macos ]; then
  # Kantor.app into ~/Applications (no sudo), plus a `kantor` command.
  mkdir -p "$apps" "$prefix/bin"
  rm -rf "$apps/Kantor.app"
  cp -R "$tmp/$name/Kantor.app" "$apps/Kantor.app"
  # Tagged releases are notarised; the nightly is ad-hoc signed and would be blocked.
  xattr -dr com.apple.quarantine "$apps/Kantor.app" 2>/dev/null || true
  ln -sf "$apps/Kantor.app/Contents/MacOS/kantor" "$prefix/bin/kantor"
  echo "Installed $("$prefix/bin/kantor" --version) to $apps/Kantor.app"
  path_hint
  echo "Uninstall: curl -fsSL https://raw.githubusercontent.com/${repo}/main/scripts/install.sh | sh -s -- --uninstall"
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
path_hint
echo "Uninstall: curl -fsSL https://raw.githubusercontent.com/${repo}/main/scripts/install.sh | sh -s -- --uninstall"
