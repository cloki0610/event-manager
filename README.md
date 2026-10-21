# event-manager

A small REST API for managing events and their guests, built with **actix-web** and **sqlx** (SQLite),
with optional **DuckDB** storage.

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

### DuckDB

DuckDB support is behind the `duckdb` cargo feature, because it compiles DuckDB from C++ source
(several minutes on the first build). Select it with a `duckdb://` URL:

```sh
DATABASE_URL=duckdb://events.duckdb cargo run --features duckdb
TEST_DATABASE_URL=duckdb://:memory: cargo test --features duckdb   # run the suite on DuckDB
```

The DuckDB schema (`src/db/duckdb_schema.sql`) is created at startup if missing. It has no
versioned migrations yet. DuckDB has no `ON DELETE CASCADE`, so deletes cascade in application
code. DuckDB allows only one process to open a database file for writing, so tools like the
`duckdb` CLI cannot open the file while the server is running. A build without the feature
refuses `duckdb://` URLs at startup.

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

## CI/CD

`.github/workflows/ci.yml` runs on every pull request: `cargo fmt --check`, clippy with warnings
denied, the test suite on SQLite and again on DuckDB, and a container image build.

`.github/workflows/deploy.yml` runs on every push to `main` (or by hand). It reruns CI, builds the
`Dockerfile`, pushes the image to Amazon ECR tagged with the commit SHA, and deploys
`deploy/aws/service.yml` with CloudFormation. GitHub authenticates to AWS through OpenID Connect,
so no AWS keys are stored in the repository.

### AWS layout

One Fargate task runs behind an Application Load Balancer. The SQLite database lives on an
encrypted EFS volume with AWS Backup enabled, so it survives redeploys. SQLite allows a single
writer, so the service runs exactly one task and a deploy stops the old task before starting the
new one. Expect a short outage on each deploy. A failed deploy rolls back automatically.

### One-time setup

1. Deploy the bootstrap stack. It creates the ECR repository, the role GitHub assumes, and the role
   CloudFormation uses to create the service. Skip the OIDC provider if the account already has one.

   ```sh
   aws cloudformation deploy --stack-name event-manager-bootstrap \
     --template-file deploy/aws/bootstrap.yml --capabilities CAPABILITY_IAM \
     --parameter-overrides GitHubRepo=<owner>/<repo>   # CreateOidcProvider=false if it exists
   ```

2. Store the OIDC client secret, if your provider issues one:
   `aws secretsmanager create-secret --name event-manager/oidc-client-secret --secret-string '<secret>'`.
3. In GitHub, create the `production` environment (Settings, Environments) and add these
   variables. Add required reviewers to the environment to gate releases.

   | Variable                     | Value |
   |------------------------------|-------|
   | `AWS_REGION`                 | e.g. `eu-west-1` |
   | `AWS_DEPLOY_ROLE_ARN`        | bootstrap output `DeployRoleArn` |
   | `AWS_CFN_EXECUTION_ROLE_ARN` | bootstrap output `CloudFormationExecutionRoleArn` |
   | `VPC_ID`                     | VPC to run in (the default VPC works) |
   | `SUBNET_IDS`                 | two public subnets in different AZs, comma separated |
   | `OIDC_ISSUER_URL`            | as in [Single sign-on](#single-sign-on) |
   | `OIDC_CLIENT_ID`             | as in [Single sign-on](#single-sign-on) |
   | `OIDC_CLIENT_SECRET_ARN`     | ARN from step 2, or empty |
   | `CERTIFICATE_ARN`            | ACM certificate for HTTPS, or empty for plain HTTP |
   | `PUBLIC_URL`                 | e.g. `https://events.example.com`, or empty to use the load balancer's address |
   | `POST_LOGIN_REDIRECT`        | optional |

4. Push to `main`. The first run creates the service stack. Its `RedirectUri` output is the URI to
   register with the OIDC provider. With `PUBLIC_URL` set, point that domain at the
   `LoadBalancerDnsName` output.

Use HTTPS (`CERTIFICATE_ARN` plus `PUBLIC_URL`) for anything real. Over plain HTTP the session
token travels in clear text.

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
