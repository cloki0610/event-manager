#!/usr/bin/env bash
# Stops the processes this run started, keeps logs as evidence, deletes the run's state.
# Usage: down.sh <run-id>      Evidence stays in target/verify/<run-id>/evidence.
set -uo pipefail
. "$(dirname "$0")/_common.sh"
RUN_ID=${1:?run id required}
RUN="target/verify/$RUN_ID"
STATE="$RUN/state"
EV="$RUN/evidence"
[ -d "$STATE" ] || { echo "down: nothing to clean for '$RUN_ID'"; exit 0; }
[ -f "$STATE/env" ] && . "$STATE/env"

for pid in ${APP_PID:-} ${IDP_PID:-}; do
  alive "$pid" || continue
  # wrangler dev serves from a workerd child process, so stop the whole tree.
  if [ -r "/proc/$pid/winpid" ]; then
    taskkill //T //F //PID "$(cat "/proc/$pid/winpid")" >/dev/null 2>&1
  else
    pkill -TERM -P "$pid" 2>/dev/null
    kill "$pid" 2>/dev/null
  fi
  for _ in $(seq 20); do alive "$pid" || break; sleep 0.25; done
  alive "$pid" && kill -9 "$pid" 2>/dev/null
done

mkdir -p "$EV"
for log in app.log idp.log migrate.log; do [ -f "$STATE/$log" ] && cp "$STATE/$log" "$EV/$log"; done
rm -rf "$STATE"

for p in ${APP_PORT:-} ${IDP_PORT:-}; do
  port_free "$p" || echo "down: WARNING port $p still answering" >&2
done
echo "down: stopped run $RUN_ID; evidence in $EV"
