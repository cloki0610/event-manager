use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{EventFields, GuestClaims, JoinOutcome, LoginState};
use crate::error::{AppError, is_unique_violation};
use crate::models::{Attendee, Event, Guest};

const EVENT_SELECT: &str = "SELECT e.id, e.title, e.description, e.location, \
     e.starts_at, e.ends_at, e.capacity, e.created_at, e.updated_at, \
     (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) AS attendee_count \
     FROM events e";

pub async fn list_events(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
    starting_after: Option<DateTime<Utc>>,
) -> Result<Vec<Event>, AppError> {
    let events = match starting_after {
        Some(now) => {
            sqlx::query_as::<_, Event>(&format!(
                "{EVENT_SELECT} WHERE e.starts_at >= ? ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"
            ))
            .bind(now)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, Event>(&format!(
                "{EVENT_SELECT} ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"
            ))
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(events)
}

pub async fn event(pool: &SqlitePool, id: &str) -> Result<Option<Event>, AppError> {
    Ok(sqlx::query_as::<_, Event>(&format!("{EVENT_SELECT} WHERE e.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn insert_event(
    pool: &SqlitePool,
    id: &str,
    f: &EventFields<'_>,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO events \
         (id, title, description, location, starts_at, ends_at, capacity, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(f.title)
    .bind(f.description)
    .bind(f.location)
    .bind(f.starts_at)
    .bind(f.ends_at)
    .bind(f.capacity)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_event(
    pool: &SqlitePool,
    id: &str,
    f: &EventFields<'_>,
    now: DateTime<Utc>,
) -> Result<bool, AppError> {
    // The capacity guard is part of the UPDATE so it cannot race with concurrent joins.
    let result = sqlx::query(
        "UPDATE events SET title = ?, description = ?, location = ?, starts_at = ?, \
         ends_at = ?, capacity = ?, updated_at = ? \
         WHERE id = ? AND (? IS NULL OR ? >= \
             (SELECT COUNT(*) FROM attendances WHERE event_id = ?))",
    )
    .bind(f.title)
    .bind(f.description)
    .bind(f.location)
    .bind(f.starts_at)
    .bind(f.ends_at)
    .bind(f.capacity)
    .bind(now)
    .bind(id)
    .bind(f.capacity)
    .bind(f.capacity)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_event(pool: &SqlitePool, id: &str) -> Result<bool, AppError> {
    // Attendances go with it through ON DELETE CASCADE.
    let result = sqlx::query("DELETE FROM events WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn attendees(pool: &SqlitePool, event_id: &str) -> Result<Vec<Attendee>, AppError> {
    Ok(sqlx::query_as::<_, Attendee>(
        "SELECT g.id AS guest_id, g.name, a.joined_at \
         FROM attendances a JOIN guests g ON g.id = a.guest_id \
         WHERE a.event_id = ? ORDER BY a.joined_at, g.id",
    )
    .bind(event_id)
    .fetch_all(pool)
    .await?)
}

pub async fn join(
    pool: &SqlitePool,
    event_id: &str,
    guest_id: &str,
    joined_at: DateTime<Utc>,
) -> Result<JoinOutcome, AppError> {
    // Single statement so the capacity check and the insert are atomic.
    let result = sqlx::query(
        "INSERT INTO attendances (event_id, guest_id, joined_at) \
         SELECT e.id, ?, ? FROM events e \
         WHERE e.id = ? AND (e.capacity IS NULL OR \
             (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) < e.capacity)",
    )
    .bind(guest_id)
    .bind(joined_at)
    .bind(event_id)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 1 => Ok(JoinOutcome::Joined),
        Ok(_) => Ok(JoinOutcome::NotInserted),
        Err(err) if is_unique_violation(&err) => Ok(JoinOutcome::AlreadyJoined),
        Err(err) => Err(err.into()),
    }
}

pub async fn leave(pool: &SqlitePool, event_id: &str, guest_id: &str) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM attendances WHERE event_id = ? AND guest_id = ?")
        .bind(event_id)
        .bind(guest_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn guest_events(pool: &SqlitePool, guest_id: &str) -> Result<Vec<Event>, AppError> {
    Ok(sqlx::query_as::<_, Event>(&format!(
        "{EVENT_SELECT} JOIN attendances me ON me.event_id = e.id \
         WHERE me.guest_id = ? ORDER BY e.starts_at, e.id"
    ))
    .bind(guest_id)
    .fetch_all(pool)
    .await?)
}

pub async fn upsert_guest(
    pool: &SqlitePool,
    c: &GuestClaims<'_>,
    now: DateTime<Utc>,
) -> Result<Guest, AppError> {
    Ok(sqlx::query_as::<_, Guest>(
        "INSERT INTO guests (id, issuer, subject, name, email, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (issuer, subject) DO UPDATE SET \
             name = excluded.name, email = excluded.email, updated_at = excluded.updated_at \
         RETURNING id, name, email, created_at",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(c.issuer)
    .bind(c.subject)
    .bind(c.name)
    .bind(c.email)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?)
}

pub async fn insert_session(
    pool: &SqlitePool,
    token_hash: &str,
    guest_id: &str,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, guest_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(token_hash)
    .bind(guest_id)
    .bind(now)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_session(pool: &SqlitePool, token_hash: &str) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn session_guest(
    pool: &SqlitePool,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<Guest>, AppError> {
    Ok(sqlx::query_as::<_, Guest>(
        "SELECT g.id, g.name, g.email, g.created_at \
         FROM sessions s JOIN guests g ON g.id = s.guest_id \
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?)
}

pub async fn insert_login_state(
    pool: &SqlitePool,
    state: &str,
    nonce: &str,
    pkce_verifier: &str,
    now: DateTime<Utc>,
    stale_before: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM login_states WHERE created_at < ?")
        .bind(stale_before)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO login_states (state, nonce, pkce_verifier, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(state)
    .bind(nonce)
    .bind(pkce_verifier)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn take_login_state(
    pool: &SqlitePool,
    state: &str,
) -> Result<Option<LoginState>, AppError> {
    Ok(sqlx::query_as(
        "DELETE FROM login_states WHERE state = ? RETURNING nonce, pkce_verifier, created_at",
    )
    .bind(state)
    .fetch_optional(pool)
    .await?)
}
