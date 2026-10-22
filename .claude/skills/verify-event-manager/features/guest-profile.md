# Guest profile

A signed-in guest can read their own profile (`id`, `name`, `email`, `created_at`), which mirrors the provider's claims as of their latest login.

## Sub-features

- `me-view` returns the current guest from `GET /guests/me`.
- `me-claims` falls back through `name` → `preferred_username` → `email` → `sub`, and refreshes it on each login.
- `me-auth` returns 401 without a session or with an expired or logged-out session.

## How to get to it (user POV)

- `GET /guests/me` with a Bearer token or the `session` cookie.

## Driving it with verify scripts

Preconditions:

- A fresh instance with doctor passing.

- **View.** Run `A=$($S/login.sh "$R" carol "Carol Danvers" carol@corp.test)`, then `$S/req.sh "$R" me-view GET /guests/me - "$A"`. Expect name `Carol Danvers` and email `carol@corp.test`.
- **Refresh.** Run `A2=$($S/login.sh "$R" carol "Captain Marvel" cm@corp.test)`, then `me-refresh GET /guests/me - "$A2"`. Expect the same `id` with the new name and email. The old token `$A` still works, because sessions are independent.
- **No session.** Run `$S/req.sh "$R" me-anon GET /guests/me`. Expect 401.
- **Bad token.** Run `$S/req.sh "$R" me-bad GET /guests/me - deadbeef`. Expect 401.
- **Stored.** Run `$S/db.sh "$R" me "SELECT id, issuer, subject, name, email FROM guests WHERE subject='carol'"`. Expect one row.

## Gotchas

- `login.sh` always sends a `name`, so the `preferred_username`/`email`/`sub` fallbacks cannot be reached through it. Proving them needs the mock provider to omit `name`, which it does not support yet. Report those sub-features as unverified rather than faking a guest row.
