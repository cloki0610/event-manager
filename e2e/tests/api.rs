//! Drives the real Worker under `wrangler dev --local` over HTTP. Each test gets its own Worker
//! process, its own freshly migrated D1 database and its own mock OIDC provider.

use std::collections::HashMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use cookie::Cookie;
use e2e::{Grant, MockIdp};
use openidconnect::url::Url;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde_json::{Value, json};

const CLIENT_ID: &str = "event-manager";
const CLIENT_SECRET: &str = "secret";
const IDP_KEY: &str = include_str!("fixtures/test_idp_key.pem");
const READY_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Deserialize)]
struct Event {
    id: String,
    title: String,
    description: Option<String>,
    starts_at: String,
    capacity: Option<i64>,
    attendee_count: i64,
}

#[derive(Debug, Deserialize)]
struct Guest {
    id: String,
    name: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    guest: Guest,
    token: String,
}

#[derive(Debug, Deserialize)]
struct Attendee {
    name: String,
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn scratch() -> PathBuf {
    root().join("target").join("e2e")
}

/// Runs the repo's pinned wrangler (`npm ci` installs it).
fn wrangler(args: &[&str]) -> Command {
    let mut cmd = Command::new("node");
    cmd.arg(root().join("node_modules/wrangler/bin/wrangler.js"))
        .args(args)
        .current_dir(root())
        .env("WRANGLER_SEND_METRICS", "false")
        .stdin(Stdio::null());
    cmd
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "{what} failed ({})\n{}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// Shared by every test in the run: the dev config and a migrated database to copy.
struct Prepared {
    config: PathBuf,
    template: PathBuf,
}

fn prepared() -> &'static Prepared {
    static PREPARED: OnceLock<Result<Prepared, String>> = OnceLock::new();
    PREPARED
        .get_or_init(prepare)
        .as_ref()
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Builds the Worker once, the way `wrangler deploy` does, and migrates a template database.
fn prepare() -> Result<Prepared, String> {
    let dir = scratch();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut build = wrangler(&["deploy", "--dry-run", "--outdir"]);
    build.arg(dir.join("bundle"));
    run(build, "building the worker")?;

    let config = dir.join("wrangler.toml");
    fs::write(&config, dev_config()?).map_err(|e| e.to_string())?;

    let template = dir.join("template");
    let mut migrate = wrangler(&["d1", "migrations", "apply", "DB", "--local", "--config"]);
    migrate.arg(&config).arg("--persist-to").arg(&template);
    run(migrate, "applying migrations")?;
    Ok(Prepared { config, template })
}

/// The repo's wrangler.toml without its build step, so `wrangler dev` serves the bundle built
/// once by [`prepare`] instead of rebuilding per test. Paths become absolute because the copy
/// lives elsewhere.
fn dev_config() -> Result<String, String> {
    let text = fs::read_to_string(root().join("wrangler.toml")).map_err(|e| e.to_string())?;
    let mut config: toml::Table = text.parse().map_err(|e| format!("wrangler.toml: {e}"))?;
    let absolute = |v: &toml::Value| {
        let path = v.as_str().expect("path is a string");
        toml::Value::String(root().join(path).to_string_lossy().into_owned())
    };
    config.remove("build");
    let main = absolute(&config["main"]);
    config.insert("main".into(), main);
    for db in config["d1_databases"].as_array_mut().expect("d1_databases") {
        let db = db.as_table_mut().expect("d1 database table");
        let migrations = absolute(&db["migrations_dir"]);
        db.insert("migrations_dir".into(), migrations);
    }
    toml::to_string(&config).map_err(|e| e.to_string())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A `wrangler dev` process serving the Worker on a private copy of the template database.
struct Worker {
    base: String,
    child: Child,
    dir: PathBuf,
}

impl Worker {
    fn start(idp: &MockIdp) -> Self {
        let prepared = prepared();
        let dir = scratch().join(uuid::Uuid::new_v4().simple().to_string());
        let state = dir.join("state");
        copy_dir(&prepared.template, &state);
        let port = free_port();
        let base = format!("http://127.0.0.1:{port}");
        let log = fs::File::create(dir.join("wrangler.log")).unwrap();

        let mut cmd = wrangler(&["dev", "--local", "--ip", "127.0.0.1"]);
        cmd.arg("--show-interactive-dev-session=false")
            .arg("--config")
            .arg(&prepared.config)
            .arg("--persist-to")
            .arg(&state)
            .args(["--port", &port.to_string()])
            .args(["--inspector-port", &free_port().to_string()]);
        for var in [
            format!("OIDC_ISSUER_URL:{}", idp.issuer()),
            format!("OIDC_CLIENT_ID:{CLIENT_ID}"),
            format!("OIDC_CLIENT_SECRET:{CLIENT_SECRET}"),
            format!("OIDC_REDIRECT_URL:{base}/auth/callback"),
            "SESSION_TTL_HOURS:1".into(),
        ] {
            cmd.arg("--var").arg(var);
        }
        cmd.stdout(log.try_clone().unwrap()).stderr(log);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let child = cmd.spawn().expect("failed to start wrangler dev");
        let mut worker = Worker { base, child, dir };
        worker.wait_ready();
        worker
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("wrangler.log")).unwrap_or_default()
    }

    fn wait_ready(&mut self) {
        let probe = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("wrangler dev exited with {status}\n{}", self.log());
            }
            let ready = probe
                .get(format!("{}/events", self.base))
                .send()
                .is_ok_and(|r| r.status() == 200);
            if ready {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!(
            "wrangler dev not ready after {READY_TIMEOUT:?}\n{}",
            self.log()
        );
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("--- wrangler dev log ---\n{}", self.log());
        }
        // wrangler runs workerd as a child, so the whole tree has to go.
        let pid = self.child.id().to_string();
        #[cfg(windows)]
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .output();
        #[cfg(unix)]
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .output();
        let _ = self.child.wait();
        // Grandchildren can hold the database files open for a moment after the kill.
        for _ in 0..20 {
            if fs::remove_dir_all(&self.dir).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

struct App {
    http: Client,
    idp: Arc<MockIdp>,
    worker: Worker,
}

fn setup() -> App {
    let idp = MockIdp::start("127.0.0.1:0", CLIENT_ID, Some(CLIENT_SECRET), IDP_KEY).unwrap();
    let worker = Worker::start(&idp);
    let http = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    App { http, idp, worker }
}

impl App {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.worker.base)
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http.get(self.url(path))
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.http.post(self.url(path))
    }

    fn patch(&self, path: &str) -> RequestBuilder {
        self.http.patch(self.url(path))
    }

    fn delete(&self, path: &str) -> RequestBuilder {
        self.http.delete(self.url(path))
    }
}

fn send(req: RequestBuilder) -> Response {
    req.send().unwrap()
}

fn status(req: RequestBuilder) -> u16 {
    send(req).status().as_u16()
}

fn read<T: for<'de> Deserialize<'de>>(req: RequestBuilder) -> T {
    let resp = send(req);
    assert!(resp.status().is_success(), "status {}", resp.status());
    resp.json().unwrap()
}

fn set_cookie(resp: &Response, name: &str) -> Option<Cookie<'static>> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| Cookie::parse(v.to_str().ok()?.to_string()).ok())
        .find(|c| c.name() == name)
}

fn with_cookie(req: RequestBuilder, cookie: &Cookie) -> RequestBuilder {
    req.header("cookie", cookie.stripped().to_string())
}

fn bearer(req: RequestBuilder, token: &str) -> RequestBuilder {
    req.header("authorization", format!("Bearer {token}"))
}

/// A login started at `/auth/login`, paused at the provider's consent screen.
struct PendingLogin {
    state: String,
    state_cookie: Cookie<'static>,
}

fn start_login(app: &App, code: &str, subject: &str, name: &str) -> PendingLogin {
    let resp = send(app.get("/auth/login"));
    assert_eq!(resp.status(), 302);
    let state_cookie = set_cookie(&resp, "oidc_state").expect("state cookie");

    let location = resp.headers()["location"].to_str().unwrap();
    let url = Url::parse(location).unwrap();
    assert!(location.starts_with(&format!("{}/authorize?", app.idp.issuer())));
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], app.url("/auth/callback"));
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].split(' ').any(|s| s == "openid"));

    app.idp.grant(
        code,
        Grant {
            subject: subject.into(),
            name: name.into(),
            email: format!("{subject}@example.com"),
            nonce: q["nonce"].clone(),
            code_challenge: q["code_challenge"].clone(),
        },
    );
    PendingLogin {
        state: q["state"].clone(),
        state_cookie,
    }
}

