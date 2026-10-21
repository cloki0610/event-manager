//! DuckDB backend, enabled with the `duckdb` cargo feature.
//!
//! DuckDB has no `ON DELETE CASCADE`, so the schema has no foreign keys and deletes cascade by
//! hand inside a transaction. The driver is synchronous: every call runs on the blocking pool
//! behind one mutex, which also makes each check-then-write below atomic.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use duckdb::{Connection, OptionalExt, Row, params};
use uuid::Uuid;

use super::{EventFields, GuestClaims, JoinOutcome, LoginState};
use crate::error::AppError;
use crate::models::{Attendee, Event, Guest};

const SCHEMA: &str = include_str!("duckdb_schema.sql");

const EVENT_SELECT: &str = "SELECT e.id, e.title, e.description, e.location, \
     e.starts_at, e.ends_at, e.capacity, e.created_at, e.updated_at, \
     (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) AS attendee_count \
     FROM events e";

pub struct DuckDb {
    conn: Arc<Mutex<Connection>>,
}

fn event_row(row: &Row<'_>) -> duckdb::Result<Event> {
    Ok(Event {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        location: row.get(3)?,
        starts_at: row.get(4)?,
        ends_at: row.get(5)?,
        capacity: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        attendee_count: row.get(9)?,
    })
}

fn guest_row(row: &Row<'_>) -> duckdb::Result<Guest> {
    Ok(Guest { id: row.get(0)?, name: row.get(1)?, email: row.get(2)?, created_at: row.get(3)? })
}

fn is_unique_violation(err: &duckdb::Error) -> bool {
    matches!(err, duckdb::Error::DuckDBFailure(e, msg)
        if e.code == duckdb::ErrorCode::ConstraintViolation
            || msg.as_deref().is_some_and(|m| m.contains("Duplicate key")))
}

