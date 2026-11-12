#!/usr/bin/env bash
# windows-smoke.sh SETUP.exe PORTABLE.zip VERSION [SCREENSHOT.png]
#
# On a Windows runner (Git Bash): install Kantor silently with the installer,
# check what it registered, run the command line (kantor.com) and a
# conversion, start the app and talk to it over MCP, take a screenshot, check
# the portable zip, then uninstall silently and check nothing is left.
set -euo pipefail
[ $# -ge 3 ] || { sed -n '2,7p' "$0" >&2; exit 2; }
setup="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
zip="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
version="$3"
shot="${4:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
export MSYS2_ARG_CONV_EXCL='*'

fail() { echo "::error::$*"; exit 1; }
expect_version() { # LABEL OUTPUT
  echo "$1: $2"
  case "$2" in "kantor ${version}"*) ;; *) fail "$1 does not report kantor ${version}" ;; esac
}

work="$(cygpath -u "$RUNNER_TEMP")/kantor-smoke"
rm -rf "$work"; mkdir -p "$work"
app="$(cygpath -u "$LOCALAPPDATA")/Programs/Kantor"

echo "── silent install"
"$setup" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- "/LOG=$(cygpath -w "$work/install.log")"
test -f "$app/kantor.exe" || { cat "$work/install.log" || true; fail "kantor.exe not in $app"; }
ls -l "$app"
test -f "$app/kantor.com" || fail "kantor.com missing"
test -f "$app/unins000.exe" || fail "no uninstaller"
test ! -e "$app/kantor-portable.txt" || fail "the portable marker was installed"
reg query 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{6E4B1C52-8F0A-4C1E-9D7B-4B3A2F6C9E11}_is1' /v DisplayVersion
for ext in .xlsx .docx .pptx .csv; do
  reg query "HKCU\\Software\\Classes\\${ext}\\OpenWithProgids" >/dev/null || fail "no Open with entry for ${ext}"
done
# "Open with", not a new default: the extension's own default value is untouched.
if reg query 'HKCU\Software\Classes\.xlsx' /ve 2>/dev/null | grep -q Kantor; then
  fail "the installer made Kantor the default for .xlsx"
fi
ls "$(cygpath -u "$APPDATA")/Microsoft/Windows/Start Menu/Programs/" | grep -i kantor || fail "no Start menu shortcut"

echo "── command line"
expect_version "kantor.com --version" "$("$app/kantor.com" --version)"
# The GUI-subsystem exe through a pipe, as the updater runs it.
expect_version "kantor.exe --version (piped)" "$("$app/kantor.exe" --version | cat)"
"$app/kantor.com" --help | head -3
cp "$root/tests/corpus/xlsx/01_formulas.xlsx" "$work/book.xlsx"
"$app/kantor.com" convert "$(cygpath -w "$work/book.xlsx")" "$(cygpath -w "$work/book.pdf")"
head -c 5 "$work/book.pdf" | grep -q '%PDF-' || fail "convert did not write a PDF"
"$app/kantor.com" convert "$(cygpath -w "$work/book.xlsx")" "$(cygpath -w "$work/book.csv")"
head -3 "$work/book.csv"
"$app/kantor.com" pdf --help | head -2

echo "── the app, and MCP against it"
"$app/kantor.exe" &
pid=$!
run="$(cygpath -u "$LOCALAPPDATA")/Kantor/run"
for _ in $(seq 1 60); do
  ls "$run"/kantor-*.port >/dev/null 2>&1 && break
  sleep 1
done
ls -l "$run" || true
ls "$run"/kantor-*.port >/dev/null 2>&1 || fail "Kantor started no MCP endpoint within 60 s (did the window open?)"
sleep 3
if [ -n "$shot" ]; then
  powershell -NoProfile -Command "
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    \$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    \$img = New-Object System.Drawing.Bitmap \$b.Width, \$b.Height
    \$g = [System.Drawing.Graphics]::FromImage(\$img)
    \$g.CopyFromScreen(\$b.Location, [System.Drawing.Point]::Empty, \$b.Size)
    \$img.Save('$(cygpath -w "$shot")', [System.Drawing.Imaging.ImageFormat]::Png)
  " || echo "::warning::no screenshot"
fi
reply="$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_documents","arguments":{}}}' \
  | "$app/kantor.com" mcp --no-launch)"
echo "$reply"
echo "$reply" | grep '"id":1' | grep -q '"isError":false' || fail "MCP list_documents failed against the running app"
taskkill /IM kantor.exe /F || true
wait "$pid" 2>/dev/null || true

echo "── portable zip"
mkdir -p "$work/portable"
7z x -y "-o$(cygpath -w "$work/portable")" "$(cygpath -w "$zip")" >/dev/null
ls -l "$work/portable"
test -f "$work/portable/kantor-portable.txt" || fail "no portable marker in the zip"
expect_version "portable kantor.com --version" "$("$work/portable/kantor.com" --version)"

echo "── silent uninstall"
"$app/unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
# The uninstaller runs from a temporary copy and returns at once.
for _ in $(seq 1 60); do
  [ -e "$app/kantor.exe" ] || break
  sleep 1
done
[ ! -e "$app/kantor.exe" ] || fail "kantor.exe still installed after uninstall"
if reg query 'HKCU\Software\Classes\.xlsx\OpenWithProgids' /v Kantor.Sheet >/dev/null 2>&1; then
  fail "the Open with entry outlived the uninstall"
fi
if reg query 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{6E4B1C52-8F0A-4C1E-9D7B-4B3A2F6C9E11}_is1' >/dev/null 2>&1; then
  fail "the uninstall entry outlived the uninstall"
fi
echo "Windows smoke test passed"
