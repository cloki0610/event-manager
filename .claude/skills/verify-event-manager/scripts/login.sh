#!/usr/bin/env bash
# Signs a guest in through the real browser flow: /auth/login -> provider /authorize -> /auth/callback.
# Usage: login.sh <run-id> <subject> [name] [email]
#        VERIFY_DENY=1 login.sh ...   makes the provider refuse consent (expects 401).
# Prints the session token. Saves the callback body to evidence/login-<subject>.json and the
# browser cookie jar (holding the `session` cookie) to state/cookies-<subject>.txt.
set -uo pipefail
. "$(dirname "$0")/_common.sh"
load_run "$1"
sub=${2:?subject required}
name=${3:-$sub}
email=${4:-$sub@example.com}
jar="$STATE/cookies-$sub.txt"
rm -f "$jar"

authorize=$(curl -s -o /dev/null -c "$jar" -w '%{redirect_url}' "$BASE/auth/login")
case $authorize in "$IDP/authorize?"*) ;; *) echo "login: /auth/login did not redirect to $IDP/authorize (got '$authorize')" >&2; exit 1 ;; esac

# Appending identity params stands in for the user typing credentials at the provider.
authorize+="&sub=$(urlencode "$sub")&name=$(urlencode "$name")&email=$(urlencode "$email")"
[ "${VERIFY_DENY:-}" = 1 ] && authorize+="&deny=1"

out="$EV/login-$sub.json"
code=$(curl -s -L -b "$jar" -c "$jar" -o "$out" -w '%{http_code}' "$authorize")
echo "login $sub -> HTTP $code" >&2
[ "$code" = 200 ] || { cat "$out" >&2; echo >&2; exit 1; }
sed -n 's/.*"token":"\([0-9a-f]*\)".*/\1/p' "$out"
