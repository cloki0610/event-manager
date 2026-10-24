use chrono::{Duration, Utc};
use openidconnect::core::CoreAuthenticationFlow;
use openidconnect::{
    AuthorizationCode, CsrfToken, Nonce, PkceCodeChallenge, PkceCodeVerifier, Scope, TokenResponse,
};
use serde::Deserialize;
use worker::{Request, Response};

use super::{Ctx, empty, json, query};
use crate::auth::{SESSION_COOKIE, cookie, create_session, delete_session, session_token};
use crate::db::{Db, GuestClaims, LoginState};
use crate::error::{AppError, internal};
use crate::models::LoginResponse;
use crate::sso::{Sso, http};

/// Binds an in-flight login to the browser that started it (prevents login CSRF).
const STATE_COOKIE: &str = "oidc_state";
const LOGIN_TIMEOUT_MINUTES: i64 = 10;

/// A `Set-Cookie` value. A `max_age` of zero removes the cookie.
fn set_cookie(name: &str, value: &str, path: &str, max_age: i64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}")
}

fn add_cookie(resp: &Response, cookie: &str) -> Result<(), AppError> {
    resp.headers()
        .append("set-cookie", cookie)
        .map_err(internal)
}

fn redirect(location: &str) -> Result<Response, AppError> {
    let resp = empty(302)?;
    resp.headers().set("location", location).map_err(internal)?;
    Ok(resp)
}

/// Starts the authorization-code flow (with PKCE) by redirecting to the provider.
pub async fn login(_req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let sso = Sso::get(&ctx.env).await?;
    let db = Db::from_env(&ctx.env)?;
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
    db.insert_login_state(
        state.secret(),
        nonce.secret(),
        pkce_verifier.secret(),
        now,
        now - Duration::minutes(LOGIN_TIMEOUT_MINUTES),
    )
    .await?;

    let resp = redirect(auth_url.as_str())?;
    add_cookie(
        &resp,
        &set_cookie(
            STATE_COOKIE,
            state.secret(),
            "/auth",
            LOGIN_TIMEOUT_MINUTES * 60,
            sso.secure_cookies(),
        ),
    )?;
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Completes the login: verifies the ID token, upserts the guest and starts a session.
pub async fn callback(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let params: CallbackParams = query(&req)?;
    let sso = Sso::get(&ctx.env).await?;
    let db = Db::from_env(&ctx.env)?;
    if let Some(error) = params.error {
        return Err(AppError::LoginFailed(format!(
            "provider returned {error}: {}",
            params.error_description.unwrap_or_default()
        )));
    }
    let (Some(code), Some(state)) = (params.code, params.state) else {
        return Err(AppError::BadRequest("missing code or state".into()));
    };

    if cookie(&req, STATE_COOKIE).as_deref() != Some(state.as_str()) {
        return Err(AppError::LoginFailed(
            "state does not match login cookie".into(),
        ));
    }

    // Single use: the row is consumed whether or not the rest of the login succeeds.
    let Some(LoginState {
        nonce,
        pkce_verifier,
        created_at,
    }) = db.take_login_state(&state).await?
    else {
        return Err(AppError::LoginFailed(
            "unknown or already used state".into(),
        ));
    };
    if Utc::now() - created_at > Duration::minutes(LOGIN_TIMEOUT_MINUTES) {
        return Err(AppError::LoginFailed("login attempt expired".into()));
    }

    let token_response = sso
        .client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| AppError::LoginFailed(format!("provider has no token endpoint: {e}")))?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
        .request_async(&http)
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

    let claims = GuestClaims {
        issuer: claims.issuer().as_str(),
        subject,
        name: &name,
        email: email.as_deref(),
    };
    let guest = db.upsert_guest(&claims, Utc::now()).await?;

    let (token, expires_at) = create_session(&db, &guest.id, sso.config.session_ttl).await?;
    let session_cookie = set_cookie(
        SESSION_COOKIE,
        &token,
        "/",
        sso.config.session_ttl.num_seconds(),
        sso.secure_cookies(),
    );

    let resp = match &sso.config.post_login_redirect {
        Some(target) => redirect(target)?,
        None => json(
            200,
            &LoginResponse {
                guest,
                token,
                expires_at,
            },
        )?,
    };
    add_cookie(&resp, &session_cookie)?;
    add_cookie(
        &resp,
        &set_cookie(STATE_COOKIE, "", "/auth", 0, sso.secure_cookies()),
    )?;
    Ok(resp)
}

/// Ends the current session (Bearer token or cookie).
pub async fn logout(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let db = Db::from_env(&ctx.env)?;
    let token = session_token(&req).ok_or(AppError::Unauthorized)?;
    if !delete_session(&db, &token).await? {
        return Err(AppError::Unauthorized);
    }
    let resp = empty(204)?;
    add_cookie(&resp, &set_cookie(SESSION_COOKIE, "", "/", 0, false))?;
    Ok(resp)
}
