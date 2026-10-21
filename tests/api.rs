use std::collections::HashMap;
use std::sync::Mutex;

use actix_web::cookie::Cookie;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::{App, HttpResponse, HttpServer, test, web};
use chrono::{Duration, Utc};
use event_manager::db::Db;
use event_manager::models::{Attendee, Event, Guest, LoginResponse};
use event_manager::sso::{Sso, SsoConfig};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::url::Url;
use openidconnect::{
    Audience, EmptyAdditionalClaims, EndUserEmail, EndUserName, IssuerUrl, JsonWebKeyId,
    LocalizedClaim, Nonce, PkceCodeChallenge, PkceCodeVerifier, PrivateSigningKey, StandardClaims,
    SubjectIdentifier,
};
use serde_json::{Value, json};

const CLIENT_ID: &str = "event-manager";
const REDIRECT_URL: &str = "http://localhost/auth/callback";
const IDP_KEY: &str = include_str!("fixtures/test_idp_key.pem");

// ---------------------------------------------------------------------------
// Mock OpenID Connect provider
// ---------------------------------------------------------------------------

/// What the user "consented to" at the provider for a given authorization code.
struct Grant {
    subject: String,
    name: String,
    email: String,
    nonce: String,
    code_challenge: String,
}

struct MockIdp {
    issuer: String,
    grants: Mutex<HashMap<String, Grant>>,
    /// When set, ID tokens are signed with this nonce instead of the requested one.
    forced_nonce: Mutex<Option<String>>,
}

fn signing_key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(IDP_KEY, Some(JsonWebKeyId::new("test-key".into()))).unwrap()
}

async fn discovery(idp: web::Data<MockIdp>) -> HttpResponse {
    let i = &idp.issuer;
    HttpResponse::Ok().json(json!({
        "issuer": i,
        "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"),
        "jwks_uri": format!("{i}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    }))
}

async fn jwks() -> HttpResponse {
    HttpResponse::Ok().json(CoreJsonWebKeySet::new(vec![
        signing_key().as_verification_key(),
    ]))
}

async fn token(idp: web::Data<MockIdp>, form: web::Form<HashMap<String, String>>) -> HttpResponse {
    let invalid_grant = || HttpResponse::BadRequest().json(json!({ "error": "invalid_grant" }));
    let Some(grant) = form
        .get("code")
        .and_then(|c| idp.grants.lock().unwrap().remove(c))
    else {
        return invalid_grant();
    };
    let verifier = PkceCodeVerifier::new(form.get("code_verifier").cloned().unwrap_or_default());
    if PkceCodeChallenge::from_code_verifier_sha256(&verifier).as_str() != grant.code_challenge {
        return invalid_grant();
    }

    let nonce = idp
        .forced_nonce
        .lock()
        .unwrap()
        .clone()
        .unwrap_or(grant.nonce);
    let mut name = LocalizedClaim::new();
    name.insert(None, EndUserName::new(grant.name));
    let claims = CoreIdTokenClaims::new(
        IssuerUrl::new(idp.issuer.clone()).unwrap(),
        vec![Audience::new(CLIENT_ID.into())],
        Utc::now() + Duration::minutes(5),
        Utc::now(),
        StandardClaims::new(SubjectIdentifier::new(grant.subject))
            .set_name(Some(name))
            .set_email(Some(EndUserEmail::new(grant.email))),
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(nonce)));
    let id_token = CoreIdToken::new(
        claims,
        &signing_key(),
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        None,
        None,
    )
    .unwrap();

    HttpResponse::Ok().json(json!({
        "access_token": "mock-access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
    }))
}

/// Starts the mock provider on a random port.
async fn start_idp() -> web::Data<MockIdp> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let idp = web::Data::new(MockIdp {
        issuer: format!("http://{}", listener.local_addr().unwrap()),
        grants: Mutex::default(),
        forced_nonce: Mutex::default(),
    });
    let data = idp.clone();
    let server = HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .route(
                "/.well-known/openid-configuration",
                web::get().to(discovery),
            )
            .route("/jwks", web::get().to(jwks))
            .route("/token", web::post().to(token))
    })
    .workers(1)
    .listen(listener)
    .unwrap()
    .run();
    actix_web::rt::spawn(server);
    idp
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

