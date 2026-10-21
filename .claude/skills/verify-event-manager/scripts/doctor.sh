#!/usr/bin/env bash
# Read-only health check: is this run's instance worth driving?
# Usage: doctor.sh <run-id>     Exit 0 only when every check passes.
set -uo pipefail
. "$(dirname "$0")/_common.sh"
load_run "$1"

bad=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; bad=1; fi; }

check "app process $APP_PID alive" 'alive "$APP_PID"'
check "mock idp process $IDP_PID alive" 'alive "$IDP_PID"'
check "GET $BASE/events -> 200" '[ "$(curl -s -o /dev/null -w "%{http_code}" "$BASE/events")" = 200 ]'
check "idp discovery issuer is $IDP" 'curl -s "$IDP/.well-known/openid-configuration" | grep -q "\"issuer\":\"$IDP\""'
check "${DB_KIND} database file $DB_FILE exists" '[ -f "$DB_FILE" ]'

bin=target/debug/event-manager
[ -f "$bin.exe" ] && bin="$bin.exe"
stale=$(find src migrations Cargo.toml -newer "$bin" -type f 2>/dev/null | head -3)
check "binary built after last source change${stale:+ (newer: $stale)}" '[ -z "$stale" ]'

# Port ownership: the listener must be the process up.sh started, not someone else's server.
if [ -r "/proc/$APP_PID/winpid" ]; then
  winpid=$(cat "/proc/$APP_PID/winpid")
  check "port $APP_PORT owned by app (winpid $winpid)" \
    'netstat -ano | tr -d "\r" | grep -E "127\.0\.0\.1:$APP_PORT .*LISTENING" | grep -qw "$winpid"'
elif command -v ss >/dev/null; then
  check "port $APP_PORT owned by app (pid $APP_PID)" 'ss -ltnpH "sport = :$APP_PORT" | grep -q "pid=$APP_PID,"'
else
  echo "skip port ownership (no /proc winpid or ss)"
fi

exit $bad
