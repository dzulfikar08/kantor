#!/usr/bin/env bash
# package-windows.sh KANTOR_EXE CONSOLE_EXE VERSION ARCH OUT_DIR [NAME_TAG]
#
# Windows packages from built binaries (run in Git Bash on a Windows runner):
#   OUT_DIR/Kantor-Setup-NAME_TAG-ARCH.exe   per-user Inno Setup installer (packaging/windows/kantor.iss)
#   OUT_DIR/Kantor-NAME_TAG-ARCH.zip         portable: unzip anywhere, run kantor.exe
#
#   KANTOR_EXE   target/<triple>/release/kantor.exe (GUI subsystem)
#   CONSOLE_EXE  target/<triple>/release/kantor-console.exe, shipped as kantor.com
#   VERSION      workspace version (0.3.1), stamped into the installer
#   ARCH         x64 | arm64
#   NAME_TAG     defaults to VERSION ("nightly" for build.yml)
#
# Code signing is optional: with WINDOWS_CERT_PFX_BASE64 (base64 of a .pfx) and
# WINDOWS_CERT_PASSWORD set, kantor.exe, kantor.com, the installer and its
# uninstaller are signed with signtool and timestamped (WINDOWS_TIMESTAMP_URL,
# default DigiCert's). Without them everything is built unsigned, and
# SmartScreen warns on first run (see docs/windows-signing.md).
set -euo pipefail
[ $# -ge 5 ] || { sed -n '2,18p' "$0" >&2; exit 2; }
exe="$1"; console="$2"; version="$3"; arch="$4"; out="$5"; tag="${6:-$3}"
case "$arch" in x64|arm64) ;; *) echo "error: ARCH must be x64 or arm64" >&2; exit 2 ;; esac
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
stage="$out/Kantor"
rm -rf "$stage"
mkdir -p "$stage"

# ── optional signing ────────────────────────────────────────────────────
sign=""
if [ -n "${WINDOWS_CERT_PFX_BASE64:-}" ]; then
  signtool="$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/signtool.exe 2>/dev/null | sort -V | tail -1)"
  [ -x "$signtool" ] || { echo "error: signtool.exe not found" >&2; exit 1; }
  pfx="$(mktemp -d)/cert.pfx"
  trap 'rm -f "$pfx"' EXIT
  printf '%s' "$WINDOWS_CERT_PFX_BASE64" | base64 -d > "$pfx"
  ts="${WINDOWS_TIMESTAMP_URL:-http://timestamp.digicert.com}"
  sign_file() {
    "$signtool" sign //fd SHA256 //td SHA256 //tr "$ts" \
      //f "$(cygpath -w "$pfx")" //p "${WINDOWS_CERT_PASSWORD:-}" "$(cygpath -w "$1")"
  }
  sign=1
  echo "signing with the certificate in WINDOWS_CERT_PFX_BASE64"
else
  echo "::notice::WINDOWS_CERT_PFX_BASE64 is not set: building unsigned Windows packages"
fi

cp "$exe" "$stage/kantor.exe"
cp "$console" "$stage/kantor.com"
[ -n "$sign" ] && { sign_file "$stage/kantor.exe"; sign_file "$stage/kantor.com"; }
tpl="$root/THIRD_PARTY_LICENSES.txt"; [ -f "$tpl" ] || python "$root/scripts/third-party-licenses.py" "$tpl"
# .txt so Notepad opens them with a double-click.
cp "$root/LICENSE" "$stage/LICENSE.txt"
cp "$root/NOTICE" "$stage/NOTICE.txt"
cp "$tpl" "$stage/THIRD_PARTY_LICENSES.txt"
cp "$root/README.md" "$stage/README.md"

# ── installer ───────────────────────────────────────────────────────────
iscc="$(command -v iscc || true)"
[ -n "$iscc" ] || iscc="/c/Program Files (x86)/Inno Setup 6/ISCC.exe"
if [ ! -x "$iscc" ]; then
  echo "Inno Setup not found; installing it with winget/choco"
  choco install innosetup -y --no-progress >/dev/null
fi
setup="Kantor-Setup-${tag}-${arch}"
args=(
  "/DAppVersion=${version}" "/DArch=${arch}"
  "/DSourceDir=$(cygpath -w "$stage")" "/DOutputDir=$(cygpath -w "$out")" "/DOutputName=${setup}"
)
if [ -n "$sign" ]; then
  # Inno signs the uninstaller it embeds, then the setup, with this command.
  st_w="$(cygpath -w "$signtool")"
  args+=("/Skantorsign=\"${st_w}\" sign /fd SHA256 /td SHA256 /tr ${ts} /f \"$(cygpath -w "$pfx")\" /p \"${WINDOWS_CERT_PASSWORD:-}\" \$f" "/DSignTool=kantorsign")
fi
MSYS2_ARG_CONV_EXCL='*' "$iscc" /Q "${args[@]}" "$(cygpath -w "$root/packaging/windows/kantor.iss")"
test -f "$out/${setup}.exe"

# ── portable zip ────────────────────────────────────────────────────────
cat > "$stage/kantor-portable.txt" <<EOF
This folder is a portable Kantor ${version}. Run kantor.exe; in a terminal, use
kantor.com (or "kantor" from this folder) for kantor convert, kantor pdf and
kantor mcp. Kantor updates the files in this folder itself; delete this file to
stop that. Settings live in %APPDATA%\\Kantor, the session in %LOCALAPPDATA%\\Kantor.
EOF
zip="Kantor-${tag}-${arch}.zip"
rm -f "$out/$zip"
( cd "$stage" && 7z a -tzip -mx=9 "$out/$zip" ./* >/dev/null )
rm -rf "$stage"

( cd "$out" && sha256sum "${setup}.exe" "$zip" | tee "Kantor-${tag}-${arch}-windows.sha256" )
