use actix_web::{HttpRequest, HttpResponse, web};
use chrono::Utc;
use uuid::Uuid;

use crate::auth::authenticate;
use crate::db::{Db, JoinOutcome};
use crate::error::AppError;
use crate::handlers::events::fetch_event;
use crate::models::Attendance;

pub async fn attendees(db: web::Data<Db>, id: web::Path<Uuid>) -> Result<HttpResponse, AppError> {
    let event = fetch_event(&db, &id.to_string()).await?;
    let attendees = db.attendees(&event.id).await?;
    Ok(HttpResponse::Ok().json(attendees))
}

pub async fn join(
    req: HttpRequest,
    db: web::Data<Db>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &db).await?;
    let event_id = id.to_string();
    let joined_at = Utc::now();

    match db.join(&event_id, &guest.id, joined_at).await? {
        JoinOutcome::Joined => Ok(HttpResponse::Created().json(Attendance {
            event_id,
            guest_id: guest.id,
            joined_at,
        })),
        JoinOutcome::NotInserted => {
            // Nothing inserted: the event is missing (404 from fetch_event) or full.
            fetch_event(&db, &event_id).await?;
            Err(AppError::Conflict("event is full".into()))
        }
        JoinOutcome::AlreadyJoined => Err(AppError::Conflict("already joined this event".into())),
    }
}

pub async fn leave(
    req: HttpRequest,
    db: web::Data<Db>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &db).await?;
    let event = fetch_event(&db, &id.to_string()).await?;

    if !db.leave(&event.id, &guest.id).await? {
        return Err(AppError::NotFound("attendance"));
    }
    Ok(HttpResponse::NoContent().finish())
}
