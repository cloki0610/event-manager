# event-manager

A small REST API for managing events and their guests, built with **actix-web** and **sqlx** (SQLite).

## Running

```sh
cargo run                       # http://127.0.0.1:8080, database in ./events.db
cargo test
```

| Env var        | Default              |
|----------------|----------------------|
| `DATABASE_URL` | `sqlite://events.db` |
| `BIND_ADDR`    | `127.0.0.1:8080`     |
| `RUST_LOG`     | `info`               |

Migrations in `migrations/` are embedded and applied at startup.

### Single sign-on

Guests sign in through any OpenID Connect provider (Google, Microsoft Entra ID, Okta, Keycloak,
Auth0, ...). Register this service as a web application with the provider, using
`<your host>/auth/callback` as the redirect URI, then set:

| Env var               | Required | Description |
|-----------------------|----------|-------------|
| `OIDC_ISSUER_URL`     | yes      | Issuer, e.g. `https://accounts.google.com` or `https://login.microsoftonline.com/<tenant>/v2.0` |
| `OIDC_CLIENT_ID`      | yes      | Client ID from the provider |
| `OIDC_CLIENT_SECRET`  | no       | Client secret (omit for public clients) |
| `OIDC_REDIRECT_URL`   | yes      | e.g. `http://127.0.0.1:8080/auth/callback`; must match the provider registration exactly |
| `POST_LOGIN_REDIRECT` | no       | Send the browser here after login instead of returning JSON |
| `SESSION_TTL_HOURS`   | no       | Session lifetime, default `168` (7 days) |

The provider's discovery document and signing keys are fetched once at startup; restart the
service if the provider rotates its keys.

> **Windows note:** sqlx compiles SQLite from C source, so the toolchain needs a working C compiler.
> If the GNU toolchain's gcc is broken, use the MSVC toolchain instead:
> `cargo +stable-x86_64-pc-windows-msvc run`.

## API

All bodies are JSON. Errors look like `{"error": "..."}`. Timestamps are RFC 3339.

### Events

| Method   | Path                      | Description |
|----------|---------------------------|-------------|
| `GET`    | `/events`                 | List events by start time. Query: `limit` (1–100, default 50), `offset`, `upcoming=true` |
| `POST`   | `/events`                 | Create: `title`, `starts_at` required; `description`, `location`, `ends_at`, `capacity` optional |
| `GET`    | `/events/{id}`            | View one event (includes `attendee_count`) |
| `PATCH`  | `/events/{id}`            | Partial update. Omitted fields are kept; `null` clears a nullable field |
| `DELETE` | `/events/{id}`            | Delete the event and its attendances |
| `GET`    | `/events/{id}/attendees`  | Guests attending (name + join time, no emails) |

### Authentication

Login uses the OpenID Connect authorization-code flow with PKCE. Guests are identified by the
provider's `(issuer, subject)` pair and created on first login; their name and email are
refreshed on each login. A successful login starts an API session whose token is accepted either
as `Authorization: Bearer <token>` or via the `session` cookie (HttpOnly, SameSite=Lax).

| Method | Path              | Auth    | Description |
|--------|-------------------|---------|-------------|
| `GET`  | `/auth/login`     | –       | Redirects the browser to the SSO provider |
| `GET`  | `/auth/callback`  | –       | Provider redirects here; returns `{ "guest", "token", "expires_at" }` and sets the `session` cookie |
| `POST` | `/auth/logout`    | Session | Ends the current session |

### Guests

| Method | Path                  | Auth    | Description |
|--------|-----------------------|---------|-------------|
| `GET`  | `/guests/me`          | Session | The current guest |
| `GET`  | `/guests/me/events`   | Session | Events the guest has joined |
| `POST` | `/events/{id}/join`   | Session | Join an event (`409` if already joined or full) |
| `POST` | `/events/{id}/leave`  | Session | Leave an event (`404` if not attending) |

### Example

```sh
curl -X POST localhost:8080/events -H 'content-type: application/json' \
  -d '{"title":"Launch party","starts_at":"2030-06-01T18:00:00Z","capacity":50}'

# Sign in: open http://127.0.0.1:8080/auth/login in a browser; the callback returns the token.
curl -X POST localhost:8080/events/<EVENT_ID>/join -H 'Authorization: Bearer <TOKEN>'
```
