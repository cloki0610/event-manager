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
check "local D1 persist dir $PERSIST exists" '[ -d "$PERSIST" ]'

# The Worker listens from a workerd child of wrangler, so the listener must be a workerd process.
if command -v netstat >/dev/null && command -v tasklist >/dev/null; then
  lpid=$(netstat -ano | tr -d "\r" | grep -E "127\.0\.0\.1:$APP_PORT .*LISTENING" | awk '{print $NF}' | head -1)
  check "port $APP_PORT owned by workerd (pid ${lpid:-none})" \
    '[ -n "$lpid" ] && tasklist //FI "PID eq $lpid" //NH | grep -qi workerd'
elif command -v ss >/dev/null; then
  check "port $APP_PORT owned by workerd" 'ss -ltnpH "sport = :$APP_PORT" | grep -q workerd'
else
  echo "skip port ownership (no netstat+tasklist or ss)"
fi

exit $bad
