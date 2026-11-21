use crate::server::{api, state::AppState};
use crate::translator::backends::ct2::Ct2Backend;
use anyhow::Result;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::any};
use std::{env, net::SocketAddr};
use tower_http::services::{ServeDir, ServeFile};

pub async fn start_server(state: AppState) -> Result<()> {
    let api_router = api::router().fallback(|| async { StatusCode::NOT_FOUND });

    let frontend =
        ServeDir::new("src/frontend/").fallback(ServeFile::new("src/frontend/index.html"));

    let app = Router::new()
        .nest("/api", api_router)
        .fallback_service(frontend)
        .with_state(state);

    let addr: SocketAddr = env::var("YI_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "server started");
    axum::serve(listener, app).await?;

    Ok(())
}
