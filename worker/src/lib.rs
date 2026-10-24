pub mod auth;
pub mod db;
pub mod error;
pub mod handlers;
pub mod models;
pub mod sso;

use std::future::Future;

use worker::{Context, Env, Request, Response, Router, event};

use crate::error::AppError;
use crate::handlers::{attendance, auth as login, events, guests};

/// Renders a handler's error as the JSON error response.
async fn respond(
    handler: impl Future<Output = Result<Response, AppError>>,
) -> worker::Result<Response> {
    match handler.await {
        Ok(resp) => Ok(resp),
        Err(err) => err.into_response(),
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> worker::Result<Response> {
    Router::new()
        .get_async("/events", |r, c| respond(events::list(r, c)))
        .post_async("/events", |r, c| respond(events::create(r, c)))
        .get_async("/events/:id", |r, c| respond(events::get(r, c)))
        .patch_async("/events/:id", |r, c| respond(events::update(r, c)))
        .delete_async("/events/:id", |r, c| respond(events::delete(r, c)))
        .get_async("/events/:id/attendees", |r, c| {
            respond(attendance::attendees(r, c))
        })
        .post_async("/events/:id/join", |r, c| respond(attendance::join(r, c)))
        .post_async("/events/:id/leave", |r, c| respond(attendance::leave(r, c)))
        .get_async("/auth/login", |r, c| respond(login::login(r, c)))
        .get_async("/auth/callback", |r, c| respond(login::callback(r, c)))
        .post_async("/auth/logout", |r, c| respond(login::logout(r, c)))
        .get_async("/guests/me", |r, c| respond(guests::me(r, c)))
        .get_async("/guests/me/events", |r, c| respond(guests::my_events(r, c)))
        .run(req, env)
        .await
}
