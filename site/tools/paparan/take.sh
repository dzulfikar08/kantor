#!/usr/bin/env bash
# take.sh LANG SCRIPT PROMPT TAG: a scripted chat job, then the workbook opened; dark then light.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
lang=$1; script=$2; prompt=$3; tag=$4
k=$here/k.sh
CHAT_SCRIPT=$script FAKE_SAVE_PATH=${WORK:-$HOME/.cache/kantor-site-paparan}/penjualan-2026.csv \
  $here/kstart.sh dark "$lang" || exit 1
$k cmd "${PALETTE:-Paparan AI Chat}"; sleep 2
$k click ${KINDX:-1287} 886; sleep 0.5          # Workbook
$k click 1493 886; sleep 3            # attach (fake portal answers with the CSV)
$k click 1300 828; sleep 0.3
$k type "$prompt"; sleep 0.5
$k key Return; sleep 7
$k shot "$tag-mid-dark"
sleep 17
$k click ${OPENX:-1414} 743; sleep 4            # Open
KANTOR_SOCKET_DIR=${WORK:-$HOME/.cache/kantor-site-paparan}/run/sock KANTOR_BIN=${KANTOR_BIN:-$(command -v kantor)} \
  python3 -I $here/../mcp_call.py sheets_select "{\"range\":\"${SEL:-J17}\"}" >/dev/null
sleep 1
$k move 700 990; sleep 0.5
$k shot "$tag-dark"
$k theme light; sleep 3
$k shot "$tag-light"
$k stop
