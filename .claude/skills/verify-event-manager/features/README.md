# event-manager verification map

This directory is the maintained source for verifying the user-facing behavior of event-manager. The "user" is an HTTP client, with an SSO-signed-in guest for the session endpoints. Read this index before driving the API, then use the matching feature file as the recipe.

## Baseline preconditions

- Start a fresh instance with `R=$($S/up.sh)`, where `S=.claude/skills/verify-event-manager/scripts`. It gets its own ports and an empty SQLite database, with migrations applied.
- `$S/doctor.sh "$R"` exits 0.
- No seed data: every recipe creates the events and guests it needs.
- Never drive an instance that was not started by this verification run, including a developer's own `cargo run` on port 8080.

## Driving conventions

- Make every request through `$S/req.sh "$R" <label> <METHOD> <path> [body|-] [token]` so it is recorded.
- Get sessions only with `$S/login.sh "$R" <subject> [name]`, which runs the real `/auth/login` → provider → `/auth/callback` chain.
- Use timestamps far in the future (`2030-…`) unless a recipe needs a past one, so `upcoming=true` is predictable.
- Treat commands as literal. Keep the quoted JSON bodies unchanged.

## Proof and skip reporting

- Proof is the numbered `evidence/NN-<label>.txt` files: request, status, headers, and body.
- Read every mutation back through a second view (a GET, and `db.sh` for stored rows).
- Record the feature ID with each label, e.g. `attend-full-...`.
- Report an unreachable path with the attempted command and the unmet precondition. Do not count a skipped entry point as verified through another path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior, then exactly four H2 sections in this order: `Sub-features`, `How to get to it (user POV)`, `Driving it with verify scripts`, `Gotchas`.

## Features

- [Events CRUD](./events-crud.md) covers create, view, partial update (including clearing with `null`), delete, and validation errors.
- [Browse events](./events-list.md) covers listing order, pagination, `upcoming=true`, and query validation.
- [SSO sign-in](./sso-login.md) covers login through the provider, denied consent, Bearer vs cookie sessions, re-login identity, and logout.
- [Attendance](./attendance.md) covers join, capacity limits, duplicate join, leave, the attendee list, and "my events".
- [Guest profile](./guest-profile.md) covers `/guests/me` and how the profile follows provider claims.
