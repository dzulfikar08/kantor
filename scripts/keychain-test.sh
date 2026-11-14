#!/usr/bin/env bash
# Test Kantor's keychain code against a real Secret Service, without
# touching the user's own keyring: a private D-Bus session with a
# throwaway gnome-keyring (its keyring files under a temporary folder).
#
#   scripts/keychain-test.sh            # needs gnome-keyring and dbus
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-$HOME/.cache}/kantor-keychain.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
export XDG_DATA_HOME="$tmp/data" XDG_CONFIG_HOME="$tmp/config" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
cd "$root"
# Build first, outside the private session (no keyring needed for that).
cargo test -p kantor_ai --test keychain --no-run
exec dbus-run-session -- bash -c '
  set -euo pipefail
  # A new login keyring, unlocked with a throwaway password.
  eval "$(printf "kantor-test" | gnome-keyring-daemon --unlock --components=secrets)"
  export GNOME_KEYRING_CONTROL
  KANTOR_TEST_KEYCHAIN=1 cargo test -p kantor_ai --test keychain -- --ignored --nocapture
'
