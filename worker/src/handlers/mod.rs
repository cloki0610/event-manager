pub mod attendance;
pub mod auth;
pub mod events;
pub mod guests;

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use worker::{Request, Response, RouteContext};

use crate::error::{AppError, internal};

pub type Ctx = RouteContext<()>;

pub fn json<T: Serialize>(status: u16, body: &T) -> Result<Response, AppError> {
    Ok(Response::from_json(body)
        .map_err(internal)?
        .with_status(status))
}

pub fn empty(status: u16) -> Result<Response, AppError> {
    Ok(Response::empty().map_err(internal)?.with_status(status))
}

/// Parses a JSON request body.
pub async fn json_body<T: DeserializeOwned>(req: &mut Request) -> Result<T, AppError> {
    // Requiring a JSON content type keeps plain cross-site form posts from reaching handlers.
    let content_type = req.headers().get("content-type").map_err(internal)?;
    let essence = content_type
        .as_deref()
        .and_then(|v| v.split(';').next())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let is_json = essence
        .split_once('/')
        .is_some_and(|(_, sub)| sub == "json" || sub.ends_with("+json"));
    if !is_json {
        return Err(AppError::BadRequest("Content type error".into()));
    }
    let bytes = req.bytes().await.map_err(internal)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::BadRequest(format!("Json deserialize error: {e}")))
}

/// Parses the query string.
pub fn query<T: DeserializeOwned>(req: &Request) -> Result<T, AppError> {
    req.query()
        .map_err(|e| AppError::BadRequest(format!("Query deserialize error: {e}")))
}

/// The `:id` path segment as a canonical UUID. Anything that is not a UUID names no event.
pub fn event_id(ctx: &Ctx) -> Result<String, AppError> {
    ctx.param("id")
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(|id| id.to_string())
        .ok_or(AppError::NotFound("event"))
}
