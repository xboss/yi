use crate::translator::{
    backend::DecodingOptions,
    backend::GlossaryEntry,
    backends::ct2::{Ct2Backend, Ct2Config, ModelFamily},
    document::Document,
    language::Language,
    pipeline::{GlossaryMode, Translator},
};
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
use std::str::FromStr;

pub async fn health() -> Json<ApiResponse<&'static str>> {
    ok("ok")
}

#[derive(Deserialize)]
pub struct TranslationReq {
    from: String,
    to: String,
    text: String,
}

#[derive(Serialize)]
pub struct TranslationResp {
    text: String,
}

#[axum::debug_handler]
pub async fn translate(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<TranslationReq>,
) -> AppResult<Json<ApiResponse<TranslationResp>>> {
    if req.from.is_empty() || req.to.is_empty() || req.text.is_empty() {
        return Err(AppError::BadRequest("Invalid params".into()));
    }

    let (Ok(from), Ok(to)) = (Language::from_str(&req.from), Language::from_str(&req.to)) else {
        return Err(AppError::BadRequest("Invalid params from or to".into()));
    };

    let mut translator = match (from, to) {
        (Language::English, Language::ChineseSimplified) => {
            let Ok(mut translator) = state.en2cn_translator.lock() else {
                return Err(AppError::Internal("translator lock failed".into()));
            };
            translator
        }
        (_, _) => {
            return Err(AppError::BadRequest("Invalid params from or to".into()));
        }
    };

    println!("start tranlate...");
    // let source = std::fs::read_to_string("article.md")?;
    // let report = translator.translate(&Document::markdown(source))?;

    let result = translator.translate_text(&req.text)?;

    println!("{}", result.text);

    Ok(ok(TranslationResp { text: result.text }))
}
