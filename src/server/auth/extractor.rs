use crate::server::{common::error::AppError, state::AppState};
use axum::{extract::FromRequestParts, http::request::Parts};
use deadpool_redis::redis::AsyncCommands;

/// Add `AuthUser` to a handler's argument list to require a valid ticket.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let ticket = parts
            .headers
            .get("ticket")
            .and_then(|h| h.to_str().ok())
            .filter(|v| !v.is_empty() && v.len() <= 128)
            .ok_or(AppError::Unauthorized)?;
        let mut conn = state.redis.get().await?;
        let id: Option<i64> = conn.get(format!("session:{ticket}")).await?;
        let id = id.ok_or(AppError::Unauthorized)?;
        Ok(Self { id })
    }
}
