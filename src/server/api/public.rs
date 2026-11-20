use crate::{
    server::common::{
        error::{AppError, AppResult},
        json::ApiJson,
        response::{ApiResponse, ok},
    },
    server::state::AppState,
};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginData {
    pub ticket: String,
    pub uid: i64,
}

pub async fn health() -> Json<ApiResponse<&'static str>> {
    ok("ok")
}
