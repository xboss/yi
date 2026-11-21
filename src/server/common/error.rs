use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("conflict")]
    Conflict,
    #[error("not found")]
    NotFound,
    // #[error("database error: {0}")]
    // Database(#[from] sqlx::Error),
    // #[error("redis error: {0}")]
    // Redis(#[from] deadpool_redis::redis::RedisError),
    // #[error("redis pool error: {0}")]
    // RedisPool(#[from] deadpool_redis::PoolError),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("translator error: {0}")]
    Translator(#[from] crate::translator::error::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Never expose database, Redis or password errors to clients.
        let (status, code, message): (StatusCode, i32, &str) = match &self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, 4000, "bad request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, 4001, "unauthorized"),
            Self::Conflict => (StatusCode::CONFLICT, 4009, "conflict"),
            Self::NotFound => (StatusCode::NOT_FOUND, 4004, "not found"),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                5000,
                "internal server error",
            ),
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let body: Value = json!({"code": code, "message": message, "data": null});
        (status, Json(body)).into_response()
    }
}
