#!/usr/bin/env bash
# Builds and starts an isolated instance: mock OIDC provider + the Worker under `wrangler dev`
# on a fresh local D1 database.
# Usage: up.sh [run-id]
# Env:   VERIFY_APP_PORT (default 18080; next free port is used if taken)
#        VERIFY_TOOLCHAIN (e.g. +stable-x86_64-pc-windows-msvc when the GNU gcc is broken)
#        VERIFY_PERSIST (reuse this D1 persist dir instead of a fresh one; for restart checks)
# Last stdout line: the run id to pass to every other script.
set -uo pipefail
. "$(dirname "$0")/_common.sh"

RUN_ID=${1:-$(date +%Y%m%d-%H%M%S)-$$}
RUN="target/verify/$RUN_ID"
STATE="$RUN/state"
EV="$RUN/evidence"
[ -e "$STATE" ] && { echo "run '$RUN_ID' already has state; pick another id or run down.sh" >&2; exit 2; }
mkdir -p "$STATE" "$EV"

fail() { echo "up: $*" >&2; "$(dirname "$0")/down.sh" "$RUN_ID" >&2; exit 1; }

[ -f node_modules/wrangler/bin/wrangler.js ] || fail "wrangler missing; run npm ci"
WRANGLER=(node node_modules/wrangler/bin/wrangler.js)
PERSIST=${VERIFY_PERSIST:-$STATE/d1}

echo "building mock idp..." >&2
cargo ${VERIFY_TOOLCHAIN:-} build -p e2e --bin mock_idp >"$STATE/build.log" 2>&1 \
  || { cat "$STATE/build.log" >&2; fail "build failed"; }

port=${VERIFY_APP_PORT:-18080}
until port_free "$port"; do port=$((port + 1)); done
APP_PORT=$port
port=$((port + 1))
until port_free "$port"; do port=$((port + 1)); done
IDP_PORT=$port
BASE="http://127.0.0.1:$APP_PORT"
IDP="http://127.0.0.1:$IDP_PORT"

# openidconnect wants a PKCS#1 key; OpenSSL 3 needs -traditional for that, 1.x emits it by default.
openssl genrsa -traditional -out "$STATE/idp_key.pem" 2048 2>/dev/null \
  || openssl genrsa -out "$STATE/idp_key.pem" 2048 2>/dev/null \
  || fail "openssl genrsa failed"

cat >"$STATE/env" <<EOV
APP_PORT=$APP_PORT
IDP_PORT=$IDP_PORT
BASE=$BASE
IDP=$IDP
PERSIST=$PERSIST
EOV

MOCK_IDP_ADDR="127.0.0.1:$IDP_PORT" MOCK_IDP_KEY="$STATE/idp_key.pem" \
  target/debug/mock_idp >"$STATE/idp.log" 2>&1 &
echo "IDP_PID=$!" >>"$STATE/env"

for _ in $(seq 60); do
  curl -sf -o /dev/null "$IDP/.well-known/openid-configuration" && break
  sleep 0.5
done
curl -sf -o /dev/null "$IDP/.well-known/openid-configuration" || { cat "$STATE/idp.log" >&2; fail "mock idp not ready"; }

"${WRANGLER[@]}" d1 migrations apply DB --local --persist-to "$PERSIST" >"$STATE/migrate.log" 2>&1 \
  || { cat "$STATE/migrate.log" >&2; fail "migrations failed"; }

echo "starting wrangler dev (builds the Worker; first build takes minutes)..." >&2
"${WRANGLER[@]}" dev --local --ip 127.0.0.1 --port "$APP_PORT" --persist-to "$PERSIST" \
  --var "OIDC_ISSUER_URL:$IDP" --var OIDC_CLIENT_ID:event-manager \
  --var "OIDC_REDIRECT_URL:$BASE/auth/callback" >"$STATE/app.log" 2>&1 &
echo "APP_PID=$!" >>"$STATE/env"

# Ready = Worker built and the API answering a real D1 query.
for _ in $(seq 600); do
  [ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/events")" = 200 ] && break
  sleep 0.5
done
[ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/events")" = 200 ] || { cat "$STATE/app.log" >&2; fail "app not ready"; }

echo "ready: app $BASE  idp $IDP  d1 $PERSIST  evidence $EV" >&2
echo "$RUN_ID"
