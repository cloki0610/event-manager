# event-manager

A small REST API for managing events and their guests. It runs as a Rust **Cloudflare Worker**
(compiled to WebAssembly with `worker-build`) and stores its data in **Cloudflare D1** (SQLite).

## Layout

| Path            | What it is |
|-----------------|------------|
| `worker/`       | The Worker: routes, SSO, and every SQL statement (`worker/src/db.rs`) |
| `migrations/`   | D1 schema migrations, applied with `wrangler d1 migrations apply` |
| `e2e/`          | API test suite and a stand-in OIDC provider (`cargo run -p e2e --bin mock_idp`) |
| `wrangler.toml` | Worker and D1 binding configuration |

## Running locally

Requirements: Rust with the `wasm32-unknown-unknown` target, `cargo install worker-build`, Node 24.

```sh
npm ci                                            # installs the pinned wrangler
npx wrangler d1 migrations apply DB --local       # creates the local D1 database
npx wrangler dev                                  # http://127.0.0.1:8787
cargo test -p e2e                                 # builds the Worker, runs the API suite against wrangler dev
```

`wrangler dev` reads Worker variables from a `.dev.vars` file (git-ignored), one `KEY=value` per line.

On Windows, keep the checkout path short (under about 80 characters). The local D1 files nest
deep enough under `target/` to pass the 260-character path limit, and wrangler then fails with
an opaque "internal error".

### Single sign-on

Guests sign in through any OpenID Connect provider (Google, Microsoft Entra ID, Okta, Keycloak,
Auth0, ...). Register this service as a web application with the provider, using
`<your host>/auth/callback` as the redirect URI, then set these Worker variables:

| Variable              | Required | Description |
|-----------------------|----------|-------------|
| `OIDC_ISSUER_URL`     | yes      | Issuer, e.g. `https://accounts.google.com` or `https://login.microsoftonline.com/<tenant>/v2.0` |
| `OIDC_CLIENT_ID`      | yes      | Client ID from the provider |
| `OIDC_CLIENT_SECRET`  | no       | Client secret, stored as a Worker secret (omit for public clients) |
| `OIDC_REDIRECT_URL`   | yes      | e.g. `https://events.example.com/auth/callback`; must match the provider registration exactly |
| `POST_LOGIN_REDIRECT` | no       | Send the browser here after login instead of returning JSON |
| `SESSION_TTL_HOURS`   | no       | Session lifetime, default `168` (7 days) |

Each Worker isolate fetches the provider's discovery document and signing keys on its first SSO
request and keeps them until Cloudflare recycles the isolate. After the provider rotates its
keys, logins can fail until that happens. Redeploying forces it.

## CI/CD

`.github/workflows/ci.yml` runs on every pull request: `cargo fmt --check`, clippy with warnings
denied (the Worker is checked for `wasm32-unknown-unknown`), and the API suite against
`wrangler dev` with a local D1.

`.github/workflows/deploy.yml` runs on every push to `main` (or by hand). It reruns CI, applies
pending D1 migrations to the production database, deploys the Worker with `wrangler deploy`, and
smoke-tests `GET /events`. Migrations run before the new code ships, so each migration must keep
working with the code that is still live.

### One-time setup

1. Create the database and commit its id. The deploy job refuses to run while `wrangler.toml` still
   holds the all-zero placeholder.

   ```sh
   npx wrangler login
   npx wrangler d1 create event-manager   # copy database_id into wrangler.toml
   ```

2. Store the OIDC client secret, if your provider issues one:
   `npx wrangler secret put OIDC_CLIENT_SECRET`. It never passes through CI.
3. Create a Cloudflare API token with **Account > Workers Scripts > Edit** and **Account > D1 > Edit**.
4. In GitHub, create the `production` environment (Settings, Environments). Add required reviewers
   to it if you want to approve each release. Then add:

   | Name                    | Kind     | Value |
   |-------------------------|----------|-------|
   | `CLOUDFLARE_API_TOKEN`  | secret   | token from step 3 |
   | `CLOUDFLARE_ACCOUNT_ID` | secret   | from the Cloudflare dashboard |
   | `PUBLIC_URL`            | variable | e.g. `https://event-manager.<subdomain>.workers.dev` or your custom domain |
   | `OIDC_ISSUER_URL`       | variable | as in [Single sign-on](#single-sign-on) |
   | `OIDC_CLIENT_ID`        | variable | as in [Single sign-on](#single-sign-on) |
   | `POST_LOGIN_REDIRECT`   | variable | optional |
   | `SESSION_TTL_HOURS`     | variable | optional |

   `OIDC_REDIRECT_URL` is derived as `<PUBLIC_URL>/auth/callback`. Register that with the provider.

5. Push to `main`.

### Cost

The Workers Free plan covers 100,000 requests a day. D1's free tier covers 5 GB of storage,
5 million rows read and 100,000 rows written a day. A low-traffic deployment should cost
nothing. These were the limits at the time of writing, so check Cloudflare's pricing pages.
Past them, the Workers Paid plan starts at $5 a month.

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
curl -X POST localhost:8787/events -H 'content-type: application/json' \
  -d '{"title":"Launch party","starts_at":"2030-06-01T18:00:00Z","capacity":50}'

# Sign in: open http://127.0.0.1:8787/auth/login in a browser; the callback returns the token.
curl -X POST localhost:8787/events/<EVENT_ID>/join -H 'Authorization: Bearer <TOKEN>'
```
