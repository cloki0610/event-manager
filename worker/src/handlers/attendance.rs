use chrono::Utc;
use worker::{Request, Response};

use super::events::fetch_event;
use super::{Ctx, empty, event_id, json};
use crate::auth::authenticate;
use crate::db::{Db, JoinOutcome};
use crate::error::AppError;
use crate::models::Attendance;

pub async fn attendees(_req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let id = event_id(&ctx)?;
    let db = Db::from_env(&ctx.env)?;
    let event = fetch_event(&db, &id).await?;
    json(200, &db.attendees(&event.id).await?)
}

pub async fn join(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let event_id = event_id(&ctx)?;
    let db = Db::from_env(&ctx.env)?;
    let guest = authenticate(&req, &db).await?;
    let joined_at = Utc::now();

    match db.join(&event_id, &guest.id, joined_at).await? {
        JoinOutcome::Joined => json(
            201,
            &Attendance {
                event_id,
                guest_id: guest.id,
                joined_at,
            },
        ),
        JoinOutcome::NotInserted => {
            // Nothing inserted: the event is missing (404 from fetch_event) or full.
            fetch_event(&db, &event_id).await?;
            Err(AppError::Conflict("event is full".into()))
        }
        JoinOutcome::AlreadyJoined => Err(AppError::Conflict("already joined this event".into())),
    }
}

pub async fn leave(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let id = event_id(&ctx)?;
    let db = Db::from_env(&ctx.env)?;
    let guest = authenticate(&req, &db).await?;
    let event = fetch_event(&db, &id).await?;

    if !db.leave(&event.id, &guest.id).await? {
        return Err(AppError::NotFound("attendance"));
    }
    empty(204)
}
