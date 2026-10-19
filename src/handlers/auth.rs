use actix_web::cookie::{Cookie, SameSite, time};
use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, web};
use chrono::{DateTime, Duration, Utc};
use openidconnect::core::CoreAuthenticationFlow;
use openidconnect::{
    AuthorizationCode, CsrfToken, Nonce, PkceCodeChallenge, PkceCodeVerifier, Scope,
    TokenResponse,
};
use serde::Deserialize;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::auth::{SESSION_COOKIE, create_session, delete_session, session_token};
use crate::error::AppError;
use crate::models::{Guest, LoginResponse};
use crate::sso::Sso;

/// Binds an in-flight login to the browser that started it (prevents login CSRF).
const STATE_COOKIE: &str = "oidc_state";
const LOGIN_TIMEOUT_MINUTES: i64 = 10;

/// Starts the authorization-code flow (with PKCE) by redirecting to the provider.
pub async fn login(
    pool: web::Data<SqlitePool>,
    sso: web::Data<Sso>,
) -> Result<HttpResponse, AppError> {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, state, nonce) = sso
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let now = Utc::now();
    sqlx::query("DELETE FROM login_states WHERE created_at < ?")
        .bind(now - Duration::minutes(LOGIN_TIMEOUT_MINUTES))
        .execute(pool.get_ref())
        .await?;
    sqlx::query(
        "INSERT INTO login_states (state, nonce, pkce_verifier, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(state.secret())
    .bind(nonce.secret())
    .bind(pkce_verifier.secret())
    .bind(now)
    .execute(pool.get_ref())
    .await?;

    let state_cookie = Cookie::build(STATE_COOKIE, state.secret().clone())
        .path("/auth")
        .http_only(true)
        .secure(sso.secure_cookies())
        .same_site(SameSite::Lax)
        .max_age(time::Duration::minutes(LOGIN_TIMEOUT_MINUTES))
        .finish();

    Ok(HttpResponse::Found()
        .insert_header((header::LOCATION, auth_url.to_string()))
        .cookie(state_cookie)
        .finish())
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Completes the login: verifies the ID token, upserts the guest and starts a session.
pub async fn callback(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    sso: web::Data<Sso>,
    params: web::Query<CallbackParams>,
) -> Result<HttpResponse, AppError> {
    let params = params.into_inner();
    if let Some(error) = params.error {
        return Err(AppError::LoginFailed(format!(
            "provider returned {error}: {}",
            params.error_description.unwrap_or_default()
        )));
    }
    let (Some(code), Some(state)) = (params.code, params.state) else {
        return Err(AppError::BadRequest("missing code or state".into()));
    };

    let cookie_state = req.cookie(STATE_COOKIE).map(|c| c.value().to_string());
    if cookie_state.as_deref() != Some(state.as_str()) {
        return Err(AppError::LoginFailed("state does not match login cookie".into()));
    }

    // Single use: the row is consumed whether or not the rest of the login succeeds.
    let pending: Option<(String, String, DateTime<Utc>)> = sqlx::query_as(
        "DELETE FROM login_states WHERE state = ? RETURNING nonce, pkce_verifier, created_at",
    )
    .bind(&state)
    .fetch_optional(pool.get_ref())
    .await?;
    let Some((nonce, pkce_verifier, created_at)) = pending else {
        return Err(AppError::LoginFailed("unknown or already used state".into()));
    };
    if Utc::now() - created_at > Duration::minutes(LOGIN_TIMEOUT_MINUTES) {
        return Err(AppError::LoginFailed("login attempt expired".into()));
    }

    let token_response = sso
        .client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| AppError::LoginFailed(format!("provider has no token endpoint: {e}")))?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
        .request_async(&sso.http)
        .await
        .map_err(|e| AppError::LoginFailed(format!("code exchange failed: {e}")))?;

    let id_token = token_response
        .id_token()
        .ok_or_else(|| AppError::LoginFailed("provider returned no ID token".into()))?;
    let claims = id_token
        .claims(&sso.client.id_token_verifier(), &Nonce::new(nonce))
        .map_err(|e| AppError::LoginFailed(format!("invalid ID token: {e}")))?;

    let subject = claims.subject().as_str();
    let email = claims.email().map(|e| e.as_str().to_string());
    let name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.as_str().to_string())
        .or_else(|| claims.preferred_username().map(|u| u.as_str().to_string()))
        .or_else(|| email.clone())
        .unwrap_or_else(|| subject.to_string());

    let now = Utc::now();
    let guest = sqlx::query_as::<_, Guest>(
        "INSERT INTO guests (id, issuer, subject, name, email, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (issuer, subject) DO UPDATE SET \
             name = excluded.name, email = excluded.email, updated_at = excluded.updated_at \
         RETURNING id, name, email, created_at",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(claims.issuer().as_str())
    .bind(subject)
    .bind(&name)
    .bind(&email)
    .bind(now)
    .bind(now)
    .fetch_one(pool.get_ref())
    .await?;

    let (token, expires_at) = create_session(&pool, &guest.id, sso.config.session_ttl).await?;

    let session_cookie = Cookie::build(SESSION_COOKIE, token.clone())
        .path("/")
        .http_only(true)
        .secure(sso.secure_cookies())
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(sso.config.session_ttl.num_seconds()))
        .finish();
    let mut clear_state = Cookie::build(STATE_COOKIE, "").path("/auth").finish();
    clear_state.make_removal();

    let mut resp = match &sso.config.post_login_redirect {
        Some(target) => HttpResponse::Found()
            .insert_header((header::LOCATION, target.as_str()))
            .finish(),
        None => HttpResponse::Ok().json(LoginResponse { guest, token, expires_at }),
    };
    resp.add_cookie(&session_cookie).map_err(|e| AppError::LoginFailed(e.to_string()))?;
    resp.add_cookie(&clear_state).map_err(|e| AppError::LoginFailed(e.to_string()))?;
    Ok(resp)
}

/// Ends the current session (Bearer token or cookie).
pub async fn logout(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
) -> Result<HttpResponse, AppError> {
    let token = session_token(&req).ok_or(AppError::Unauthorized)?;
    if !delete_session(&pool, &token).await? {
        return Err(AppError::Unauthorized);
    }
    let mut clear = Cookie::build(SESSION_COOKIE, "").path("/").finish();
    clear.make_removal();
    Ok(HttpResponse::NoContent().cookie(clear).finish())
}
