use serde_json::json;
use worker::{Response, console_error, console_warn};

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
    Database(#[from] worker::Error),
    /// Misconfiguration or a failure outside the database. The detail is logged, not returned.
    #[error("internal server error")]
    Internal(String),
}

impl AppError {
    pub fn status(&self) -> u16 {
        match self {
            AppError::NotFound(_) => 404,
            AppError::BadRequest(_) => 400,
            AppError::Conflict(_) => 409,
            AppError::Unauthorized | AppError::LoginFailed(_) => 401,
            AppError::Database(_) | AppError::Internal(_) => 500,
        }
    }

    /// Renders the error as `{"error": "..."}`, logging details that are not returned.
    pub fn into_response(self) -> worker::Result<Response> {
        match &self {
            AppError::Database(err) => console_error!("database error: {err}"),
            AppError::Internal(detail) => console_error!("internal error: {detail}"),
            AppError::LoginFailed(detail) => console_warn!("SSO login failed: {detail}"),
            _ => {}
        }
        Ok(Response::from_json(&json!({ "error": self.to_string() }))?.with_status(self.status()))
    }
}

/// Returns true when the error is a UNIQUE / PRIMARY KEY constraint violation.
pub fn is_unique_violation(err: &worker::Error) -> bool {
    err.to_string().contains("UNIQUE constraint failed")
}

/// For worker API failures outside the database (headers, bodies, responses).
pub fn internal(err: worker::Error) -> AppError {
    AppError::Internal(err.to_string())
}
