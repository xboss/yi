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
use anyhow::Result;
use axum::{Json, extract::State, http::StatusCode};
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

    println!("start tranlate...");
    // let source = std::fs::read_to_string("article.md")?;
    // let report = translator.translate(&Document::markdown(source))?;

    // let result = translator.translate_text(&req.text)?;
    let texts = state
        .translate_en2cn_texts(vec![req.text])
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "translation failed");
            AppError::Internal("translation failed".into())
        })?;

    let translated = texts.into_iter().next().unwrap();

    println!("{}", translated);

    Ok(ok(TranslationResp { text: translated }))
}

#[derive(Deserialize)]
pub struct ImmersiveReq {
    pub source_lang: String,
    pub target_lang: String,
    pub text_list: Vec<String>,
}

#[derive(Serialize)]
pub struct ImmersiveItem {
    pub detected_source_lang: String,
    pub text: String,
}

#[derive(Serialize)]
pub struct ImmersiveResp {
    pub translations: Vec<ImmersiveItem>,
}

#[axum::debug_handler]
pub async fn immersive(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<ImmersiveReq>,
) -> Result<Json<ImmersiveResp>, (StatusCode, String)> {
    println!("start tranlate...");

    let mut items = Vec::<ImmersiveItem>::new();
    for text in req.text_list {
        let texts = state
            .translate_en2cn_texts(vec![text])
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let translated = texts.into_iter().next().unwrap();
        let item = ImmersiveItem {
            detected_source_lang: "en".to_string(),
            text: translated,
        };

        items.push(item);
    }

    Ok(Json(ImmersiveResp {
        translations: items,
    }))
}
