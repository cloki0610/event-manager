//! Stand-in OpenID Connect provider for driving the real app locally.
//! Used by `.claude/skills/verify-event-manager`; never deploy it.
//!
//! `GET /authorize` consents at once and redirects back with a code. The identity comes from
//! `sub`, `name` and `email` query parameters appended to the authorize URL (default `alice`);
//! `deny=1` makes it answer `error=access_denied` instead.
//!
//! Env: `MOCK_IDP_ADDR` (default `127.0.0.1:18081`), `MOCK_IDP_KEY` (RSA private key PEM, required),
//! `MOCK_IDP_CLIENT_ID` (default `event-manager`).

use std::collections::HashMap;
use std::sync::Mutex;

use actix_web::http::header;
use actix_web::{App, HttpResponse, HttpServer, web};
use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::url::Url;
use openidconnect::{
    Audience, EmptyAdditionalClaims, EndUserEmail, EndUserName, IssuerUrl, JsonWebKeyId,
    LocalizedClaim, Nonce, PkceCodeChallenge, PkceCodeVerifier, PrivateSigningKey,
    StandardClaims, SubjectIdentifier,
};
use serde_json::json;
use uuid::Uuid;

struct Grant {
    subject: String,
    name: String,
    email: String,
    nonce: String,
    code_challenge: String,
}

struct Idp {
    issuer: String,
    client_id: String,
    key_pem: String,
    grants: Mutex<HashMap<String, Grant>>,
}

impl Idp {
    fn signing_key(&self) -> CoreRsaPrivateSigningKey {
        CoreRsaPrivateSigningKey::from_pem(&self.key_pem, Some(JsonWebKeyId::new("mock".into())))
            .expect("MOCK_IDP_KEY is not an RSA private key")
    }
}

async fn discovery(idp: web::Data<Idp>) -> HttpResponse {
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

async fn jwks(idp: web::Data<Idp>) -> HttpResponse {
    HttpResponse::Ok().json(CoreJsonWebKeySet::new(vec![idp.signing_key().as_verification_key()]))
}

async fn authorize(idp: web::Data<Idp>, q: web::Query<HashMap<String, String>>) -> HttpResponse {
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    if get("client_id") != idp.client_id {
        return HttpResponse::BadRequest().body("unknown client_id");
    }
    let Ok(mut redirect) = Url::parse(&get("redirect_uri")) else {
        return HttpResponse::BadRequest().body("invalid redirect_uri");
    };

    if get("deny") == "1" {
        redirect
            .query_pairs_mut()
            .append_pair("error", "access_denied")
            .append_pair("error_description", "user denied consent")
            .append_pair("state", &get("state"));
    } else {
        let subject = q.get("sub").cloned().unwrap_or_else(|| "alice".into());
        let code = Uuid::new_v4().simple().to_string();
        idp.grants.lock().unwrap().insert(
            code.clone(),
            Grant {
                name: q.get("name").cloned().unwrap_or_else(|| subject.clone()),
                email: q.get("email").cloned().unwrap_or_else(|| format!("{subject}@example.com")),
                subject,
                nonce: get("nonce"),
                code_challenge: get("code_challenge"),
            },
        );
        redirect
            .query_pairs_mut()
            .append_pair("code", &code)
            .append_pair("state", &get("state"));
    }
    HttpResponse::Found().insert_header((header::LOCATION, redirect.to_string())).finish()
}

async fn token(idp: web::Data<Idp>, form: web::Form<HashMap<String, String>>) -> HttpResponse {
    let invalid_grant = || HttpResponse::BadRequest().json(json!({ "error": "invalid_grant" }));
    let Some(grant) = form.get("code").and_then(|c| idp.grants.lock().unwrap().remove(c)) else {
        return invalid_grant();
    };
    let verifier = PkceCodeVerifier::new(form.get("code_verifier").cloned().unwrap_or_default());
    if PkceCodeChallenge::from_code_verifier_sha256(&verifier).as_str() != grant.code_challenge {
        return invalid_grant();
    }

    let mut name = LocalizedClaim::new();
    name.insert(None, EndUserName::new(grant.name));
    let claims = CoreIdTokenClaims::new(
        IssuerUrl::new(idp.issuer.clone()).unwrap(),
        vec![Audience::new(idp.client_id.clone())],
        Utc::now() + Duration::minutes(5),
        Utc::now(),
        StandardClaims::new(SubjectIdentifier::new(grant.subject))
            .set_name(Some(name))
            .set_email(Some(EndUserEmail::new(grant.email))),
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(grant.nonce)));
    let id_token = CoreIdToken::new(
        claims,
        &idp.signing_key(),
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("MOCK_IDP_ADDR").unwrap_or_else(|_| "127.0.0.1:18081".into());
    let key_path = std::env::var("MOCK_IDP_KEY").map_err(|_| std::io::Error::other("MOCK_IDP_KEY must be set"))?;
    let idp = web::Data::new(Idp {
        issuer: format!("http://{addr}"),
        client_id: std::env::var("MOCK_IDP_CLIENT_ID").unwrap_or_else(|_| "event-manager".into()),
        key_pem: std::fs::read_to_string(key_path)?,
        grants: Mutex::default(),
    });
    idp.signing_key();

    println!("mock idp listening on {}", idp.issuer);
    HttpServer::new(move || {
        App::new()
            .app_data(idp.clone())
            .route("/.well-known/openid-configuration", web::get().to(discovery))
            .route("/jwks", web::get().to(jwks))
            .route("/authorize", web::get().to(authorize))
            .route("/token", web::post().to(token))
    })
    .workers(1)
    .bind(addr)?
    .run()
    .await
}
