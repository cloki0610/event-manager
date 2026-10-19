use actix_web::HttpRequest;
use actix_web::http::header;
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::Guest;

/// Cookie carrying the session token for browser clients.
pub const SESSION_COOKIE: &str = "session";

/// Generates a new random session token from two v4 UUIDs (244 random bits).
pub fn generate_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Tokens are stored hashed so a database leak does not expose live sessions.
pub fn hash_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

/// Starts a session for `guest_id` and returns its token and expiry.
pub async fn create_session(
    pool: &SqlitePool,
    guest_id: &str,
    ttl: Duration,
) -> Result<(String, DateTime<Utc>), AppError> {
    let token = generate_token();
    let now = Utc::now();
    let expires_at = now + ttl;

    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, guest_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(hash_token(&token))
    .bind(guest_id)
    .bind(now)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok((token, expires_at))
}

/// Ends the session identified by `token`. Returns false if it did not exist.
pub async fn delete_session(pool: &SqlitePool, token: &str) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(hash_token(token))
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Extracts the session token from `Authorization: Bearer <token>` or the session cookie.
pub fn session_token(req: &HttpRequest) -> Option<String> {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    bearer.or_else(|| {
        req.cookie(SESSION_COOKIE)
            .map(|c| c.value().to_string())
            .filter(|t| !t.is_empty())
    })
}

/// Resolves the guest owning the request's (unexpired) session.
pub async fn authenticate(req: &HttpRequest, pool: &SqlitePool) -> Result<Guest, AppError> {
    let token = session_token(req).ok_or(AppError::Unauthorized)?;

    sqlx::query_as::<_, Guest>(
        "SELECT g.id, g.name, g.email, g.created_at \
         FROM sessions s JOIN guests g ON g.id = s.guest_id \
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(hash_token(&token))
    .bind(Utc::now())
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::Unauthorized)
}
