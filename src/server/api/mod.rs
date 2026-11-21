pub mod public;

use crate::server::state::AppState;
use axum::{
    Router,
    routing::{get, post},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(public::health))
        .route("/translate", post(public::translate))
        .route("/immersive", post(public::immersive))
    // .route("/api/register", post(public::register))
    // .route("/api/login", post(public::login))
    // .route("/api/me", post(user::me))
    // .route("/api/logout", post(user::logout))
}
