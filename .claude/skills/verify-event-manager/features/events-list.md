# Browse events

`GET /events` lists events ordered by start time, with `limit`/`offset` paging and an `upcoming=true` filter that hides events that have already started.

## Sub-features

- `list-order` sorts by `starts_at`, then `id`.
- `list-page` uses `limit` (clamped to 1–100, default 50) and `offset`.
- `list-upcoming` with `upcoming=true` drops events whose `starts_at` is in the past.
- `list-badquery` returns 400 JSON for a non-numeric `limit`.

## How to get to it (user POV)

- `GET /events[?limit=&offset=&upcoming=true]`.

## Driving it with verify scripts

Preconditions:

- A fresh instance with doctor passing.

- **Seed three events.** Create `Later` at `2031-01-01T00:00:00Z`, `Sooner` at `2030-01-01T00:00:00Z`, and `Past` at `2001-01-01T00:00:00Z` with `$S/req.sh "$R" list-seed-N POST /events '{"title":"...","starts_at":"..."}'`.
- **Order.** Run `$S/req.sh "$R" list-all GET /events`. Expect `Past`, `Sooner`, `Later` in that order.
- **Page.** Run `$S/req.sh "$R" list-page GET '/events?limit=1&offset=1'`. Expect exactly `[Sooner]`.
- **Clamp.** Run `$S/req.sh "$R" list-clamp GET '/events?limit=0'`. Expect one event (clamped to 1).
- **Upcoming.** Run `$S/req.sh "$R" list-upcoming GET '/events?upcoming=true'`. Expect `Sooner` and `Later` only.
- **Bad query.** Run `$S/req.sh "$R" list-bad GET '/events?limit=abc'`. Expect 400 with an `error` string.

## Gotchas

- Quote paths that contain `?` and `&` in the shell.
- Each list item includes `attendee_count`. A change to the counting subquery shows up here as well as in `GET /events/{id}`.
