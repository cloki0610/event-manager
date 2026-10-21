use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("missing or invalid session")]
    Unauthorized,
    /// SSO login could not be completed. The detail is logged, not returned.
    #[error("SSO login failed")]
    LoginFailed(String),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[cfg(feature = "duckdb")]
    #[error("database error")]
    DuckDb(#[from] duckdb::Error),
    /// A blocking database task panicked or was cancelled.
    #[error("database error")]
    Blocking(String),
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Unauthorized | AppError::LoginFailed(_) => StatusCode::UNAUTHORIZED,
            AppError::Database(_) | AppError::Blocking(_) => StatusCode::INTERNAL_SERVER_ERROR,
            #[cfg(feature = "duckdb")]
            AppError::DuckDb(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        match self {
            AppError::Database(err) => log::error!("database error: {err}"),
            AppError::Blocking(err) => log::error!("database task failed: {err}"),
            #[cfg(feature = "duckdb")]
            AppError::DuckDb(err) => log::error!("database error: {err}"),
            AppError::LoginFailed(detail) => log::warn!("SSO login failed: {detail}"),
            _ => {}
        }
        HttpResponse::build(self.status_code()).json(json!({ "error": self.to_string() }))
    }
}

/// Returns true when the error is a UNIQUE / PRIMARY KEY constraint violation.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|db| db.is_unique_violation())
}
