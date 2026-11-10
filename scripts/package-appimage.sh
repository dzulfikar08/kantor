#!/usr/bin/env bash
# package-appimage.sh BINARY OUT.AppImage   (Linux x86_64 or aarch64, the binary's own arch)
#
# A hand-rolled AppDir (AppRun, kantor.desktop, icon, usr/bin/kantor, the launchers and icons
# under usr/share) packed by appimagetool with the static type-2 runtime, both pinned below.
#
# Nothing is bundled but the binary. Everything it links is on the AppImage excludelist
# (glibc, libgcc_s, fontconfig, freetype, libxcb, …) or belongs to the host's desktop stack
# like those: libxkbcommon(-x11), and Vulkan, Wayland and X11 libraries, which GPUI opens at
# run time and must match the host's drivers and compositor. The check below fails if the
# binary starts linking anything else, so a new dependency is a decision, not an accident.
#
# glibc baseline: whatever the build host has. CI builds on Ubuntu 24.04 (glibc 2.39), so
# the AppImage runs on Ubuntu 24.04+, Debian 13+, Fedora 40+, current Arch, … (not 22.04).
# Running it needs FUSE 3 (fusermount3), present on desktop distros; without it:
#   ./Kantor-x86_64.AppImage --appimage-extract-and-run
set -euo pipefail
bin="$1"; out="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

case "$(uname -m)" in
  x86_64)
    arch=x86_64
    tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64|arm64)
    arch=aarch64
    tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
tool_url="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-${arch}.AppImage"
runtime_url="https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-${arch}"

file -b "$bin" | grep -q "$(case $arch in x86_64) echo x86-64 ;; aarch64) echo aarch64 ;; esac)" \
  || { echo "error: $bin is not an $arch binary" >&2; exit 1; }

# Shared libraries the binary may link (ldd names, no paths).
allowed='linux-vdso.so.1 linux-gate.so.1 ld-linux-x86-64.so.2 ld-linux-aarch64.so.1
libc.so.6 libm.so.6 libgcc_s.so.1 libdl.so.2 libpthread.so.0 librt.so.1
libfontconfig.so.1 libfreetype.so.6 libexpat.so.1 libz.so.1 libbz2.so.1.0 libpng16.so.16
libbrotlidec.so.1 libbrotlicommon.so.1
libxcb.so.1 libXau.so.6 libXdmcp.so.6 libbsd.so.0 libmd.so.0
libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1'
unexpected=""
for lib in $(ldd "$bin" | awk '{ print $1 }' | xargs -n1 basename); do
  case " $(echo "$allowed" | tr '\n' ' ') " in *" $lib "*) ;; *) unexpected="$unexpected $lib" ;; esac
done
[ -z "$unexpected" ] || { echo "error: kantor links libraries the AppImage doesn't plan for:$unexpected (bundle them or add them to the list in $0)" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fetch() { # url sha256 dest
  curl -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c --quiet - || { echo "error: checksum mismatch for $1" >&2; exit 1; }
}
fetch "$tool_url" "$tool_sha" "$work/appimagetool"
fetch "$runtime_url" "$runtime_sha" "$work/runtime"
chmod +x "$work/appimagetool"

app="$work/Kantor.AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/scalable/apps" \
  "$app/usr/share/metainfo" "$app/usr/share/doc/kantor"
install -m755 "$bin" "$app/usr/bin/kantor"
install -m644 "$root"/packaging/*.desktop "$app/usr/share/applications/"
install -m644 "$root"/assets/icons/*.svg "$app/usr/share/icons/hicolor/scalable/apps/"
install -m644 "$root/LICENSE" "$root/README.md" "$app/usr/share/doc/kantor/"
# appimagetool wants exactly one .desktop and its icon at the top.
cp "$root/packaging/kantor.desktop" "$app/kantor.desktop"
echo "X-AppImage-Version=$version" >> "$app/kantor.desktop"
cp "$root/assets/icons/kantor.svg" "$app/kantor.svg"
ln -s kantor.svg "$app/.DirIcon"
cat > "$app/AppRun" <<'EOF'
#!/bin/sh
# Kantor AppImage entry point. No LD_LIBRARY_PATH: nothing is bundled but the binary.
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/kantor" "$@"
EOF
chmod 755 "$app/AppRun"

mkdir -p "$(dirname "$out")"
# --appimage-extract-and-run: CI runners have no FUSE for running appimagetool itself.
ARCH="$arch" "$work/appimagetool" --appimage-extract-and-run --no-appstream \
  --runtime-file "$work/runtime" "$app" "$out"
chmod 755 "$out"
"$out" --appimage-extract-and-run --version
ls -l "$out"
