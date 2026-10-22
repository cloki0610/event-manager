---
name: verify-event-manager
description: Launch an isolated event-manager REST API (actix-web, SQLite or DuckDB storage) with a mock OpenID Connect provider, sign guests in through the real SSO flow, drive events/attendance endpoints with curl, and capture HTTP + database evidence. Use to prove a change to event-manager works end to end, not just in `cargo test`.
---

# Verify event-manager

event-manager is a JSON REST API; its users are HTTP clients. Guests sign in through an OIDC provider, so this skill runs a stand-in provider (`examples/mock_idp.rs`) next to the real binary. Everything else is the production code path.

All commands run from the repo root in bash (Git Bash on Windows). Helpers live in `.claude/skills/verify-event-manager/scripts/`; below, `S=.claude/skills/verify-event-manager/scripts`.

Requirements on PATH: `cargo`, `curl`, `openssl`. `sqlite3` is optional (only `db.sh` needs it).

## Launch

```sh
S=.claude/skills/verify-event-manager/scripts
R=$($S/up.sh)            # or: $S/up.sh my-run-id
```

`up.sh` builds `event-manager` and the `mock_idp` example, generates a throwaway RSA key, starts the mock provider, then starts the app on a fresh SQLite file with `OIDC_*` pointed at the provider. Migrations run at startup. It returns only when the app logged `listening on` **and** `GET /events` returns 200, and prints the run id as its last stdout line. Progress goes to stderr. On any failure it prints the log and tears itself down.

- Ports: app on `VERIFY_APP_PORT` (default 18080), provider on the next free port. Busy ports are skipped, so several runs can coexist. The app's own default (`127.0.0.1:8080`, `./events.db`) is never used. Never drive a server you did not start with `up.sh`.
- Run layout: `target/verify/<run-id>/state/` (db, key, pids, cookie jars, logs) and `target/verify/<run-id>/evidence/`.
- First build takes about 2 minutes. Wrap calls in a timeout so a hang can't stall you (`timeout 300 $S/up.sh`).
- If the GNU toolchain's gcc is broken (see README), set `VERIFY_TOOLCHAIN=+stable-x86_64-pc-windows-msvc`.
- Storage backend: `VERIFY_DB=duckdb $S/up.sh` builds with `--features duckdb` and serves from `state/events.duckdb`. The default is SQLite. When a change touches `src/db/`, run the relevant feature recipes once per backend. DuckDB's first build compiles C++ for several minutes, so use `timeout 900`.

## Doctor

```sh
$S/doctor.sh "$R"        # exit 0 = worth driving
```

Read-only. Checks that both processes are alive, `GET /events` is 200, the provider's issuer matches, the DB file exists, the binary is newer than `src/`, `migrations/` and `Cargo.toml` (otherwise rerun `up.sh`), and that the app port is owned by the app process we started (netstat winpid on Windows, `ss` on Linux). Run it first and again whenever a response surprises you.

## Drive

Signing in uses the browser flow a real user takes: `GET /auth/login` sets the `oidc_state` cookie and redirects to the provider. The provider "consents" right away and redirects to `/auth/callback`, which verifies the ID token (PKCE, nonce, signature) and returns the session.

```sh
TOKEN=$($S/login.sh "$R" alice "Alice Liddell" [alice@example.com])  # prints the session token
VERIFY_DENY=1 $S/login.sh "$R" mallory                              # provider refuses -> 401
```

Each subject is a stable identity (`issuer`, `sub`), so logging in as `alice` again reaches the same guest. The cookie jar with the `session` cookie is saved at `target/verify/$R/state/cookies-alice.txt` for cookie-based requests (`curl -b <jar>`).

Make every API call through `req.sh`, which records it:

```sh
$S/req.sh "$R" <label> <METHOD> <path> [json-body|-] [token]
$S/req.sh "$R" create POST /events '{"title":"Launch","starts_at":"2030-06-01T18:00:00Z","capacity":2}'
$S/req.sh "$R" join   POST /events/$ID/join - "$TOKEN"
```

It prints the response body and reports `HTTP <status>  (<evidence file>)` on stderr. It exits 0 for any HTTP status, so check the status yourself. To get an id from a body without jq: `sed -n 's/.*"id":"\([^"]*\)".*/\1/p'`.

Look at stored state read-only (SQLite runs only):

```sh
$S/db.sh "$R" <label> "SELECT ... FROM events|guests|sessions|attendances|login_states"
```

On a DuckDB run, `db.sh` refuses, because the server holds DuckDB's single-writer lock on the file. Prove stored state through a second API view instead (`GET /events/{id}`, `/attendees`, `/guests/me/events`).

Per-feature recipes are in [`features/README.md`](features/README.md). Use them, and drive every entry point a feature file lists.

## Evidence

Everything goes to `target/verify/<run-id>/evidence/`:

- `NN-<label>.txt`: request line and body, status, response headers, response body, numbered in order.
- `login-<subject>.json`: the callback response (guest, token, expires_at).
- `db-<label>.txt`: SQL and result.
- `app.log`, `idp.log`: copied in at teardown.

Proof standards:

- Use the real user path: public endpoints and the real SSO redirect chain. Never insert rows into `guests`/`sessions` to fake a login, and never call handlers directly.
- Capture the action and the resulting state. After a mutation, read it back through a second view (`GET /events/{id}`, `/attendees`, `/guests/me/events`) and, for anything stored, with `db.sh`.
- Prove the negative paths the change touches (401 without a session, 409, 404, 400 validation) as well as the happy path.
- The only mock is the identity provider, which already sits behind a network boundary (discovery + JWKS + token endpoint). Everything the app does with the token is real.

## Cleanup

```sh
$S/down.sh "$R"
```

Kills only the two PIDs this run started, copies the logs into `evidence/`, deletes `state/` (DB, key, cookie jars), and warns if a port is still listening. Evidence stays at `target/verify/<run-id>/evidence/`. `cargo clean` deletes it, so copy anything you want to keep long-term. Run `down.sh` after failed attempts too. If a launch was interrupted, look for a leftover `up.sh` with `ps` and kill that PID, never by process name.

## What the stand-in does not cover

The mock provider stands in for Google/Entra/Okta and friends. It does not verify:

- discovery against a real issuer, or redirect URI registration,
- confidential-client authentication (runs are public clients: no `OIDC_CLIENT_SECRET`),
- `Secure` cookies (needs an `https://` redirect URL),
- `POST_LOGIN_REDIRECT` (unset here, so the callback returns JSON).

Changes in those areas need a run against a real provider.
