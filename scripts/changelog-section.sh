#!/usr/bin/env bash
# changelog-section.sh VERSION [CHANGELOG]  ->  prints the body of "## [VERSION]" (no heading).
# Fails if the section is missing or empty. Used for GitHub Release notes.
set -euo pipefail
version="${1#v}"
file="${2:-$(cd "$(dirname "$0")/.." && pwd)/CHANGELOG.md}"
body="$(awk -v v="$version" '
  /^## \[/ { if (on) exit; if (index($0, "## [" v "]") == 1) { on = 1; next } }
  /^\[[^]]+\]: / { if (on) exit }
  on { print }
' "$file" | sed -e '/./,$!d')"   # drop leading blank lines
if [ -z "$(printf '%s' "$body" | tr -d '[:space:]')" ]; then
  echo "no '## [$version]' section in $file" >&2
  exit 1
fi
printf '%s\n' "$body"