async fn setup() -> (
    impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
    web::Data<MockIdp>,
) {
    let idp = start_idp().await;
    let sso = Sso::discover(SsoConfig {
        issuer_url: idp.issuer.clone(),
        client_id: CLIENT_ID.into(),
        client_secret: Some("secret".into()),
        redirect_url: REDIRECT_URL.into(),
        post_login_redirect: None,
        session_ttl: Duration::hours(1),
    })
    .await
    .unwrap();
    // A single connection keeps every query on the same in-memory database.
    // TEST_DATABASE_URL=duckdb://:memory: runs the suite against DuckDB (needs `--features duckdb`).
    let url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| "sqlite::memory:".into());
    let db = Db::connect(&url, 1).await.unwrap();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(db))
            .app_data(web::Data::new(sso))
            .configure(event_manager::configure),
    )
    .await;
    (app, idp)
}

/// A login started at `/auth/login`, paused at the provider's consent screen.
struct PendingLogin {
    state: String,
    state_cookie: Cookie<'static>,
}

async fn start_login<S>(
    app: &S,
    idp: &MockIdp,
    code: &str,
    subject: &str,
    name: &str,
) -> PendingLogin
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get().uri("/auth/login").to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FOUND);
    let state_cookie = resp
        .response()
        .cookies()
        .find(|c| c.name() == "oidc_state")
        .expect("state cookie")
        .into_owned();

    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    let url = Url::parse(location).unwrap();
    assert!(location.starts_with(&format!("{}/authorize?", idp.issuer)));
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], REDIRECT_URL);
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].split(' ').any(|s| s == "openid"));

    idp.grants.lock().unwrap().insert(
        code.into(),
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

async fn finish_login<S>(app: &S, login: &PendingLogin, code: &str) -> ServiceResponse
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let req = test::TestRequest::get()
        .uri(&format!("/auth/callback?code={code}&state={}", login.state))
        .cookie(login.state_cookie.clone())
        .to_request();
    test::call_service(app, req).await
}

/// Runs the whole SSO login for `subject` and returns the session.
async fn login<S>(app: &S, idp: &MockIdp, subject: &str, name: &str) -> LoginResponse
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let code = format!("code-{subject}-{}", uuid::Uuid::new_v4());
    let pending = start_login(app, idp, &code, subject, name).await;
    let resp = finish_login(app, &pending, &code).await;
    assert_eq!(resp.status(), StatusCode::OK);
    test::read_body_json(resp).await
}

fn bearer(token: &str) -> (&'static str, String) {
    ("Authorization", format!("Bearer {token}"))
}

async fn create_event<S>(app: &S, body: Value) -> Event
where
    S: Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
{
    let req = test::TestRequest::post()
        .uri("/events")
        .set_json(body)
        .to_request();
    let resp = test::call_service(app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    test::read_body_json(resp).await
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[actix_web::test]
async fn event_crud() {
    let (app, _idp) = setup().await;

    let event = create_event(
        &app,
        json!({
            "title": "Rust meetup",
            "description": "Talks and pizza",
            "starts_at": "2030-01-10T18:00:00Z",
            "ends_at": "2030-01-10T21:00:00Z",
            "capacity": 30
        }),
    )
    .await;
    assert_eq!(event.title, "Rust meetup");
    assert_eq!(event.attendee_count, 0);

    // view
    let req = test::TestRequest::get()
        .uri(&format!("/events/{}", event.id))
        .to_request();
    let fetched: Event = test::call_and_read_body_json(&app, req).await;
    assert_eq!(fetched.id, event.id);

    // partial update: change title, clear description, leave the rest untouched
    let req = test::TestRequest::patch()
        .uri(&format!("/events/{}", event.id))
        .set_json(json!({ "title": "Rust meetup #2", "description": null }))
        .to_request();
    let updated: Event = test::call_and_read_body_json(&app, req).await;
    assert_eq!(updated.title, "Rust meetup #2");
    assert_eq!(updated.description, None);
    assert_eq!(updated.capacity, Some(30));
    assert_eq!(updated.starts_at, event.starts_at);

    // list
    create_event(
        &app,
        json!({ "title": "Earlier", "starts_at": "2029-05-01T09:00:00Z" }),
    )
    .await;
    let req = test::TestRequest::get().uri("/events").to_request();
    let events: Vec<Event> = test::call_and_read_body_json(&app, req).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].title, "Earlier", "sorted by start time");

    let req = test::TestRequest::get()
        .uri("/events?limit=1&offset=1")
        .to_request();
    let page: Vec<Event> = test::call_and_read_body_json(&app, req).await;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, event.id);

    // delete
    let req = test::TestRequest::delete()
        .uri(&format!("/events/{}", event.id))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NO_CONTENT
    );
    let req = test::TestRequest::get()
        .uri(&format!("/events/{}", event.id))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NOT_FOUND
    );
    let req = test::TestRequest::delete()
        .uri(&format!("/events/{}", event.id))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[actix_web::test]
