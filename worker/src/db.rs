//! Storage. Every SQL statement the service runs against D1 lives here.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use uuid::Uuid;
use worker::{D1Database, D1PreparedStatement, Env, query};

use crate::error::{AppError, is_unique_violation};
use crate::models::{Attendee, Event, Guest};

/// Column values of an event as written by create and update.
pub struct EventFields<'a> {
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub location: Option<&'a str>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: Option<DateTime<Utc>>,
    pub capacity: Option<i64>,
}

/// Identity claims from the SSO provider, upserted on every login.
pub struct GuestClaims<'a> {
    pub issuer: &'a str,
    pub subject: &'a str,
    pub name: &'a str,
    pub email: Option<&'a str>,
}

/// A consumed in-flight login.
#[derive(Deserialize)]
pub struct LoginState {
    pub nonce: String,
    pub pkce_verifier: String,
    pub created_at: DateTime<Utc>,
}

pub enum JoinOutcome {
    Joined,
    /// Nothing inserted: the event is missing or full.
    NotInserted,
    AlreadyJoined,
}

/// Timestamps are stored as text and compared as text (expiry, `upcoming`), so every bound
/// value must use this one fixed-width format.
fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

const EVENT_SELECT: &str = "SELECT e.id, e.title, e.description, e.location, \
     e.starts_at, e.ends_at, e.capacity, e.created_at, e.updated_at, \
     (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) AS attendee_count \
     FROM events e";

/// The `DB` D1 binding.
pub struct Db(D1Database);

impl Db {
    pub fn from_env(env: &Env) -> Result<Self, AppError> {
        Ok(Self(env.d1("DB")?))
    }

    pub async fn list_events(
        &self,
        limit: i64,
        offset: i64,
        starting_after: Option<DateTime<Utc>>,
    ) -> Result<Vec<Event>, AppError> {
        let stmt = match starting_after {
            Some(now) => query!(
                &self.0,
                &format!(
                    "{EVENT_SELECT} WHERE e.starts_at >= ? ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"
                ),
                ts(now),
                limit,
                offset
            )?,
            None => query!(
                &self.0,
                &format!("{EVENT_SELECT} ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"),
                limit,
                offset
            )?,
        };
        Ok(stmt.all().await?.results()?)
    }

    pub async fn event(&self, id: &str) -> Result<Option<Event>, AppError> {
        let stmt = query!(&self.0, &format!("{EVENT_SELECT} WHERE e.id = ?"), id)?;
        Ok(stmt.first(None).await?)
    }

    pub async fn insert_event(
        &self,
        id: &str,
        f: &EventFields<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        query!(
            &self.0,
            "INSERT INTO events \
             (id, title, description, location, starts_at, ends_at, capacity, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            f.title,
            f.description,
            f.location,
            ts(f.starts_at),
            f.ends_at.map(ts),
            f.capacity,
            ts(now),
            ts(now)
        )?
        .run()
        .await?;
        Ok(())
    }

    /// Returns false when the event is gone or `capacity` is below its attendee count.
    pub async fn update_event(
        &self,
        id: &str,
        f: &EventFields<'_>,
        now: DateTime<Utc>,
    ) -> Result<bool, AppError> {
        // The capacity guard is part of the UPDATE so it cannot race with concurrent joins.
        let stmt = query!(
            &self.0,
            "UPDATE events SET title = ?, description = ?, location = ?, starts_at = ?, \
             ends_at = ?, capacity = ?, updated_at = ? \
             WHERE id = ? AND (? IS NULL OR ? >= \
                 (SELECT COUNT(*) FROM attendances WHERE event_id = ?))",
            f.title,
            f.description,
            f.location,
            ts(f.starts_at),
            f.ends_at.map(ts),
            f.capacity,
            ts(now),
            id,
            f.capacity,
            f.capacity,
            id
        )?;
        changed(stmt).await
    }

    /// Deletes the event and its attendances. Returns false if it did not exist.
    pub async fn delete_event(&self, id: &str) -> Result<bool, AppError> {
        // Attendances go with it through ON DELETE CASCADE.
        changed(query!(&self.0, "DELETE FROM events WHERE id = ?", id)?).await
    }

    pub async fn attendees(&self, event_id: &str) -> Result<Vec<Attendee>, AppError> {
        let stmt = query!(
            &self.0,
            "SELECT g.id AS guest_id, g.name, a.joined_at \
             FROM attendances a JOIN guests g ON g.id = a.guest_id \
             WHERE a.event_id = ? ORDER BY a.joined_at, g.id",
            event_id
        )?;
        Ok(stmt.all().await?.results()?)
    }

