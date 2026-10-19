use actix_web::{HttpRequest, HttpResponse, web};
use sqlx::SqlitePool;

use crate::auth::authenticate;
use crate::error::AppError;
use crate::handlers::events::EVENT_SELECT;
use crate::models::Event;

pub async fn me(req: HttpRequest, pool: web::Data<SqlitePool>) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &pool).await?;
    Ok(HttpResponse::Ok().json(guest))
}

/// Events the authenticated guest has joined.
pub async fn my_events(
    req: HttpRequest,
    pool: web::Data<SqlitePool>,
) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &pool).await?;
    let events = sqlx::query_as::<_, Event>(&format!(
        "{EVENT_SELECT} JOIN attendances me ON me.event_id = e.id \
         WHERE me.guest_id = ? ORDER BY e.starts_at, e.id"
    ))
    .bind(&guest.id)
    .fetch_all(pool.get_ref())
    .await?;
    Ok(HttpResponse::Ok().json(events))
}
