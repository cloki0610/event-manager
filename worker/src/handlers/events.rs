use chrono::{DateTime, Utc};
use uuid::Uuid;
use worker::{Request, Response};

use super::{Ctx, empty, event_id, json, json_body, query};
use crate::db::{Db, EventFields};
use crate::error::{AppError, internal};
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
        return Err(AppError::BadRequest(
            "ends_at must not be before starts_at".into(),
        ));
    }
    if capacity.is_some_and(|c| c < 1) {
        return Err(AppError::BadRequest("capacity must be at least 1".into()));
    }
    Ok(())
}

pub async fn list(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let params: ListParams = query(&req)?;
    let db = Db::from_env(&ctx.env)?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);
    let starting_after = params.upcoming.then(Utc::now);

    let events = db.list_events(limit, offset, starting_after).await?;
    json(200, &events)
}

pub async fn get(_req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let id = event_id(&ctx)?;
    let db = Db::from_env(&ctx.env)?;
    json(200, &fetch_event(&db, &id).await?)
}

pub async fn create(mut req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let input: CreateEvent = json_body(&mut req).await?;
    let db = Db::from_env(&ctx.env)?;
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
    let resp = json(201, &event)?;
    resp.headers()
        .set("location", &format!("/events/{id}"))
        .map_err(internal)?;
    Ok(resp)
}

pub async fn update(mut req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let id = event_id(&ctx)?;
    let patch: UpdateEvent = json_body(&mut req).await?;
    let db = Db::from_env(&ctx.env)?;
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

    json(200, &fetch_event(&db, &id).await?)
}

pub async fn delete(_req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let id = event_id(&ctx)?;
    let db = Db::from_env(&ctx.env)?;
    if !db.delete_event(&id).await? {
        return Err(AppError::NotFound("event"));
    }
    empty(204)
}
