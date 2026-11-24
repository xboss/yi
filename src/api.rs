use std::{
    error::Error,
    fs,
    fs::OpenOptions,
    io,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::{Arc, mpsc::SyncSender},
    time::Duration,
};

use directories::ProjectDirs;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use translation::TranslationError;

use crate::{
    config::{Config, load_config, save_config},
    lifecycle::{ExitAction, Lifecycle},
    macos_runloop,
    translator::{JobError, SubmitError, TranslatorHandle, valid_language_tag},
};

type ApiFailure = (StatusCode, Json<ErrorResponse>);

#[derive(Clone)]
struct AppState {
    translator: TranslatorHandle,
    lifecycle: Lifecycle,
    config: Arc<Config>,
}

#[derive(Deserialize)]
struct TranslateRequest {
    text: String,
    #[serde(default = "default_from")]
    from: String,
    #[serde(default = "default_to")]
    to: String,
}

fn default_from() -> String {
    "en".to_string()
}
fn default_to() -> String {
    "zh-Hans".to_string()
}

#[derive(Serialize)]
struct TranslateResponse {
    text: String,
    from: String,
    to: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    backend: &'static str,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

fn failure(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiFailure {
    (
        status,
        Json(ErrorResponse {
            error: ErrorDetail {
                code,
                message: message.into(),
            },
        }),
    )
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        backend: "Apple Translation.framework",
    })
}

async fn translate_handler(
    State(state): State<AppState>,
    Json(request): Json<TranslateRequest>,
) -> Result<Json<TranslateResponse>, ApiFailure> {
    if request.text.trim().is_empty() {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "EMPTY_TEXT",
            "text cannot be empty",
        ));
    }
    if request.text.len() > state.config.max_text_bytes {
        return Err(failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "TEXT_TOO_LARGE",
            "text must not exceed 16 KiB; split longer documents into segments",
        ));
    }
    if !valid_language_tag(&request.from) || !valid_language_tag(&request.to) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "INVALID_LANGUAGE",
            "from/to must be language tags such as en, zh-Hans, ja",
        ));
    }
    if request.from.eq_ignore_ascii_case(&request.to) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "IDENTICAL_LANGUAGES",
            "source and target must differ",
        ));
    }

    let from = request.from;
    let to = request.to;
    let receiver = state
        .translator
        .submit(request.text, from.clone(), to.clone())
        .map_err(|error| match error {
            SubmitError::QueueFull => failure(
                StatusCode::TOO_MANY_REQUESTS,
                "QUEUE_FULL",
                "translation queue is full; retry later",
            ),
            SubmitError::WorkerStopped => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "WORKER_STOPPED",
                "translation worker is not available",
            ),
        })?;

    let translated =
        tokio::time::timeout(Duration::from_secs(state.config.response_timeout), receiver)
            .await
            .map_err(|_| {
                failure(
                    StatusCode::GATEWAY_TIMEOUT,
                    "REQUEST_TIMEOUT",
                    "translation did not finish before the HTTP timeout",
                )
            })?
            .map_err(|_| {
                failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "WORKER_STOPPED",
                    "translation worker terminated before replying",
                )
            })?
            .map_err(map_translation_error)?;

    Ok(Json(TranslateResponse {
        text: translated,
        from,
        to,
    }))
}

fn map_translation_error(error: JobError) -> ApiFailure {
    match error {
        JobError::InvalidLanguage(message) => {
            failure(StatusCode::BAD_REQUEST, "INVALID_LANGUAGE", message)
        }
        JobError::Framework(error) => match error {
            TranslationError::NotInstalled(message) => {
                failure(StatusCode::CONFLICT, "LANGUAGE_PACK_NOT_INSTALLED", message)
            }
            TranslationError::UnsupportedSourceLanguage(message)
            | TranslationError::UnsupportedTargetLanguage(message)
            | TranslationError::UnsupportedLanguagePairing(message) => failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                "UNSUPPORTED_LANGUAGE_PAIR",
                message,
            ),
            TranslationError::MainRunLoopNotRunning(message) => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "MAIN_RUN_LOOP_NOT_RUNNING",
                message,
            ),
            TranslationError::UnavailableOnThisMacOS(message) => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "MACOS_VERSION_UNSUPPORTED",
                message,
            ),
            TranslationError::TimedOut(message) => {
                failure(StatusCode::GATEWAY_TIMEOUT, "TRANSLATION_TIMEOUT", message)
            }
            other => failure(
                StatusCode::BAD_GATEWAY,
                "TRANSLATION_FAILED",
                other.to_string(),
            ),
        },
    }
}

///////////////////////////////////////////////////
// service for immersive
///////////////////////////////////////////////////

#[derive(Deserialize)]
struct ImmersiveReq {
    source_lang: String,
    target_lang: String,
    text_list: Vec<String>,
}

#[derive(Serialize)]
struct ImmersiveItem {
    detected_source_lang: String,
    text: String,
}

#[derive(Serialize)]
struct ImmersiveResp {
    translations: Vec<ImmersiveItem>,
}

