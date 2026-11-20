mod server;
mod translator;

use translator::{
    backend::DecodingOptions,
    backend::GlossaryEntry,
    backends::ct2::{Ct2Backend, Ct2Config, ModelFamily},
    document::Document,
    language::Language,
    pipeline::{GlossaryMode, Translator},
};

// fn main() -> Result<(), Box<dyn std::error::Error>> {
//     // 模型只加载一次，后续可以连续翻译多篇文章。
//     let backend = Ct2Backend::load(Ct2Config::new(
//         "/Users/sunji/Library/Application Support/com.audioscript.desktop/models/mt-lmt-high",
//         ModelFamily::Lmt,
//     ))?;

//     println!("load end...");

//     let mut translator =
//         Translator::builder(backend, Language::English, Language::ChineseSimplified)
//             .decoding(DecodingOptions {
//                 beam_size: 5,
//                 max_output_tokens: 2048,
//                 ..Default::default()
//             })
//             .max_chunk_tokens(1024)
//             .glossary([
//                 GlossaryEntry::new("Rust", "Rust"),
//                 GlossaryEntry::new("ownership", "所有权"),
//                 GlossaryEntry::new("cache", "缓存"),
//             ])
//             .build()?;

//     println!("start tranlate...");
//     // let source = std::fs::read_to_string("article.md")?;
//     // let report = translator.translate(&Document::markdown(source))?;
//     let result = translator.translate_text("Hello, how are you?")?;

//     println!("{}", result.text);

//     // println!("translate end...")
//     // std::fs::write("article.zh.md", &report.text)?;

//     // for diagnostic in &report.diagnostics {
//     //     eprintln!("{:?}: {}", diagnostic.kind, diagnostic.message);
//     // }

//     Ok(())
// }

use crate::server::{api, state::AppState};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::any};
use serde_json::json;
use std::{env, net::SocketAddr};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let state = AppState {};
    let app = Router::new()
        .merge(api::router())
        .fallback(any(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"code":4004,"message":"not found","data":null})),
            )
        }))
        .with_state(state);
    let addr: SocketAddr = env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "server started");
    axum::serve(listener, app).await?;
    Ok(())
}
