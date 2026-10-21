#!/usr/bin/env bash
# Sends one API request and records it as evidence.
# Usage: req.sh <run-id> <label> <METHOD> <path> [json-body|-] [token]
#   json-body "-" means no body. token adds `Authorization: Bearer <token>`.
# Prints the response body; writes evidence/<NN>-<label>.txt (request, status, headers, body).
# Exit code 0 regardless of HTTP status; the status is the first stderr line.
set -uo pipefail
. "$(dirname "$0")/_common.sh"
load_run "$1"
label=$2 method=$3 path=$4 body=${5:--} token=${6:-}

n=$(printf '%02d' $(( $(find "$EV" -name '[0-9][0-9]-*.txt' | wc -l) + 1 )))
file="$EV/$n-$label.txt"
args=(-s -X "$method" -D "$STATE/headers" -o "$STATE/body" -w '%{http_code}')
[ "$body" != - ] && args+=(-H 'content-type: application/json' --data-binary "$body")
[ -n "$token" ] && args+=(-H "Authorization: Bearer $token")

status=$(curl "${args[@]}" "$BASE$path")
{
  echo "> $method $path${token:+  (Bearer ${token:0:8}...)}"
  [ "$body" != - ] && echo "> $body"
  echo "< HTTP $status"
  tr -d '\r' <"$STATE/headers" | sed -n '2,$p' | sed '/^$/d; s/^/< /'
  echo
  cat "$STATE/body"
  echo
} >"$file"
echo "HTTP $status  ($file)" >&2
cat "$STATE/body"
echo