// #[axum::debug_handler]
async fn immersive_handler(
    State(state): State<AppState>,
    Json(req): Json<ImmersiveReq>,
) -> Result<Json<ImmersiveResp>, (StatusCode, String)> {
    println!("start tranlate...");

    let (from, to) = match (req.source_lang.as_str(), req.target_lang.as_str()) {
        ("en", "zh-CN") => ("en", "zh-Hans"),
        ("zh-CN", "en") => ("zh-Hans", "en"),
        (_, _) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "source/target language error.".to_string(),
            ));
        }
    };

    let mut items = Vec::<ImmersiveItem>::new();
    for text in req.text_list {
        let receiver = state
            .translator
            .submit(text, from.to_string(), to.to_string())
            .map_err(|error| match error {
                SubmitError::QueueFull => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "translation queue is full; retry later".to_string(),
                ),
                SubmitError::WorkerStopped => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "translation worker is not available".to_string(),
                ),
            })?;

        let translated =
            tokio::time::timeout(Duration::from_secs(state.config.response_timeout), receiver)
                .await
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "translation did not finish before the HTTP timeout".to_string(),
                    )
                })?
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "translation worker terminated before replying".to_string(),
                    )
                })?
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "translation worker error".to_string(),
                    )
                })?;

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

pub fn run_http_server(
    config: Arc<Config>,
    std_listener: TcpListener,
    translator: TranslatorHandle,
    ready: SyncSender<Result<(), String>>,
) -> ExitAction {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(format!("cannot start Tokio runtime: {error}")));
            return ExitAction::Stop;
        }
    };

    let lifecycle = Lifecycle::new();

    let exit_action = runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::from_std(std_listener) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = ready.send(Err(format!("cannot initialize listener: {error}")));
                return ExitAction::Stop;
            }
        };

        let state = AppState {
            translator,
            lifecycle: lifecycle.clone(),
            config: config,
        };
        let app = Router::new()
            .route("/healthz", get(healthz))
            .route("/api/translate", post(translate_handler))
            .route("/api/immersive", post(immersive_handler))
            .route(
                "/api/config",
                get(config_handler).post(update_config_handler),
            )
            .route("/api/restart", post(restart_handler))
            .route("/", get(index_handler))
            .layer(DefaultBodyLimit::max(state.config.body_limit * 1024))
            .with_state(state);

        let _ = ready.send(Ok(()));

        let shutdown = {
            let lifecycle = lifecycle.clone();
            async move {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            eprintln!("cannot install Ctrl-C handler: {error}");
                        };
                        lifecycle.request_stop();
                    }
                    _ = lifecycle.wait() => {}
                }
                println!("HTTP server shutting down: {:?}", lifecycle.action());
            }
        };

        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
        {
            eprintln!("HTTP server error: {error}");
        }

        match lifecycle.action() {
            ExitAction::Running => ExitAction::Stop,
            action => action,
        }
    });

    drop(runtime);

    // The server has stopped; allow the main thread to return from CFRunLoopRun.
    macos_runloop::request_stop();

    exit_action
}

///////////////////////////////////////////////////
// config
///////////////////////////////////////////////////

async fn update_config_handler(
    State(state): State<AppState>,
    Json(request): Json<Config>,
) -> Result<Json<Config>, ApiFailure> {
    println!("update config handler.");

    validate_config(&request).map_err(|e| e)?;
    let Ok(_) = save_config(&request) else {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "SAVE_CONFIG_ERROR",
            "save config error",
        ));
    };

    println!("config: {:?}", request);
    Ok(Json(request))
}

async fn config_handler(State(state): State<AppState>) -> Result<Json<Config>, ApiFailure> {
    let config = load_config().map_err(|_| {
        failure(
            StatusCode::BAD_REQUEST,
            "LOAD_CONFIG_ERROR",
            "load config error",
        )
    })?;

    Ok(Json(config))
}

fn validate_config(config: &Config) -> Result<(), ApiFailure> {
    if config.translate_bind.is_empty() || !config.translate_bind.parse::<SocketAddr>().is_ok() {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "TRANSLATE_BIND_ERROR",
            "invalid translate bind",
        ));
    }
    // queue_capacity: usize,
    if !(1..=256).contains(&config.queue_capacity) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "QUEUE_CAPACITY_ERROR",
            "queue capacity must between 1 and 256",
        ));
    }
    // session_cache_capacity: usize,
    if !(1..=128).contains(&config.session_cache_capacity) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "SESSION_CACHE_CAPACITY_ERROR",
            "session cache capacity must bebween 1 and 128",
        ));
    }
    // response_timeout: usize,    // seconds
    if !(60..=600).contains(&config.response_timeout) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "RESPONSE_TIMEOUT_ERROR",
            "response timeout must bebween 60 and 600 seconds",
        ));
    }
    // max_text_bytes: usize,     // KB
    if !(1..=1024).contains(&config.max_text_bytes) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "MAX_TEXT_BYTE_ERROR",
            "max text bytes must bebween 1 and 1024 KB",
        ));
    }
    // default_body_limit: usize, // KB
    if !(1..=1024).contains(&config.body_limit) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "BODY_LIMIT_ERROR",
            "body limit must bebween 1 and 1024 KB, and must be greater than max text bytes",
        ));
    }
    // worker_count: usize,
    if !(1..=64).contains(&config.worker_count) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "WORKER_COUNT_ERROR",
            "worker count must bebween 1 and 64",
        ));
    }

    Ok(())
}

///////////////////////////////////////////////////
// restart process
///////////////////////////////////////////////////

async fn restart_handler(State(state): State<AppState>) -> StatusCode {
    state.lifecycle.request_restart();
    StatusCode::ACCEPTED
}

///////////////////////////////////////////////////
// index page
///////////////////////////////////////////////////

const INDEX_HTML: &str = include_str!("../frontend/index.html");

async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}
