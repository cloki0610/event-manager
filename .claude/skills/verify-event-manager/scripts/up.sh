#!/usr/bin/env bash
# Builds and starts an isolated instance: mock OIDC provider + event-manager on a fresh database.
# Usage: up.sh [run-id]
# Env:   VERIFY_APP_PORT (default 18080; next free port is used if taken)
#        VERIFY_TOOLCHAIN (e.g. +stable-x86_64-pc-windows-msvc when the GNU gcc is broken)
#        VERIFY_DB=duckdb   store data in DuckDB (builds with --features duckdb); default sqlite
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

case ${VERIFY_DB:-sqlite} in
  sqlite) FEATURES=() DB_FILE="$STATE/events.db" DATABASE_URL="sqlite://$STATE/events.db" ;;
  duckdb) FEATURES=(--features duckdb) DB_FILE="$STATE/events.duckdb" DATABASE_URL="duckdb://$STATE/events.duckdb" ;;
  *) echo "VERIFY_DB must be sqlite or duckdb" >&2; exit 2 ;;
esac

echo "building (${VERIFY_DB:-sqlite})..." >&2
cargo ${VERIFY_TOOLCHAIN:-} build ${FEATURES[@]+"${FEATURES[@]}"} --bin event-manager --example mock_idp >"$STATE/build.log" 2>&1 \
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

cat >"$STATE/env" <<EOF
APP_PORT=$APP_PORT
IDP_PORT=$IDP_PORT
BASE=$BASE
IDP=$IDP
DB_KIND=${VERIFY_DB:-sqlite}
DB_FILE=$DB_FILE
EOF

MOCK_IDP_ADDR="127.0.0.1:$IDP_PORT" MOCK_IDP_KEY="$STATE/idp_key.pem" \
  target/debug/examples/mock_idp >"$STATE/idp.log" 2>&1 &
echo "IDP_PID=$!" >>"$STATE/env"

for _ in $(seq 60); do
  curl -sf -o /dev/null "$IDP/.well-known/openid-configuration" && break
  sleep 0.5
done
curl -sf -o /dev/null "$IDP/.well-known/openid-configuration" || { cat "$STATE/idp.log" >&2; fail "mock idp not ready"; }

DATABASE_URL="$DATABASE_URL" BIND_ADDR="127.0.0.1:$APP_PORT" RUST_LOG=info \
  OIDC_ISSUER_URL="$IDP" OIDC_CLIENT_ID=event-manager \
  OIDC_REDIRECT_URL="$BASE/auth/callback" \
  target/debug/event-manager >"$STATE/app.log" 2>&1 &
echo "APP_PID=$!" >>"$STATE/env"

# Ready = migrations applied, SSO discovered, and the API answering a real query.
for _ in $(seq 60); do
  grep -q "listening on" "$STATE/app.log" 2>/dev/null \
    && [ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/events")" = 200 ] && break
  sleep 0.5
done
[ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/events")" = 200 ] || { cat "$STATE/app.log" >&2; fail "app not ready"; }

echo "ready: app $BASE  idp $IDP  db $DB_FILE (${VERIFY_DB:-sqlite})  evidence $EV" >&2
echo "$RUN_ID"
