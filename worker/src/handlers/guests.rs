use worker::{Request, Response};

use super::{Ctx, json};
use crate::auth::authenticate;
use crate::db::Db;
use crate::error::AppError;

pub async fn me(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let db = Db::from_env(&ctx.env)?;
    json(200, &authenticate(&req, &db).await?)
}

/// Events the authenticated guest has joined.
pub async fn my_events(req: Request, ctx: Ctx) -> Result<Response, AppError> {
    let db = Db::from_env(&ctx.env)?;
    let guest = authenticate(&req, &db).await?;
    json(200, &db.guest_events(&guest.id).await?)
}