fn finish_login(app: &App, login: &PendingLogin, code: &str) -> Response {
    let path = format!("/auth/callback?code={code}&state={}", login.state);
    send(with_cookie(app.get(&path), &login.state_cookie))
}

/// Runs the whole SSO login for `subject` and returns the session.
fn login(app: &App, subject: &str, name: &str) -> LoginResponse {
    let code = format!("code-{subject}-{}", uuid::Uuid::new_v4());
    let pending = start_login(app, &code, subject, name);
    let resp = finish_login(app, &pending, &code);
    assert_eq!(resp.status(), 200);
    resp.json().unwrap()
}

fn create_event(app: &App, body: Value) -> Event {
    let resp = send(app.post("/events").json(&body));
    assert_eq!(resp.status(), 201);
    let event: Event = resp.json().unwrap();
    event
}

#[test]
fn event_crud() {
    let app = setup();

    let resp = send(app.post("/events").json(&json!({
        "title": "Rust meetup",
        "description": "Talks and pizza",
        "starts_at": "2030-01-10T18:00:00Z",
        "ends_at": "2030-01-10T21:00:00Z",
        "capacity": 30
    })));
    assert_eq!(resp.status(), 201);
    let location = resp.headers()["location"].to_str().unwrap().to_string();
    let event: Event = resp.json().unwrap();
    assert_eq!(location, format!("/events/{}", event.id));
    assert_eq!(event.title, "Rust meetup");
    assert_eq!(event.attendee_count, 0);

    let fetched: Event = read(app.get(&format!("/events/{}", event.id)));
    assert_eq!(fetched.id, event.id);

    let updated: Event = read(
        app.patch(&format!("/events/{}", event.id))
            .json(&json!({ "title": "Rust meetup #2", "description": null })),
    );
    assert_eq!(updated.title, "Rust meetup #2");
    assert_eq!(updated.description, None, "explicit null clears");
    assert_eq!(updated.capacity, Some(30), "omitted field kept");
    assert_eq!(updated.starts_at, event.starts_at, "omitted field kept");

    create_event(
        &app,
        json!({ "title": "Earlier", "starts_at": "2029-05-01T09:00:00Z" }),
    );
    let events: Vec<Event> = read(app.get("/events"));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].title, "Earlier", "sorted by start time");

    let page: Vec<Event> = read(app.get("/events?limit=1&offset=1"));
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, event.id, "second page of one");

    let path = format!("/events/{}", event.id);
    assert_eq!(status(app.delete(&path)), 204);
    assert_eq!(status(app.get(&path)), 404, "deleted event is gone");
    assert_eq!(status(app.delete(&path)), 404, "deleting twice");
}

