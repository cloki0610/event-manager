# Attendance

A signed-in guest can join an event while it has room, cannot join twice, can leave again, and appears in the event's public attendee list and in their own "my events" list.

## Sub-features

- `attend-join` adds the current guest to an event (201).
- `attend-full` refuses a join once `capacity` is reached (409 `event is full`).
- `attend-dup` refuses a second join by the same guest (409).
- `attend-leave` removes the guest (204), and a second leave is 404.
- `attend-list` shows attendees as `guest_id`, `name`, `joined_at`, with no emails.
- `attend-mine` lists the guest's joined events at `/guests/me/events`.
- `attend-auth` rejects join or leave without a session (401).

## How to get to it (user POV)

- `POST /events/{id}/join` and `POST /events/{id}/leave` with `Authorization: Bearer <token>` or the `session` cookie.
- `GET /events/{id}/attendees` (no auth).
- `GET /guests/me/events` (session).

## Driving it with verify scripts

Preconditions:

- A fresh instance with doctor passing.
- Two guests signed in: `A=$($S/login.sh "$R" alice "Alice Liddell")` and `B=$($S/login.sh "$R" bob Bob)`.

- **Create a one-seat event.** Run `$S/req.sh "$R" attend-create POST /events '{"title":"Tiny dinner","starts_at":"2030-06-01T18:00:00Z","capacity":1}'` and keep its `id` as `$ID`. Expect 201 with `attendee_count` 0.
- **Join.** Run `$S/req.sh "$R" attend-join POST /events/$ID/join - "$A"`. Expect 201 with `event_id`, `guest_id`, and `joined_at`.
- **Full.** Run `$S/req.sh "$R" attend-full POST /events/$ID/join - "$B"`. Expect 409 `{"error":"event is full"}`.
- **Duplicate.** On a second event with no capacity, have alice join twice. Run `$S/req.sh "$R" attend-dup-1 POST /events/$ID2/join - "$A"` and then `attend-dup-2` with the same command. Expect 201, then 409 `already joined this event`.
- **No session.** Run `$S/req.sh "$R" attend-auth POST /events/$ID/join`. Expect 401 `missing or invalid session`.
- **Cookie session.** Run `curl -s -b target/verify/$R/state/cookies-alice.txt -X POST http://127.0.0.1:<APP_PORT>/events/$ID2/leave -w '%{http_code}'`. Expect 204. Use the `BASE` value in `target/verify/$R/state/env`.
- **Attendee list.** Run `$S/req.sh "$R" attend-list GET /events/$ID/attendees`. Expect one entry named `Alice Liddell` and no `email` key.
- **My events.** Run `$S/req.sh "$R" attend-mine GET /guests/me/events - "$A"`. Expect `Tiny dinner` with `attendee_count` 1.
- **Stored state.** Run `$S/db.sh "$R" attend "SELECT a.event_id, g.name, g.subject FROM attendances a JOIN guests g ON g.id=a.guest_id"`.
- **Leave frees the seat.** Run `attend-leave POST /events/$ID/leave - "$A"` (expect 204), then `attend-leave-again` (expect 404 `attendance not found`), then `attend-rejoin-b POST /events/$ID/join - "$B"` (expect 201). Finally `GET /events/$ID` shows `attendee_count` 1.

## Gotchas

- The capacity check runs before the duplicate check. A guest who already joined a **full** event and joins again gets `event is full`, not `already joined this event`. Status is 409 either way. Use an uncapped or not-yet-full event to prove `attend-dup`.
- Join and leave on an unknown event id return 404 `event not found`. A malformed (non-UUID) id returns 404 from routing before any auth check.
- Deleting an event cascades to its attendances. Re-read `/guests/me/events` after a delete if a change touches cascades.
