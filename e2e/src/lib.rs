//! Stand-in OpenID Connect provider for driving the real Worker locally. Never deploy it.
//!
//! Serves discovery, JWKS, `/authorize` and `/token`. `GET /authorize` consents at once and
//! redirects back with a code. The identity comes from `sub`, `name` and `email` query
//! parameters appended to the authorize URL (default `alice`), and `deny=1` answers
//! `error=access_denied` instead. Tests can also register a grant directly with
//! [`MockIdp::grant`] and skip `/authorize`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::url::{Url, form_urlencoded};
use openidconnect::{
    Audience, EmptyAdditionalClaims, EndUserEmail, EndUserName, IssuerUrl, JsonWebKeyId,
    LocalizedClaim, Nonce, PkceCodeChallenge, PkceCodeVerifier, PrivateSigningKey, StandardClaims,
    SubjectIdentifier,
};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};
use uuid::Uuid;

/// What the user "consented to" at the provider for one authorization code.
pub struct Grant {
    pub subject: String,
    pub name: String,
    pub email: String,
    pub nonce: String,
    pub code_challenge: String,
}

pub struct MockIdp {
    issuer: String,
    client_id: String,
    /// When set, the token endpoint requires it via HTTP Basic client authentication.
    client_secret: Option<String>,
    key: CoreRsaPrivateSigningKey,
    grants: Mutex<HashMap<String, Grant>>,
    /// When set, ID tokens carry this nonce instead of the requested one.
    forced_nonce: Mutex<Option<String>>,
}

type Reply = Response<std::io::Cursor<Vec<u8>>>;

impl MockIdp {
    /// Serves the provider on `addr` (port 0 picks a free one) from a background thread.
    pub fn start(
        addr: &str,
        client_id: &str,
        client_secret: Option<&str>,
        key_pem: &str,
    ) -> Result<Arc<Self>, String> {
        let key =
            CoreRsaPrivateSigningKey::from_pem(key_pem, Some(JsonWebKeyId::new("mock".into())))
                .map_err(|e| format!("signing key is not an RSA private key: {e}"))?;
        let server = Server::http(addr).map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        let bound = server
            .server_addr()
            .to_ip()
            .ok_or("mock idp is not listening on TCP")?;
        let idp = Arc::new(Self {
            issuer: format!("http://{bound}"),
            client_id: client_id.into(),
            client_secret: client_secret.map(Into::into),
            key,
            grants: Mutex::default(),
            forced_nonce: Mutex::default(),
        });
        let served = idp.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let reply = served.handle(&mut req);
                let _ = req.respond(reply);
            }
        });
        Ok(idp)
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Registers the outcome of a consent screen for `code`.
    pub fn grant(&self, code: &str, grant: Grant) {
        self.grants.lock().unwrap().insert(code.into(), grant);
    }

    /// Makes later ID tokens carry `nonce` (a replayed token) until reset with `None`.
    pub fn force_nonce(&self, nonce: Option<&str>) {
        *self.forced_nonce.lock().unwrap() = nonce.map(Into::into);
    }

    fn handle(&self, req: &mut Request) -> Reply {
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let query: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        match (req.method(), path) {
            (Method::Get, "/.well-known/openid-configuration") => self.discovery(),
            (Method::Get, "/jwks") => json_reply(
                200,
                &json!(CoreJsonWebKeySet::new(vec![self.key.as_verification_key()])),
            ),
            (Method::Get, "/authorize") => self.authorize(&query),
            (Method::Post, "/token") => {
                let authorization = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("authorization"))
                    .map(|h| h.value.to_string());
                let mut body = Vec::new();
                let _ = req.as_reader().read_to_end(&mut body);
                let form = form_urlencoded::parse(&body).into_owned().collect();
                self.token(authorization.as_deref(), &form)
            }
            _ => Response::from_string("not found").with_status_code(404),
        }
    }

    fn discovery(&self) -> Reply {
        let i = &self.issuer;
        json_reply(
            200,
            &json!({
                "issuer": i,
                "authorization_endpoint": format!("{i}/authorize"),
                "token_endpoint": format!("{i}/token"),
                "jwks_uri": format!("{i}/jwks"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"],
            }),
        )
    }

    fn authorize(&self, q: &HashMap<String, String>) -> Reply {
        let get = |k: &str| q.get(k).cloned().unwrap_or_default();
        if get("client_id") != self.client_id {
            return Response::from_string("unknown client_id").with_status_code(400);
        }
        let Ok(mut redirect) = Url::parse(&get("redirect_uri")) else {
            return Response::from_string("invalid redirect_uri").with_status_code(400);
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
            self.grant(
                &code,
                Grant {
                    name: q.get("name").cloned().unwrap_or_else(|| subject.clone()),
                    email: q
                        .get("email")
                        .cloned()
                        .unwrap_or_else(|| format!("{subject}@example.com")),
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
        Response::from_string("")
            .with_status_code(302)
            .with_header(Header::from_bytes("Location", redirect.as_str()).unwrap())
    }

    fn token(&self, authorization: Option<&str>, form: &HashMap<String, String>) -> Reply {
        if let Some(secret) = &self.client_secret {
            let expected = format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{secret}", self.client_id))
            );
            if authorization != Some(expected.as_str()) {
                return json_reply(401, &json!({ "error": "invalid_client" }));
            }
        }
        let invalid_grant = || json_reply(400, &json!({ "error": "invalid_grant" }));
        let Some(grant) = form
            .get("code")
            .and_then(|c| self.grants.lock().unwrap().remove(c))
        else {
            return invalid_grant();
        };
        let verifier =
            PkceCodeVerifier::new(form.get("code_verifier").cloned().unwrap_or_default());
        if PkceCodeChallenge::from_code_verifier_sha256(&verifier).as_str() != grant.code_challenge
        {
            return invalid_grant();
        }

        let nonce = self
            .forced_nonce
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(grant.nonce);
        let mut name = LocalizedClaim::new();
        name.insert(None, EndUserName::new(grant.name));
        let claims = CoreIdTokenClaims::new(
            IssuerUrl::new(self.issuer.clone()).unwrap(),
            vec![Audience::new(self.client_id.clone())],
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
            &self.key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            None,
            None,
        )
        .unwrap();

        json_reply(
            200,
            &json!({
                "access_token": "mock-access-token",
                "token_type": "Bearer",
                "expires_in": 300,
                "id_token": id_token,
            }),
        )
    }
}

fn json_reply(status: u16, body: &Value) -> Reply {
    Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}