impl DuckDb {
    /// Opens `path` (`:memory:` for an in-memory database) and creates missing tables.
    pub async fn open(path: &str) -> Result<Self, AppError> {
        let path = path.to_string();
        let conn = actix_web::web::block(move || -> duckdb::Result<Connection> {
            let conn = if path == ":memory:" {
                Connection::open_in_memory()?
            } else {
                Connection::open(&path)?
            };
            conn.execute_batch(SCHEMA)?;
            Ok(conn)
        })
        .await
        .map_err(|e| AppError::Blocking(e.to_string()))??;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    async fn run<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    {
        let conn = self.conn.clone();
        actix_web::web::block(move || f(&mut conn.lock().unwrap_or_else(|e| e.into_inner())))
            .await
            .map_err(|e| AppError::Blocking(e.to_string()))?
    }

    pub async fn list_events(
        &self,
        limit: i64,
        offset: i64,
        starting_after: Option<DateTime<Utc>>,
    ) -> Result<Vec<Event>, AppError> {
        self.run(move |c| {
            let events = match starting_after {
                Some(now) => c
                    .prepare(&format!(
                        "{EVENT_SELECT} WHERE e.starts_at >= CAST(? AS TIMESTAMP) \
                         ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"
                    ))?
                    .query_map(params![now, limit, offset], event_row)?
                    .collect::<duckdb::Result<_>>()?,
                None => c
                    .prepare(&format!("{EVENT_SELECT} ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"))?
                    .query_map(params![limit, offset], event_row)?
                    .collect::<duckdb::Result<_>>()?,
            };
            Ok(events)
        })
        .await
    }

    pub async fn event(&self, id: &str) -> Result<Option<Event>, AppError> {
        let id = id.to_string();
        self.run(move |c| {
            Ok(c.query_row(&format!("{EVENT_SELECT} WHERE e.id = ?"), [id], event_row)
                .optional()?)
        })
        .await
    }

    pub async fn insert_event(
        &self,
        id: &str,
        f: &EventFields<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let id = id.to_string();
        let (title, description, location) = owned_text(f);
        let (starts_at, ends_at, capacity) = (f.starts_at, f.ends_at, f.capacity);
        self.run(move |c| {
            c.execute(
                "INSERT INTO events \
                 (id, title, description, location, starts_at, ends_at, capacity, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![id, title, description, location, starts_at, ends_at, capacity, now, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn update_event(
        &self,
        id: &str,
        f: &EventFields<'_>,
        now: DateTime<Utc>,
    ) -> Result<bool, AppError> {
        let id = id.to_string();
        let (title, description, location) = owned_text(f);
        let (starts_at, ends_at, capacity) = (f.starts_at, f.ends_at, f.capacity);
        self.run(move |c| {
            let attendees: i64 = c.query_row(
                "SELECT COUNT(*) FROM attendances WHERE event_id = ?",
                [&id],
                |r| r.get(0),
            )?;
            if capacity.is_some_and(|cap| cap < attendees) {
                return Ok(false);
            }
            let n = c.execute(
                "UPDATE events SET title = ?, description = ?, location = ?, starts_at = ?, \
                 ends_at = ?, capacity = ?, updated_at = ? WHERE id = ?",
                params![title, description, location, starts_at, ends_at, capacity, now, id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    pub async fn delete_event(&self, id: &str) -> Result<bool, AppError> {
        let id = id.to_string();
        self.run(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM attendances WHERE event_id = ?", [&id])?;
            let n = tx.execute("DELETE FROM events WHERE id = ?", [&id])?;
            tx.commit()?;
            Ok(n > 0)
        })
        .await
    }

    pub async fn attendees(&self, event_id: &str) -> Result<Vec<Attendee>, AppError> {
        let event_id = event_id.to_string();
        self.run(move |c| {
            Ok(c.prepare(
                "SELECT g.id AS guest_id, g.name, a.joined_at \
                 FROM attendances a JOIN guests g ON g.id = a.guest_id \
                 WHERE a.event_id = ? ORDER BY a.joined_at, g.id",
            )?
            .query_map([event_id], |r| {
                Ok(Attendee { guest_id: r.get(0)?, name: r.get(1)?, joined_at: r.get(2)? })
            })?
            .collect::<duckdb::Result<_>>()?)
        })
        .await
    }

    pub async fn join(
        &self,
        event_id: &str,
        guest_id: &str,
        joined_at: DateTime<Utc>,
    ) -> Result<JoinOutcome, AppError> {
        let (event_id, guest_id) = (event_id.to_string(), guest_id.to_string());
        self.run(move |c| {
            let result = c.execute(
                "INSERT INTO attendances (event_id, guest_id, joined_at) \
                 SELECT e.id, CAST(? AS VARCHAR), CAST(? AS TIMESTAMP) FROM events e \
                 WHERE e.id = ? AND (e.capacity IS NULL OR \
                     (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) < e.capacity)",
                params![guest_id, joined_at, event_id],
            );
            match result {
                Ok(1) => Ok(JoinOutcome::Joined),
                Ok(_) => Ok(JoinOutcome::NotInserted),
                Err(err) if is_unique_violation(&err) => Ok(JoinOutcome::AlreadyJoined),
                Err(err) => Err(err.into()),
            }
        })
        .await
    }

    pub async fn leave(&self, event_id: &str, guest_id: &str) -> Result<bool, AppError> {
        let (event_id, guest_id) = (event_id.to_string(), guest_id.to_string());
        self.run(move |c| {
            let n = c.execute(
                "DELETE FROM attendances WHERE event_id = ? AND guest_id = ?",
                [event_id, guest_id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    pub async fn guest_events(&self, guest_id: &str) -> Result<Vec<Event>, AppError> {
        let guest_id = guest_id.to_string();
        self.run(move |c| {
            Ok(c.prepare(&format!(
                "{EVENT_SELECT} JOIN attendances me ON me.event_id = e.id \
                 WHERE me.guest_id = ? ORDER BY e.starts_at, e.id"
            ))?
            .query_map([guest_id], event_row)?
            .collect::<duckdb::Result<_>>()?)
        })
        .await
    }

    pub async fn upsert_guest(
        &self,
        claims: &GuestClaims<'_>,
        now: DateTime<Utc>,
    ) -> Result<Guest, AppError> {
        let issuer = claims.issuer.to_string();
        let subject = claims.subject.to_string();
        let name = claims.name.to_string();
        let email = claims.email.map(str::to_string);
        self.run(move |c| {
            let tx = c.transaction()?;
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM guests WHERE issuer = ? AND subject = ?",
                    [&issuer, &subject],
                    |r| r.get(0),
                )
                .optional()?;
            let id = match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE guests SET name = ?, email = ?, updated_at = ? WHERE id = ?",
                        params![name, email, now, id],
                    )?;
                    id
                }
                None => {
                    let id = Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO guests (id, issuer, subject, name, email, created_at, updated_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                        params![id, issuer, subject, name, email, now, now],
                    )?;
                    id
                }
            };
            let guest = tx.query_row(
                "SELECT id, name, email, created_at FROM guests WHERE id = ?",
                [&id],
                guest_row,
            )?;
            tx.commit()?;
            Ok(guest)
        })
        .await
    }

    pub async fn insert_session(
        &self,
        token_hash: &str,
        guest_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let (token_hash, guest_id) = (token_hash.to_string(), guest_id.to_string());
        self.run(move |c| {
            c.execute("DELETE FROM sessions WHERE expires_at < CAST(? AS TIMESTAMP)", params![now])?;
            c.execute(
                "INSERT INTO sessions (token_hash, guest_id, created_at, expires_at) \
                 VALUES (?, ?, ?, ?)",
                params![token_hash, guest_id, now, expires_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool, AppError> {
        let token_hash = token_hash.to_string();
        self.run(move |c| {
            Ok(c.execute("DELETE FROM sessions WHERE token_hash = ?", [token_hash])? > 0)
        })
        .await
    }

    pub async fn session_guest(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Guest>, AppError> {
        let token_hash = token_hash.to_string();
        self.run(move |c| {
            Ok(c.query_row(
                "SELECT g.id, g.name, g.email, g.created_at \
                 FROM sessions s JOIN guests g ON g.id = s.guest_id \
                 WHERE s.token_hash = ? AND s.expires_at > CAST(? AS TIMESTAMP)",
                params![token_hash, now],
                guest_row,
            )
            .optional()?)
        })
        .await
    }

    pub async fn insert_login_state(
        &self,
        state: &str,
        nonce: &str,
        pkce_verifier: &str,
        now: DateTime<Utc>,
        stale_before: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let (state, nonce, verifier) =
            (state.to_string(), nonce.to_string(), pkce_verifier.to_string());
        self.run(move |c| {
            c.execute(
                "DELETE FROM login_states WHERE created_at < CAST(? AS TIMESTAMP)",
                params![stale_before],
            )?;
            c.execute(
                "INSERT INTO login_states (state, nonce, pkce_verifier, created_at) \
                 VALUES (?, ?, ?, ?)",
                params![state, nonce, verifier, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn take_login_state(&self, state: &str) -> Result<Option<LoginState>, AppError> {
        let state = state.to_string();
        self.run(move |c| {
            let tx = c.transaction()?;
            let pending = tx
                .query_row(
                    "SELECT nonce, pkce_verifier, created_at FROM login_states WHERE state = ?",
                    [&state],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            tx.execute("DELETE FROM login_states WHERE state = ?", [&state])?;
            tx.commit()?;
            Ok(pending)
        })
        .await
    }
}

fn owned_text(f: &EventFields<'_>) -> (String, Option<String>, Option<String>) {
    (f.title.to_string(), f.description.map(str::to_string), f.location.map(str::to_string))
}
