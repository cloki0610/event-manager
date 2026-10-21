# Events CRUD

Anyone can create, view, partially update, and delete an event. Updates keep omitted fields and clear nullable fields sent as `null`. Invalid input is rejected with 400 and a JSON error.

## Sub-features

- `ev-create` creates an event with `title` and `starts_at`, plus optional `description`, `location`, `ends_at`, and `capacity` (201).
- `ev-view` returns one event with `attendee_count`.
- `ev-patch` changes only the fields sent, and `null` clears `description`, `location`, `ends_at`, or `capacity`.
- `ev-delete` removes the event (204). After that, GET and DELETE return 404.
- `ev-validate` rejects a blank title, `ends_at` before `starts_at`, `capacity` < 1, a bad timestamp, or a missing title (400).

## How to get to it (user POV)

- `POST /events`, `GET /events/{id}`, `PATCH /events/{id}`, `DELETE /events/{id}` with JSON bodies.

## Driving it with verify scripts

Preconditions:

- A fresh instance with doctor passing.

- **Create.** Run `$S/req.sh "$R" ev-create POST /events '{"title":"Rust meetup","description":"Talks and pizza","location":"Hall A","starts_at":"2030-01-10T18:00:00Z","ends_at":"2030-01-10T21:00:00Z","capacity":30}'` and keep `$ID`. Expect 201 with `attendee_count` 0.
- **View.** Run `$S/req.sh "$R" ev-view GET /events/$ID`. Expect the same fields.
- **Patch.** Run `$S/req.sh "$R" ev-patch PATCH /events/$ID '{"title":"Rust meetup #2","description":null}'`. Expect 200 with the new title, `description` null, and `capacity` 30, `location` `Hall A`, and `starts_at` unchanged. `updated_at` is later than `created_at`.
- **Persisted.** Run `$S/db.sh "$R" ev-row "SELECT title, description, location, capacity FROM events WHERE id='$ID'"`.
- **Validate.** For each body (`{"title":"  ","starts_at":"2030-01-01T00:00:00Z"}`, `{"title":"x","starts_at":"2030-01-02T00:00:00Z","ends_at":"2030-01-01T00:00:00Z"}`, `{"title":"x","starts_at":"2030-01-01T00:00:00Z","capacity":0}`, `{"title":"x","starts_at":"not a date"}`, `{"starts_at":"2030-01-01T00:00:00Z"}`), run `$S/req.sh "$R" ev-bad-N POST /events '<body>'`. Expect 400 with an `error` string. Also run PATCH with `'{"ends_at":"2000-01-01T00:00:00Z"}'` and expect 400.
- **Delete.** Run `ev-delete DELETE /events/$ID` (expect 204), then `ev-view-gone GET /events/$ID` (expect 404 `event not found`), then `ev-delete-again` (expect 404).

## Gotchas

- Event creation needs no session by design. Do not add a token and then claim the auth path is verified.
- Timestamps come back in RFC 3339 with sub-second precision for server-set fields. Compare `starts_at` as sent.
- A non-UUID id returns 404, not 400.
