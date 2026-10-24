use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use worker::Request;

use crate::db::Db;
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
    db: &Db,
    guest_id: &str,
    ttl: Duration,
) -> Result<(String, DateTime<Utc>), AppError> {
    let token = generate_token();
    let now = Utc::now();
    let expires_at = now + ttl;
    db.insert_session(&hash_token(&token), guest_id, now, expires_at)
        .await?;
    Ok((token, expires_at))
}

/// Ends the session identified by `token`. Returns false if it did not exist.
pub async fn delete_session(db: &Db, token: &str) -> Result<bool, AppError> {
    db.delete_session(&hash_token(token)).await
}

/// The value of the request cookie `name`.
pub fn cookie(req: &Request, name: &str) -> Option<String> {
    let header = req.headers().get("cookie").ok().flatten()?;
    header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.to_string())
}

/// Extracts the session token from `Authorization: Bearer <token>` or the session cookie.
pub fn session_token(req: &Request) -> Option<String> {
    let bearer = req
        .headers()
        .get("authorization")
        .ok()
        .flatten()
        .and_then(|v| v.strip_prefix("Bearer ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty());
    bearer.or_else(|| cookie(req, SESSION_COOKIE).filter(|t| !t.is_empty()))
}

/// Resolves the guest owning the request's (unexpired) session.
pub async fn authenticate(req: &Request, db: &Db) -> Result<Guest, AppError> {
    let token = session_token(req).ok_or(AppError::Unauthorized)?;
    db.session_guest(&hash_token(&token), Utc::now())
        .await?
        .ok_or(AppError::Unauthorized)
}
