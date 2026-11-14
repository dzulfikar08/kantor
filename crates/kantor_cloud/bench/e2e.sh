#!/bin/bash
# End-to-end tests of kantor_cloud against the Nextcloud stack in nextcloud/
# (and optionally OpenCloud), with Toxiproxy for the offline cases.
#
#   DOCKER_HOST=... ./e2e.sh nextcloud     # stacks must be up (docker compose up -d)
#   DOCKER_HOST=... ./e2e.sh opencloud
#
# Nextcloud: creates users alice and bob, app passwords, a folder "Bersama"
# that alice shares with bob. OpenCloud: uses the demo users alan and lynn.
# Then runs `cargo test -p kantor_cloud --test e2e_server
# -- --ignored --test-threads=1`. Credentials go to $E2E_ENV (default
# ~/.cache/kantor-cloud-e2e.env) for the UI tests.
set -euo pipefail
export TMPDIR=${TMPDIR:-$HOME/.cache}
server=${1:-nextcloud}
E2E_ENV=${E2E_ENV:-$HOME/.cache/kantor-cloud-e2e.env}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)

case $server in
nextcloud)
  base=http://127.0.0.1:18680
  occ() { docker exec -u www-data kspike-nc-app-1 php occ "$@"; }
  for u in alice bob; do
    if ! occ user:info "$u" >/dev/null 2>&1; then
      docker exec -e OC_PASS="Pass-$u-2026!" -u www-data kspike-nc-app-1 \
        php occ user:add --password-from-env --display-name "${u^} Santoso" "$u" >/dev/null
    fi
  done
  # Brute-force protection would slow down the many sign-ins of a test run.
  occ config:system:set auth.bruteforce.protection.enabled --value=false --type=boolean >/dev/null
  apass() { docker exec -e NC_PASS="Pass-$1-2026!" -u www-data kspike-nc-app-1 \
    php occ user:auth-tokens:add --password-from-env "$1" | tail -1; }
  AP=$(apass alice)
  BP=$(apass bob)
  curl -sf -u "alice:$AP" -X MKCOL "$base/remote.php/dav/files/alice/Bersama" -o /dev/null || true
  # Share Bersama with bob (read/write/create/delete).
  curl -sf -u "alice:$AP" -H "OCS-APIRequest: true" \
    -d "path=/Bersama&shareType=0&shareWith=bob&permissions=31" \
    "$base/ocs/v2.php/apps/files_sharing/api/v1/shares" -o /dev/null || true
  # Bob sees it at the top of his files once accepted (Nextcloud auto-accepts by default).
  ALICE_URL=$base/remote.php/dav/files/alice/
  BOB_URL=$base/remote.php/dav/files/bob/
  PROXY_URL=http://127.0.0.1:28080/remote.php/dav/files/alice/
  TOXI_PROXY=nextcloud
  EXTRA="export KANTOR_E2E_ALICE_NAME='Alice Santoso'
export KANTOR_E2E_BOB_NAME='Bob Santoso'"
  ;;
opencloud)
  # Demo users (IDM_CREATE_DEMO_USERS in opencloud/compose.yaml): alan
  # shares "Bersama" with lynn, who sees it under Shares/.
  base=http://127.0.0.1:9200
  tok() { docker exec kspike-oc-opencloud-1 opencloud auth-app create --user-name "$1" --expiration 72h 2>&1 | sed -n 's/^ *token: //p'; }
  AP=$(tok alan); BP=$(tok lynn)
  E="alan:$AP"
  curl -s -u "$E" -X MKCOL "$base/remote.php/dav/files/alan/Bersama" -o /dev/null || true
  drive=$(curl -s -u "$E" "$base/graph/v1.0/me/drives" | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(next(x["id"] for x in d["value"] if x["driveType"]=="personal"))')
  item=$(curl -s -u "$E" -X PROPFIND -H 'Depth: 0' --data '<d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop><oc:fileid/></d:prop></d:propfind>' "$base/remote.php/dav/files/alan/Bersama" | grep -o '<oc:fileid>[^<]*' | sed 's/.*>//')
  lynn=$(curl -s -u "$E" "$base/graph/v1.0/users?%24search=%22lynn%22" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["value"][0]["id"])')
  role=$(curl -s -u "$E" "$base/graph/v1beta1/roleManagement/permissions/roleDefinitions" | python3 -I -c 'import json,sys; print(next(r["id"] for r in json.load(sys.stdin) if r.get("displayName")=="Can edit"))')
  curl -s -u "$E" -H 'Content-Type: application/json' -X POST \
    -d "{\"recipients\":[{\"objectId\":\"$lynn\",\"@libre.graph.recipient.type\":\"user\"}],\"roles\":[\"$role\"]}" \
    "$base/graph/v1beta1/drives/$drive/items/$item/invite" -o /dev/null || true
  ALICE_URL=$base/remote.php/dav/files/alan/
  BOB_URL=$base/remote.php/dav/files/lynn/
  PROXY_URL=http://127.0.0.1:29200/remote.php/dav/files/alan/
  TOXI_PROXY=opencloud
  EXTRA="export KANTOR_E2E_BOB_SHARED=Shares/Bersama
export KANTOR_E2E_ALICE_NAME='Alan Turing'
export KANTOR_E2E_BOB_NAME='Lynn Conway'"
  ;;
esac

cat > "$E2E_ENV" <<EOF
export KANTOR_E2E_ALICE_URL=$ALICE_URL
export KANTOR_E2E_ALICE_PASS='$AP'
export KANTOR_E2E_BOB_URL=$BOB_URL
export KANTOR_E2E_BOB_PASS='$BP'
export KANTOR_E2E_SHARED=Bersama
export KANTOR_E2E_ALICE_PROXY_URL=$PROXY_URL
export KANTOR_E2E_TOXI_API=http://127.0.0.1:28474
export KANTOR_E2E_TOXI_PROXY=$TOXI_PROXY
$EXTRA
EOF
. "$E2E_ENV"
cd "$root"
cargo test -p kantor_cloud --test e2e_server -- --ignored --test-threads=1 --nocapture
