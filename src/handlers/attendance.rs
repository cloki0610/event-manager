use actix_web::{HttpRequest, HttpResponse, web};
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::auth::authenticate;
use crate::error::{AppError, is_unique_violation};
use crate::handlers::events::fetch_event;
use crate::models::{Attendance, Attendee};

pub async fn attendees(
    pool: web::Data<SqlitePool>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let event = fetch_event(&pool, &id.to_string()).await?;
    let attendees = sqlx::query_as::<_, Attendee>(
        "SELECT g.id AS guest_id, g.name, a.joined_at \
         FROM attendances a JOIN guests g ON g.id = a.guest_id \
         WHERE a.event_id = ? ORDER BY a.joined_at, g.id",
    )
    .bind(&event.id)
    .fetch_all(pool.get_ref())
    .await?;
    Ok(HttpResponse::Ok().json(attendees))
}

pub async fn join(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &pool).await?;
    let event_id = id.to_string();
    let joined_at = Utc::now();

    // Single statement so the capacity check and the insert are atomic.
    let result = sqlx::query(
        "INSERT INTO attendances (event_id, guest_id, joined_at) \
         SELECT e.id, ?, ? FROM events e \
         WHERE e.id = ? AND (e.capacity IS NULL OR \
             (SELECT COUNT(*) FROM attendances a WHERE a.event_id = e.id) < e.capacity)",
    )
    .bind(&guest.id)
    .bind(joined_at)
    .bind(&event_id)
    .execute(pool.get_ref())
    .await;

    match result {
        Ok(r) if r.rows_affected() == 1 => Ok(HttpResponse::Created().json(Attendance {
            event_id,
            guest_id: guest.id,
            joined_at,
        })),
        Ok(_) => {
            // Nothing inserted: the event is missing (404 from fetch_event) or full.
            fetch_event(&pool, &event_id).await?;
            Err(AppError::Conflict("event is full".into()))
        }
        Err(err) if is_unique_violation(&err) => {
            Err(AppError::Conflict("already joined this event".into()))
        }
        Err(err) => Err(err.into()),
    }
}

pub async fn leave(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
    id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &pool).await?;
    let event = fetch_event(&pool, &id.to_string()).await?;

    let result = sqlx::query("DELETE FROM attendances WHERE event_id = ? AND guest_id = ?")
        .bind(&event.id)
        .bind(&guest.id)
        .execute(pool.get_ref())
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("attendance"));
    }
    Ok(HttpResponse::NoContent().finish())
}
