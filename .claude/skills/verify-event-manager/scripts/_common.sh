# Sourced by the other scripts. Resolves the repo root and, given a run id, the run's paths.
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)
cd "$ROOT"

# load_run <run-id>: sets RUN_ID, RUN (repo-relative), STATE, EV and the values up.sh recorded.
load_run() {
  RUN_ID=${1:?run id required (printed by up.sh)}
  RUN="target/verify/$RUN_ID"
  STATE="$RUN/state"
  EV="$RUN/evidence"
  [ -f "$STATE/env" ] || { echo "no live run '$RUN_ID' ($STATE/env missing)" >&2; exit 2; }
  # shellcheck disable=SC1091
  . "$STATE/env"
}

# urlencode <string>
urlencode() {
  local s=$1 out= c i
  for ((i = 0; i < ${#s}; i++)); do
    c=${s:i:1}
    case $c in
      [A-Za-z0-9._~-]) out+=$c ;;
      *) out+=$(printf '%%%02X' "'$c") ;;
    esac
  done
  printf '%s' "$out"
}

# alive <pid>
alive() { kill -0 "$1" 2>/dev/null; }

# port_free <port>: nothing listens on <port>. Asks the OS rather than probing, because a
# refused connection to localhost takes ~2s on Windows.
port_free() {
  if command -v ss >/dev/null; then
    [ -z "$(ss -ltnH "sport = :$1")" ]
  elif command -v netstat >/dev/null; then
    ! netstat -an | tr -d '\r' | grep -E "LISTEN" | grep -qE "[.:]$1[[:space:]]"
  else
    curl -s -o /dev/null --max-time 5 "http://127.0.0.1:$1/"
    [ $? -eq 7 ]
  fi
}
