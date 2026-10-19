pub mod auth;
pub mod error;
pub mod handlers;
pub mod models;
pub mod sso;

use std::str::FromStr;

use actix_web::web;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::error::AppError;

/// Connects to the SQLite database at `url` and applies pending migrations.
pub async fn init_db(url: &str, max_connections: u32) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

/// Registers every route of the API. Requires `web::Data<SqlitePool>` and `web::Data<sso::Sso>`.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.app_data(
        web::JsonConfig::default()
            .error_handler(|err, _| AppError::BadRequest(err.to_string()).into()),
    )
    .app_data(
        web::QueryConfig::default()
            .error_handler(|err, _| AppError::BadRequest(err.to_string()).into()),
    )
    .service(
        web::scope("/events")
            .route("", web::get().to(handlers::events::list))
            .route("", web::post().to(handlers::events::create))
            .route("/{id}", web::get().to(handlers::events::get))
            .route("/{id}", web::patch().to(handlers::events::update))
            .route("/{id}", web::delete().to(handlers::events::delete))
            .route("/{id}/attendees", web::get().to(handlers::attendance::attendees))
            .route("/{id}/join", web::post().to(handlers::attendance::join))
            .route("/{id}/leave", web::post().to(handlers::attendance::leave)),
    )
    .service(
        web::scope("/auth")
            .route("/login", web::get().to(handlers::auth::login))
            .route("/callback", web::get().to(handlers::auth::callback))
            .route("/logout", web::post().to(handlers::auth::logout)),
    )
    .service(
        web::scope("/guests")
            .route("/me", web::get().to(handlers::guests::me))
            .route("/me/events", web::get().to(handlers::guests::my_events)),
    );
}
