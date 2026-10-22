# SSO sign-in

Guests sign in through an OpenID Connect provider. A successful login creates the guest on first sight (keyed by issuer + subject), refreshes their name and email on later logins, and starts a session usable as a Bearer token or `session` cookie until logout or expiry.

## Sub-features

- `sso-login` runs `/auth/login` → provider → `/auth/callback` and returns `{guest, token, expires_at}`, sets the `session` cookie, and clears `oidc_state`.
- `sso-deny` handles provider-reported errors (consent denied) with 401 `SSO login failed`.
- `sso-replay` refuses a callback with a reused or mismatched `state` (401).
- `sso-relogin` keeps the same guest id for the same subject and updates the name.
- `sso-bearer-cookie` accepts the session from either the header or the cookie.
- `sso-logout` ends the session (204), and the token then gets 401.

## How to get to it (user POV)

- Browser: open `GET /auth/login`. The provider redirects back to `/auth/callback`.
- API: `POST /auth/logout` with the session.

## Driving it with verify scripts

Preconditions:

- A fresh instance with doctor passing.

- **Login.** Run `A=$($S/login.sh "$R" alice "Alice Liddell")`. Stderr shows `login alice -> HTTP 200`. `evidence/login-alice.json` has the guest name and `expires_at` about 7 days out. `target/verify/$R/state/cookies-alice.txt` has a `session` line.
- **Session works.** Run `$S/req.sh "$R" sso-me GET /guests/me - "$A"`. Expect 200 for `Alice Liddell`.
- **Denied consent.** Run `VERIFY_DENY=1 $S/login.sh "$R" mallory`. Expect HTTP 401 `{"error":"SSO login failed"}`. `app.log` (copied at teardown) has `SSO login failed: provider returned access_denied`. `$S/db.sh "$R" sso-deny "SELECT * FROM guests WHERE subject='mallory'"` returns no rows.
- **Replay.** Take `/auth/callback?code=...&state=...` from `login-alice` by hand: run `curl -s -o /dev/null -c jar -w '%{redirect_url}' $BASE/auth/login`, follow the provider URL with `curl -s -o /dev/null -w '%{redirect_url}'` to get the callback URL, call it once with `-b jar` (200), and then again (401). Calling it without `-b jar` is 401 (state cookie mismatch).
- **Re-login.** Run `$S/login.sh "$R" alice "Alice L."`. `evidence/login-alice.json` has the same `guest.id` as before and the new name.
- **Logout.** Run `$S/req.sh "$R" sso-logout POST /auth/logout - "$A"` (expect 204 with a `set-cookie: session=;` removal), then `sso-after-logout GET /guests/me - "$A"` (expect 401).

## Gotchas

- `login.sh` overwrites `evidence/login-<subject>.json` and the cookie jar on each call. Use a new subject, or copy the file, before logging the same subject in again if the earlier proof matters.
- The state cookie has path `/auth`, so a jar replayed against other paths will not carry it.
- Login failures return only `SSO login failed`. The cause is in `app.log` at WARN level.
- Not covered by the mock provider: real issuer discovery, client-secret auth, `Secure` cookies, `POST_LOGIN_REDIRECT`. See SKILL.md.
