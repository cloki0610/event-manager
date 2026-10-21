#!/usr/bin/env bash
# Read-only SQL against the run's database, recorded as evidence. Needs the sqlite3 CLI.
# SQLite runs only: the server holds DuckDB's single-writer lock, so read DuckDB state via the API.
# Usage: db.sh <run-id> <label> "<sql>"
set -uo pipefail
. "$(dirname "$0")/_common.sh"
load_run "$1"
label=$2 sql=$3
[ "$DB_KIND" = sqlite ] || { echo "db: $DB_KIND run; the server locks the file, verify stored state through the API instead" >&2; exit 2; }
command -v sqlite3 >/dev/null || { echo "db: sqlite3 CLI not on PATH" >&2; exit 2; }

file="$EV/db-$label.txt"
{ echo "sql> $sql"; sqlite3 -readonly -header "$DB_FILE" "$sql"; } | tee "$file"