#[test]
fn event_validation() {
    let app = setup();

    for body in [
        json!({ "title": "  ", "starts_at": "2030-01-01T00:00:00Z" }),
        json!({ "title": "x", "starts_at": "2030-01-02T00:00:00Z", "ends_at": "2030-01-01T00:00:00Z" }),
        json!({ "title": "x", "starts_at": "2030-01-01T00:00:00Z", "capacity": 0 }),
        json!({ "title": "x", "starts_at": "not a date" }),
        json!({ "starts_at": "2030-01-01T00:00:00Z" }),
    ] {
        let resp = send(app.post("/events").json(&body));
        assert_eq!(resp.status(), 400, "body: {body}");
        let err: Value = resp.json().unwrap();
        assert!(err["error"].is_string(), "JSON error body for {body}");
    }

    assert_eq!(status(app.get("/events/not-a-uuid")), 404);
}

#[test]
fn sso_login_creates_session() {
    let app = setup();

    let code = "code-1";
    let pending = start_login(&app, code, "user-123", "Alice Liddell");
    let resp = finish_login(&app, &pending, code);
    assert_eq!(resp.status(), 200);
    let session_cookie = set_cookie(&resp, "session").expect("session cookie");
    assert!(
        session_cookie.http_only().unwrap_or(false),
        "session cookie is HttpOnly"
    );
    let session: LoginResponse = resp.json().unwrap();
    assert_eq!(session.guest.name, "Alice Liddell");
    assert_eq!(session.guest.email.as_deref(), Some("user-123@example.com"));
    assert_eq!(session_cookie.value(), session.token);

    let me: Guest = read(bearer(app.get("/guests/me"), &session.token));
    assert_eq!(me.id, session.guest.id, "session works as a bearer token");
    let me: Guest = read(with_cookie(app.get("/guests/me"), &session_cookie));
    assert_eq!(me.id, session.guest.id, "session works as a cookie");

    let again = login(&app, "user-123", "Alice L.");
    assert_eq!(
        again.guest.id, session.guest.id,
        "same SSO subject, same guest"
    );
    assert_eq!(again.guest.name, "Alice L.", "profile change picked up");
    assert_ne!(again.token, session.token);

    let other = login(&app, "user-456", "Bob");
    assert_ne!(
        other.guest.id, session.guest.id,
        "different subject, different guest"
    );

    assert_eq!(
        status(bearer(app.post("/auth/logout"), &session.token)),
        204
    );
    assert_eq!(
        status(bearer(app.get("/guests/me"), &session.token)),
        401,
        "logged-out session rejected"
    );
    assert_eq!(
        status(bearer(app.get("/guests/me"), &again.token)),
        200,
        "logout ends only that session"
    );
}