async fn event_validation() {
    let (app, _idp) = setup().await;

    for body in [
        json!({ "title": "  ", "starts_at": "2030-01-01T00:00:00Z" }),
        json!({ "title": "x", "starts_at": "2030-01-02T00:00:00Z", "ends_at": "2030-01-01T00:00:00Z" }),
        json!({ "title": "x", "starts_at": "2030-01-01T00:00:00Z", "capacity": 0 }),
        json!({ "title": "x", "starts_at": "not a date" }),
        json!({ "starts_at": "2030-01-01T00:00:00Z" }),
    ] {
        let req = test::TestRequest::post()
            .uri("/events")
            .set_json(&body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "body: {body}");
        let err: Value = test::read_body_json(resp).await;
        assert!(err["error"].is_string());
    }

    let req = test::TestRequest::get()
        .uri("/events/not-a-uuid")
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------------------
// SSO
// ---------------------------------------------------------------------------

#[actix_web::test]
async fn sso_login_creates_session() {
    let (app, idp) = setup().await;

    let code = "code-1";
    let pending = start_login(&app, &idp, code, "user-123", "Alice Liddell").await;
    let resp = finish_login(&app, &pending, code).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let session_cookie = resp
        .response()
        .cookies()
        .find(|c| c.name() == "session")
        .expect("session cookie")
        .into_owned();
    assert!(session_cookie.http_only().unwrap_or(false));
    let session: LoginResponse = test::read_body_json(resp).await;
    assert_eq!(session.guest.name, "Alice Liddell");
    assert_eq!(session.guest.email.as_deref(), Some("user-123@example.com"));
    assert_eq!(session_cookie.value(), session.token);

    // the session works as a bearer token and as a cookie
    let req = test::TestRequest::get()
        .uri("/guests/me")
        .insert_header(bearer(&session.token))
        .to_request();
    let me: Guest = test::call_and_read_body_json(&app, req).await;
    assert_eq!(me.id, session.guest.id);
    let req = test::TestRequest::get()
        .uri("/guests/me")
        .cookie(session_cookie)
        .to_request();
    let me: Guest = test::call_and_read_body_json(&app, req).await;
    assert_eq!(me.id, session.guest.id);

    // the same SSO subject maps to the same guest; profile changes are picked up
    let again = login(&app, &idp, "user-123", "Alice L.").await;
    assert_eq!(again.guest.id, session.guest.id);
    assert_eq!(again.guest.name, "Alice L.");
    assert_ne!(again.token, session.token);

    // a different subject is a different guest
    let other = login(&app, &idp, "user-456", "Bob").await;
    assert_ne!(other.guest.id, session.guest.id);

    // logout invalidates only that session
    let req = test::TestRequest::post()
        .uri("/auth/logout")
        .insert_header(bearer(&session.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NO_CONTENT
    );
    let req = test::TestRequest::get()
        .uri("/guests/me")
        .insert_header(bearer(&session.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let req = test::TestRequest::get()
        .uri("/guests/me")
        .insert_header(bearer(&again.token))
        .to_request();
    assert_eq!(test::call_service(&app, req).await.status(), StatusCode::OK);
}

#[actix_web::test]
async fn sso_rejects_tampered_logins() {
    let (app, idp) = setup().await;

    // callback without the browser's state cookie (login CSRF)
    let pending = start_login(&app, &idp, "c1", "mallory", "Mallory").await;
    let req = test::TestRequest::get()
        .uri(&format!("/auth/callback?code=c1&state={}", pending.state))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // state cookie from a different login
    let other = start_login(&app, &idp, "c2", "mallory", "Mallory").await;
    let req = test::TestRequest::get()
        .uri(&format!("/auth/callback?code=c1&state={}", pending.state))
        .cookie(other.state_cookie.clone())
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // a state can only be used once
    assert_eq!(
        finish_login(&app, &other, "c2").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        finish_login(&app, &other, "c2").await.status(),
        StatusCode::UNAUTHORIZED
    );

    // unknown authorization code
    let pending = start_login(&app, &idp, "c3", "mallory", "Mallory").await;
    assert_eq!(
        finish_login(&app, &pending, "wrong-code").await.status(),
        StatusCode::UNAUTHORIZED
    );

    // ID token with the wrong nonce (replayed token)
    let pending = start_login(&app, &idp, "c4", "mallory", "Mallory").await;
    *idp.forced_nonce.lock().unwrap() = Some("stale-nonce".into());
    assert_eq!(
        finish_login(&app, &pending, "c4").await.status(),
        StatusCode::UNAUTHORIZED
    );
    *idp.forced_nonce.lock().unwrap() = None;

    // provider reports an error (e.g. user denied consent)
    let req = test::TestRequest::get()
        .uri("/auth/callback?error=access_denied&error_description=denied")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let err: Value = test::read_body_json(resp).await;
    assert_eq!(err["error"], "SSO login failed", "details are not leaked");

    // missing or bogus sessions
    let req = test::TestRequest::get().uri("/guests/me").to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let req = test::TestRequest::get()
        .uri("/guests/me")
        .insert_header(bearer("bogus"))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let req = test::TestRequest::post()
        .uri("/auth/logout")
        .insert_header(bearer("bogus"))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // the old self-registration endpoint is gone
    let req = test::TestRequest::post()
        .uri("/guests")
        .set_json(json!({ "name": "x", "email": "x@example.com" }))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------------------
// Attendance
// ---------------------------------------------------------------------------

#[actix_web::test]
async fn guests_join_and_leave() {
    let (app, idp) = setup().await;
    let event = create_event(
        &app,
        json!({ "title": "Small dinner", "starts_at": "2030-03-01T19:00:00Z", "capacity": 1 }),
    )
    .await;
    let join_uri = format!("/events/{}/join", event.id);
    let leave_uri = format!("/events/{}/leave", event.id);

    let alice = login(&app, &idp, "alice", "Alice").await;
    let bob = login(&app, &idp, "bob", "Bob").await;

    // joining requires a session
    let req = test::TestRequest::post().uri(&join_uri).to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // alice joins; joining twice conflicts
    let req = test::TestRequest::post()
        .uri(&join_uri)
        .insert_header(bearer(&alice.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::CREATED
    );
    let req = test::TestRequest::post()
        .uri(&join_uri)
        .insert_header(bearer(&alice.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::CONFLICT
    );

    // event is full for bob
    let req = test::TestRequest::post()
        .uri(&join_uri)
        .insert_header(bearer(&bob.token))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let err: Value = test::read_body_json(resp).await;
    assert_eq!(err["error"], "event is full");

    // capacity cannot drop below attendee count
    let req = test::TestRequest::patch()
        .uri(&format!("/events/{}", event.id))
        .set_json(json!({ "capacity": null }))
        .to_request();
    assert_eq!(test::call_service(&app, req).await.status(), StatusCode::OK);
    let req = test::TestRequest::post()
        .uri(&join_uri)
        .insert_header(bearer(&bob.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::CREATED
    );
    let req = test::TestRequest::patch()
        .uri(&format!("/events/{}", event.id))
        .set_json(json!({ "capacity": 1 }))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::CONFLICT
    );

    // attendee list hides emails
    let req = test::TestRequest::get()
        .uri(&format!("/events/{}/attendees", event.id))
        .to_request();
    let attendees: Vec<Value> = test::call_and_read_body_json(&app, req).await;
    assert_eq!(attendees.len(), 2);
    assert!(attendees.iter().all(|a| a.get("email").is_none()));
    let attendees: Vec<Attendee> = serde_json::from_value(Value::Array(attendees)).unwrap();
    assert_eq!(attendees[0].name, "Alice");

    // my events
    let req = test::TestRequest::get()
        .uri("/guests/me/events")
        .insert_header(bearer(&bob.token))
        .to_request();
    let mine: Vec<Event> = test::call_and_read_body_json(&app, req).await;
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].attendee_count, 2);

    // leave; leaving again is 404
    let req = test::TestRequest::post()
        .uri(&leave_uri)
        .insert_header(bearer(&alice.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NO_CONTENT
    );
    let req = test::TestRequest::post()
        .uri(&leave_uri)
        .insert_header(bearer(&alice.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NOT_FOUND
    );

    // deleting the event removes attendances
    let req = test::TestRequest::delete()
        .uri(&format!("/events/{}", event.id))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NO_CONTENT
    );
    let req = test::TestRequest::get()
        .uri("/guests/me/events")
        .insert_header(bearer(&bob.token))
        .to_request();
    let mine: Vec<Event> = test::call_and_read_body_json(&app, req).await;
    assert!(mine.is_empty());

    // joining a missing event
    let req = test::TestRequest::post()
        .uri(&join_uri)
        .insert_header(bearer(&bob.token))
        .to_request();
    assert_eq!(
        test::call_service(&app, req).await.status(),
        StatusCode::NOT_FOUND
    );
}