    /// Adds the guest to the event if it exists and has room, atomically.
    pub async fn join(
        &self,
        event_id: &str,
        guest_id: &str,
        joined_at: DateTime<Utc>,
    ) -> Result<JoinOutcome, AppError> {
        // Single statement so the capacity check and the insert are atomic.
        let stmt = query!(
            &self.0,
            "INSERT INTO attendances (event_id, guest_id, joined_at) \
             SELECT e.id, ?, ? FROM events e \
             WHERE e.id = ? AND (e.capacity IS NULL OR \
                 (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) < e.capacity)",
            guest_id,
            ts(joined_at),
            event_id
        )?;
        match changed(stmt).await {
            Ok(true) => Ok(JoinOutcome::Joined),
            Ok(false) => Ok(JoinOutcome::NotInserted),
            Err(AppError::Database(err)) if is_unique_violation(&err) => {
                Ok(JoinOutcome::AlreadyJoined)
            }
            Err(err) => Err(err),
        }
    }

    /// Returns false if the guest was not attending.
    pub async fn leave(&self, event_id: &str, guest_id: &str) -> Result<bool, AppError> {
        let stmt = query!(
            &self.0,
            "DELETE FROM attendances WHERE event_id = ? AND guest_id = ?",
            event_id,
            guest_id
        )?;
        changed(stmt).await
    }

    pub async fn guest_events(&self, guest_id: &str) -> Result<Vec<Event>, AppError> {
        let stmt = query!(
            &self.0,
            &format!(
                "{EVENT_SELECT} JOIN attendances me ON me.event_id = e.id \
                 WHERE me.guest_id = ? ORDER BY e.starts_at, e.id"
            ),
            guest_id
        )?;
        Ok(stmt.all().await?.results()?)
    }

    /// Creates the guest on first login, otherwise refreshes name and email.
    pub async fn upsert_guest(
        &self,
        c: &GuestClaims<'_>,
        now: DateTime<Utc>,
    ) -> Result<Guest, AppError> {
        let stmt = query!(
            &self.0,
            "INSERT INTO guests (id, issuer, subject, name, email, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (issuer, subject) DO UPDATE SET \
                 name = excluded.name, email = excluded.email, updated_at = excluded.updated_at \
             RETURNING id, name, email, created_at",
            Uuid::new_v4().to_string(),
            c.issuer,
            c.subject,
            c.name,
            c.email,
            ts(now),
            ts(now)
        )?;
        stmt.first(None)
            .await?
            .ok_or_else(|| AppError::Internal("guest upsert returned no row".into()))
    }

    /// Stores a session, first purging sessions that expired before `now`.
    pub async fn insert_session(
        &self,
        token_hash: &str,
        guest_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        query!(
            &self.0,
            "DELETE FROM sessions WHERE expires_at < ?",
            ts(now)
        )?
        .run()
        .await?;
        query!(
            &self.0,
            "INSERT INTO sessions (token_hash, guest_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
            token_hash,
            guest_id,
            ts(now),
            ts(expires_at)
        )?
        .run()
        .await?;
        Ok(())
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool, AppError> {
        changed(query!(
            &self.0,
            "DELETE FROM sessions WHERE token_hash = ?",
            token_hash
        )?)
        .await
    }

    /// The guest owning an unexpired session.
    pub async fn session_guest(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Guest>, AppError> {
        let stmt = query!(
            &self.0,
            "SELECT g.id, g.name, g.email, g.created_at \
             FROM sessions s JOIN guests g ON g.id = s.guest_id \
             WHERE s.token_hash = ? AND s.expires_at > ?",
            token_hash,
            ts(now)
        )?;
        Ok(stmt.first(None).await?)
    }

    /// Stores an in-flight login, first purging those created before `stale_before`.
    pub async fn insert_login_state(
        &self,
        state: &str,
        nonce: &str,
        pkce_verifier: &str,
        now: DateTime<Utc>,
        stale_before: DateTime<Utc>,
    ) -> Result<(), AppError> {
        query!(
            &self.0,
            "DELETE FROM login_states WHERE created_at < ?",
            ts(stale_before)
        )?
        .run()
        .await?;
        query!(
            &self.0,
            "INSERT INTO login_states (state, nonce, pkce_verifier, created_at) VALUES (?, ?, ?, ?)",
            state,
            nonce,
            pkce_verifier,
            ts(now)
        )?
        .run()
        .await?;
        Ok(())
    }

    /// Removes and returns an in-flight login (single use).
    pub async fn take_login_state(&self, state: &str) -> Result<Option<LoginState>, AppError> {
        let stmt = query!(
            &self.0,
            "DELETE FROM login_states WHERE state = ? RETURNING nonce, pkce_verifier, created_at",
            state
        )?;
        Ok(stmt.first(None).await?)
    }
}

/// Runs a write and reports whether it touched any row.
async fn changed(stmt: D1PreparedStatement) -> Result<bool, AppError> {
    let meta = stmt.run().await?.meta()?;
    Ok(meta.and_then(|m| m.changes).unwrap_or(0) > 0)
}
