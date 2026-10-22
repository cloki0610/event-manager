use actix_web::{HttpRequest, HttpResponse, web};

use crate::auth::authenticate;
use crate::db::Db;
use crate::error::AppError;

pub async fn me(req: HttpRequest, db: web::Data<Db>) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &db).await?;
    Ok(HttpResponse::Ok().json(guest))
}

/// Events the authenticated guest has joined.
pub async fn my_events(req: HttpRequest, db: web::Data<Db>) -> Result<HttpResponse, AppError> {
    let guest = authenticate(&req, &db).await?;
    let events = db.guest_events(&guest.id).await?;
    Ok(HttpResponse::Ok().json(events))
}
