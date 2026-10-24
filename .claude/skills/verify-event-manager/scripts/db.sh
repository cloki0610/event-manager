#!/usr/bin/env bash
# Read-only SQL against the run's local D1 database, recorded as evidence.
# Usage: db.sh <run-id> <label> "<sql>"
set -uo pipefail
. "$(dirname "$0")/_common.sh"
load_run "$1"
label=$2 sql=$3
case $(printf '%s' "$sql" | tr '[:lower:]' '[:upper:]') in
  SELECT*|WITH*) ;;
  *) echo "db: read-only, SELECT or WITH only" >&2; exit 2 ;;
esac

file="$EV/db-$label.txt"
{ echo "sql> $sql"
  node node_modules/wrangler/bin/wrangler.js d1 execute DB --local --persist-to "$PERSIST" \
    --command "$sql" 2>&1; } | tee "$file"
