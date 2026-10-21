use actix_web::{HttpResponse, web};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::{Db, EventFields};
use crate::error::AppError;
use crate::models::{CreateEvent, Event, ListParams, UpdateEvent};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;

pub(crate) async fn fetch_event(db: &Db, id: &str) -> Result<Event, AppError> {
    db.event(id).await?.ok_or(AppError::NotFound("event"))
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
    db: web::Data<Db>,
    params: web::Query<ListParams>,
) -> Result<HttpResponse, AppError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);
    let starting_after = params.upcoming.then(Utc::now);

    let events = db.list_events(limit, offset, starting_after).await?;
    Ok(HttpResponse::Ok().json(events))
}

pub async fn get(db: web::Data<Db>, id: web::Path<Uuid>) -> Result<HttpResponse, AppError> {
    let event = fetch_event(&db, &id.to_string()).await?;
    Ok(HttpResponse::Ok().json(event))
}

pub async fn create(
    db: web::Data<Db>,
    body: web::Json<CreateEvent>,
) -> Result<HttpResponse, AppError> {
    let input = body.into_inner();
    validate(&input.title, input.starts_at, input.ends_at, input.capacity)?;

    let id = Uuid::new_v4().to_string();
    let fields = EventFields {
        title: input.title.trim(),
        description: input.description.as_deref(),
        location: input.location.as_deref(),
        starts_at: input.starts_at,
        ends_at: input.ends_at,
        capacity: input.capacity,
    };
    db.insert_event(&id, &fields, Utc::now()).await?;

    let event = fetch_event(&db, &id).await?;
    Ok(HttpResponse::Created()
        .insert_header(("Location", format!("/events/{id}")))
        .json(event))
}

pub async fn update(
    db: web::Data<Db>,
    id: web::Path<Uuid>,
    body: web::Json<UpdateEvent>,
) -> Result<HttpResponse, AppError> {
    let id = id.to_string();
    let patch = body.into_inner();
    let current = fetch_event(&db, &id).await?;

    let title = patch.title.unwrap_or(current.title);
    let description = patch.description.unwrap_or(current.description);
    let location = patch.location.unwrap_or(current.location);
    let starts_at = patch.starts_at.unwrap_or(current.starts_at);
    let ends_at = patch.ends_at.unwrap_or(current.ends_at);
    let capacity = patch.capacity.unwrap_or(current.capacity);
    validate(&title, starts_at, ends_at, capacity)?;

    let fields = EventFields {
        title: title.trim(),
        description: description.as_deref(),
        location: location.as_deref(),
        starts_at,
        ends_at,
        capacity,
    };
    if !db.update_event(&id, &fields, Utc::now()).await? {
        // Either the event was deleted meanwhile, or the new capacity is too small.
        let event = fetch_event(&db, &id).await?;
        return Err(AppError::Conflict(format!(
            "capacity cannot be lower than the current attendee count ({})",
            event.attendee_count
        )));
    }

    let event = fetch_event(&db, &id).await?;
    Ok(HttpResponse::Ok().json(event))
}

pub async fn delete(db: web::Data<Db>, id: web::Path<Uuid>) -> Result<HttpResponse, AppError> {
    if !db.delete_event(&id.to_string()).await? {
        return Err(AppError::NotFound("event"));
    }
    Ok(HttpResponse::NoContent().finish())
}
