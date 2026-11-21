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

use crate::server::{server::start_server, state::AppState};
use anyhow::Result;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // 模型只加载一次，后续可以连续翻译多篇文章。
    let backend = Ct2Backend::load(Ct2Config::new(
        "/Users/sunji/Library/Application Support/com.audioscript.desktop/models/mt-lmt-high",
        ModelFamily::Lmt,
    ))?;

    let mut en2cn_translator =
        Translator::builder(backend, Language::English, Language::ChineseSimplified)
            .decoding(DecodingOptions {
                beam_size: 5,
                max_output_tokens: 2048,
                ..Default::default()
            })
            .max_chunk_tokens(1024)
            .glossary([
                GlossaryEntry::new("Rust", "Rust"),
                GlossaryEntry::new("ownership", "所有权"),
                GlossaryEntry::new("cache", "缓存"),
            ])
            .build()?;

    let state = AppState {
        en2cn_translator: Arc::new(Mutex::new(en2cn_translator)),
    };
    println!("model load end...");

    start_server(state).await?;
    Ok(())
}
