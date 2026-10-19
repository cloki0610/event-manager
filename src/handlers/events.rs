use actix_web::{HttpResponse, web};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{CreateEvent, Event, ListParams, UpdateEvent};

pub(crate) const EVENT_SELECT: &str = "SELECT e.id, e.title, e.description, e.location, \
     e.starts_at, e.ends_at, e.capacity, e.created_at, e.updated_at, \
     (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) AS attendee_count \
     FROM events e";

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;

pub(crate) async fn fetch_event(pool: &SqlitePool, id: &str) -> Result<Event, AppError> {
    sqlx::query_as::<_, Event>(&format!("{EVENT_SELECT} WHERE e.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("event"))
}

fn validate(
    title: &str,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    capacity: Option<i64>,
) -> Result<(), AppError> {
    if title.trim().is_empty() {
        return Err(AppError::BadRequest("title must not be empty".into()));
    }
    if ends_at.is_some_and(|end| end < starts_at) {
        return Err(AppError::BadRequest("ends_at must not be before starts_at".into()));
    }
    if capacity.is_some_and(|c| c < 1) {
        return Err(AppError::BadRequest("capacity must be at least 1".into()));
    }
    Ok(())
}

pub async fn list(
    pool: web::Data<SqlitePool>,
    params: web::Query<ListParams>,
) -> Result<HttpResponse, AppError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    let events = if params.upcoming {
        sqlx::query_as::<_, Event>(&format!(
            "{EVENT_SELECT} WHERE e.starts_at >= ? ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"
        ))
        .bind(Utc::now())
        .bind(limit)
        .bind(offset)
        .fetch_all(pool.get_ref())
        .await?
    } else {
        sqlx::query_as::<_, Event>(&format!(
            "{EVENT_SELECT} ORDER BY e.starts_at, e.id LIMIT ? OFFSET ?"
        ))
        .bind(limit)
        .bind(offset)
        .fetch_all(pool.get_ref())
        .await?
    };

    Ok(HttpResponse::Ok().json(events))
}

pub async fn get(
    pool: web::Data<SqlitePool>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let event = fetch_event(&pool, &id.to_string()).await?;
    Ok(HttpResponse::Ok().json(event))
}

pub async fn create(
    pool: web::Data<SqlitePool>,
    body: web::Json<CreateEvent>,
) -> Result<HttpResponse, AppError> {
    let input = body.into_inner();
    validate(&input.title, input.starts_at, input.ends_at, input.capacity)?;

    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO events \
         (id, title, description, location, starts_at, ends_at, capacity, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(input.title.trim())
    .bind(&input.description)
    .bind(&input.location)
    .bind(input.starts_at)
    .bind(input.ends_at)
    .bind(input.capacity)
    .bind(now)
    .bind(now)
    .execute(pool.get_ref())
    .await?;

    let event = fetch_event(&pool, &id).await?;
    Ok(HttpResponse::Created()
        .insert_header(("Location", format!("/events/{id}")))
        .json(event))
}

pub async fn update(
    pool: web::Data<SqlitePool>,
    id: web::Path<Uuid>,
    body: web::Json<UpdateEvent>,
) -> Result<HttpResponse, AppError> {
    let id = id.to_string();
    let patch = body.into_inner();
    let current = fetch_event(&pool, &id).await?;

    let title = patch.title.unwrap_or(current.title);
    let description = patch.description.unwrap_or(current.description);
    let location = patch.location.unwrap_or(current.location);
    let starts_at = patch.starts_at.unwrap_or(current.starts_at);
    let ends_at = patch.ends_at.unwrap_or(current.ends_at);
    let capacity = patch.capacity.unwrap_or(current.capacity);
    validate(&title, starts_at, ends_at, capacity)?;

    // The capacity guard is part of the UPDATE so it cannot race with concurrent joins.
    let result = sqlx::query(
        "UPDATE events SET title = ?, description = ?, location = ?, starts_at = ?, \
         ends_at = ?, capacity = ?, updated_at = ? \
         WHERE id = ? AND (? IS NULL OR ? >= \
             (SELECT COUNT(*) FROM attendances WHERE event_id = ?))",
    )
    .bind(title.trim())
    .bind(&description)
    .bind(&location)
    .bind(starts_at)
    .bind(ends_at)
    .bind(capacity)
    .bind(Utc::now())
    .bind(&id)
    .bind(capacity)
    .bind(capacity)
    .bind(&id)
    .execute(pool.get_ref())
    .await?;

    if result.rows_affected() == 0 {
        // Either the event was deleted meanwhile, or the new capacity is too small.
        let event = fetch_event(&pool, &id).await?;
        return Err(AppError::Conflict(format!(
            "capacity cannot be lower than the current attendee count ({})",
            event.attendee_count
        )));
    }

    let event = fetch_event(&pool, &id).await?;
    Ok(HttpResponse::Ok().json(event))
}

pub async fn delete(
    pool: web::Data<SqlitePool>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let result = sqlx::query("DELETE FROM events WHERE id = ?")
        .bind(id.to_string())
        .execute(pool.get_ref())
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("event"));
    }
    Ok(HttpResponse::NoContent().finish())
}