#[test]
fn sso_rejects_tampered_logins() {
    let app = setup();

    let pending = start_login(&app, "c1", "mallory", "Mallory");
    let path = format!("/auth/callback?code=c1&state={}", pending.state);
    assert_eq!(
        status(app.get(&path)),
        401,
        "callback without the browser's state cookie (login CSRF)"
    );

    let other = start_login(&app, "c2", "mallory", "Mallory");
    assert_eq!(
        status(with_cookie(app.get(&path), &other.state_cookie)),
        401,
        "state cookie from a different login"
    );

    assert_eq!(finish_login(&app, &other, "c2").status(), 200);
    assert_eq!(
        finish_login(&app, &other, "c2").status(),
        401,
        "a state can only be used once"
    );

    let pending = start_login(&app, "c3", "mallory", "Mallory");
    assert_eq!(
        finish_login(&app, &pending, "wrong-code").status(),
        401,
        "unknown authorization code"
    );

    let pending = start_login(&app, "c4", "mallory", "Mallory");
    app.idp.force_nonce(Some("stale-nonce"));
    assert_eq!(
        finish_login(&app, &pending, "c4").status(),
        401,
        "ID token with the wrong nonce (replayed token)"
    );
    app.idp.force_nonce(None);

    let resp = send(app.get("/auth/callback?error=access_denied&error_description=denied"));
    assert_eq!(resp.status(), 401, "provider reports an error");
    let err: Value = resp.json().unwrap();
    assert_eq!(err["error"], "SSO login failed", "details are not leaked");

    assert_eq!(status(app.get("/guests/me")), 401, "no session");
    assert_eq!(
        status(bearer(app.get("/guests/me"), "bogus")),
        401,
        "bogus session"
    );
    assert_eq!(
        status(bearer(app.post("/auth/logout"), "bogus")),
        401,
        "bogus logout"
    );

    let req = app
        .post("/guests")
        .json(&json!({ "name": "x", "email": "x@example.com" }));
    assert_eq!(
        status(req),
        404,
        "the old self-registration endpoint is gone"
    );
}

#[test]
fn guests_join_and_leave() {
    let app = setup();
    let event = create_event(
        &app,
        json!({ "title": "Small dinner", "starts_at": "2030-03-01T19:00:00Z", "capacity": 1 }),
    );
    let event_path = format!("/events/{}", event.id);
    let join_path = format!("{event_path}/join");
    let leave_path = format!("{event_path}/leave");

    let alice = login(&app, "alice", "Alice");
    let bob = login(&app, "bob", "Bob");

    assert_eq!(
        status(app.post(&join_path)),
        401,
        "joining requires a session"
    );

    assert_eq!(status(bearer(app.post(&join_path), &alice.token)), 201);
    assert_eq!(
        status(bearer(app.post(&join_path), &alice.token)),
        409,
        "joining twice"
    );

    let resp = send(bearer(app.post(&join_path), &bob.token));
    assert_eq!(resp.status(), 409, "event is full for bob");
    let err: Value = resp.json().unwrap();
    assert_eq!(err["error"], "event is full");

    let uncapped = app.patch(&event_path).json(&json!({ "capacity": null }));
    assert_eq!(status(uncapped), 200);
    assert_eq!(status(bearer(app.post(&join_path), &bob.token)), 201);
    let resp = send(bearer(app.post(&join_path), &bob.token));
    assert_eq!(resp.status(), 409, "joining an uncapped event twice");
    let err: Value = resp.json().unwrap();
    assert_eq!(err["error"], "already joined this event");
    let capped = app.patch(&event_path).json(&json!({ "capacity": 1 }));
    assert_eq!(
        status(capped),
        409,
        "capacity cannot drop below attendee count"
    );

    let attendees: Vec<Value> = read(app.get(&format!("{event_path}/attendees")));
    assert_eq!(attendees.len(), 2);
    assert!(
        attendees.iter().all(|a| a.get("email").is_none()),
        "attendee list hides emails"
    );
    let attendees: Vec<Attendee> = serde_json::from_value(Value::Array(attendees)).unwrap();
    assert_eq!(attendees[0].name, "Alice", "ordered by join time");

    let mine: Vec<Event> = read(bearer(app.get("/guests/me/events"), &bob.token));
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].attendee_count, 2);

    assert_eq!(status(bearer(app.post(&leave_path), &alice.token)), 204);
    assert_eq!(
        status(bearer(app.post(&leave_path), &alice.token)),
        404,
        "leaving twice"
    );

    assert_eq!(status(app.delete(&event_path)), 204);
    let mine: Vec<Event> = read(bearer(app.get("/guests/me/events"), &bob.token));
    assert!(mine.is_empty(), "deleting the event removes attendances");

    assert_eq!(
        status(bearer(app.post(&join_path), &bob.token)),
        404,
        "joining a missing event"
    );
}
