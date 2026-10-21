//! Storage backends. Handlers talk to [`Db`]; each backend owns its SQL.

#[cfg(feature = "duckdb")]
pub mod duckdb;
pub mod sqlite;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::error::AppError;
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

/// A consumed in-flight login: `(nonce, pkce_verifier, created_at)`.
pub type LoginState = (String, String, DateTime<Utc>);

pub enum JoinOutcome {
    Joined,
    /// Nothing inserted: the event is missing or full.
    NotInserted,
    AlreadyJoined,
}

pub enum Db {
    Sqlite(SqlitePool),
    #[cfg(feature = "duckdb")]
    DuckDb(duckdb::DuckDb),
}

impl Db {
    /// Opens the database named by `url` and brings its schema up to date.
    /// `duckdb://<path>` (or `duckdb://:memory:`) selects DuckDB; anything else is SQLite.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, String> {
        if let Some(path) = url.strip_prefix("duckdb://") {
            #[cfg(feature = "duckdb")]
            return duckdb::DuckDb::open(path).await.map(Db::DuckDb).map_err(|e| e.to_string());
            #[cfg(not(feature = "duckdb"))]
            return Err(format!(
                "cannot open duckdb://{path}: built without the `duckdb` feature \
                 (cargo build --features duckdb)"
            ));
        }
        crate::init_db(url, max_connections)
            .await
            .map(Db::Sqlite)
            .map_err(|e| e.to_string())
    }

    pub async fn list_events(
        &self,
        limit: i64,
        offset: i64,
        starting_after: Option<DateTime<Utc>>,
    ) -> Result<Vec<Event>, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::list_events(p, limit, offset, starting_after).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.list_events(limit, offset, starting_after).await,
        }
    }

    pub async fn event(&self, id: &str) -> Result<Option<Event>, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::event(p, id).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.event(id).await,
        }
    }

    pub async fn insert_event(
        &self,
        id: &str,
        fields: &EventFields<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        match self {
            Db::Sqlite(p) => sqlite::insert_event(p, id, fields, now).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.insert_event(id, fields, now).await,
        }
    }

    /// Returns false when the event is gone or `capacity` is below its attendee count.
    pub async fn update_event(
        &self,
        id: &str,
        fields: &EventFields<'_>,
        now: DateTime<Utc>,
    ) -> Result<bool, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::update_event(p, id, fields, now).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.update_event(id, fields, now).await,
        }
    }

    /// Deletes the event and its attendances. Returns false if it did not exist.
    pub async fn delete_event(&self, id: &str) -> Result<bool, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::delete_event(p, id).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.delete_event(id).await,
        }
    }

    pub async fn attendees(&self, event_id: &str) -> Result<Vec<Attendee>, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::attendees(p, event_id).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.attendees(event_id).await,
        }
    }

    /// Adds the guest to the event if it exists and has room, atomically.
    pub async fn join(
        &self,
        event_id: &str,
        guest_id: &str,
        joined_at: DateTime<Utc>,
    ) -> Result<JoinOutcome, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::join(p, event_id, guest_id, joined_at).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.join(event_id, guest_id, joined_at).await,
        }
    }

    /// Returns false if the guest was not attending.
    pub async fn leave(&self, event_id: &str, guest_id: &str) -> Result<bool, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::leave(p, event_id, guest_id).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.leave(event_id, guest_id).await,
        }
    }

    pub async fn guest_events(&self, guest_id: &str) -> Result<Vec<Event>, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::guest_events(p, guest_id).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.guest_events(guest_id).await,
        }
    }

    /// Creates the guest on first login, otherwise refreshes name and email.
    pub async fn upsert_guest(
        &self,
        claims: &GuestClaims<'_>,
        now: DateTime<Utc>,
    ) -> Result<Guest, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::upsert_guest(p, claims, now).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.upsert_guest(claims, now).await,
        }
    }

    /// Stores a session, first purging sessions that expired before `now`.
    pub async fn insert_session(
        &self,
        token_hash: &str,
        guest_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        match self {
            Db::Sqlite(p) => sqlite::insert_session(p, token_hash, guest_id, now, expires_at).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.insert_session(token_hash, guest_id, now, expires_at).await,
        }
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::delete_session(p, token_hash).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.delete_session(token_hash).await,
        }
    }

    /// The guest owning an unexpired session.
    pub async fn session_guest(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Guest>, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::session_guest(p, token_hash, now).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.session_guest(token_hash, now).await,
        }
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
        match self {
            Db::Sqlite(p) => {
                sqlite::insert_login_state(p, state, nonce, pkce_verifier, now, stale_before).await
            }
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.insert_login_state(state, nonce, pkce_verifier, now, stale_before).await,
        }
    }

    /// Removes and returns an in-flight login (single use).
    pub async fn take_login_state(&self, state: &str) -> Result<Option<LoginState>, AppError> {
        match self {
            Db::Sqlite(p) => sqlite::take_login_state(p, state).await,
            #[cfg(feature = "duckdb")]
            Db::DuckDb(d) => d.take_login_state(state).await,
        }
    }
}
