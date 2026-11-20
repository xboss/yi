//! Local CTranslate2 models, adapted from AudioScript's translation adapters.
//!
//! The model path is the directory containing `model.bin`, not an AudioScript
//! application directory. This module never downloads models or contacts a
//! service. It uses the raw CT2 API so that token budgets and EOS detection are
//! available before decoding the result to a string.
//!
//! Source: AudioScript a410e1b971ababedcc8c08c43864a0640e04028a, MIT,
//! Copyright (c) 2026 Nico. See THIRD_PARTY_NOTICES.md for attribution.

use std::path::{Path, PathBuf};

use ct2rs::tokenizers::hf::Tokenizer as HfTokenizer;
use ferrous_opencc::{OpenCC, config::BuiltinConfig};

use crate::translator::backend::{
    Backend, BackendCapabilities, BackendOutput, DecodingOptions, FinishReason, SegmentRequest,
    TokenCount,
};
use crate::translator::error::{Error, Result};
use crate::translator::language::Language;

const SEQ2SEQ_LIMIT: usize = 512;
const LMT_CONTEXT_WINDOW: usize = 32_768;
const LMT_END_TOKENS: [&str; 2] = ["<|im_end|>", "<|endoftext|>"];
const SEQ2SEQ_END_TOKENS: [&str; 1] = ["</s>"];

/// Model format and trained language directions. These are not quality ratings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFamily {
    /// The translation-finetuned NiuTrans LMT-60 family, using its official prompt.
    /// A generic Qwen model or an LMT `-Base` checkpoint is not interchangeable.
    Lmt,
    /// NLLB-200. This adapter deliberately limits source/target sequences to 512
    /// tokens. The original model is intended for sentence translation.
    Nllb,
    /// Helsinki-NLP/opus-mt-en-zh with separate source/target tokenizer JSONs.
    OpusEnZh,
    /// Helsinki-NLP/opus-mt-zh-en with separate source/target tokenizer JSONs.
    OpusZhEn,
}

impl ModelFamily {
    fn name(self) -> &'static str {
        match self {
            Self::Lmt => "CTranslate2 / LMT-60",
            Self::Nllb => "CTranslate2 / NLLB",
            Self::OpusEnZh => "CTranslate2 / Opus en-zh",
            Self::OpusZhEn => "CTranslate2 / Opus zh-en",
        }
    }

    fn is_seq2seq(self) -> bool {
        self != Self::Lmt
    }
}

/// CPU computation type. Expanding INT8 weights to float32 does not restore
/// information lost when those weights were quantized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComputeType {
    /// Preserve the stored model precision, subject to CT2's CPU fallback rules.
    #[default]
    Default,
    Float32,
    Int8Float32,
}

impl ComputeType {
    fn native(self) -> ct2rs::ComputeType {
        match self {
            Self::Default => ct2rs::ComputeType::DEFAULT,
            Self::Float32 => ct2rs::ComputeType::FLOAT32,
            Self::Int8Float32 => ct2rs::ComputeType::INT8_FLOAT32,
        }
    }
}

/// Loading configuration for one reusable model instance.
///
/// LMT defaults to a conservative 2048-token input/output ceiling and the
/// official LMT-60 32768-position window. If `source-config.json` is present,
/// its `max_position_embeddings` further restricts that window. Input limits
/// include the full prompt and all manually inserted special tokens.
#[derive(Clone, Debug)]
pub struct Ct2Config {
    model_dir: PathBuf,
    family: ModelFamily,
    compute_type: ComputeType,
    threads: usize,
    max_input_tokens: usize,
    max_output_tokens: usize,
    context_window_tokens: Option<usize>,
    warmup: bool,
}

impl Ct2Config {
    pub fn new(model_dir: impl Into<PathBuf>, family: ModelFamily) -> Self {
        let limit = if family == ModelFamily::Lmt {
            2048
        } else {
            SEQ2SEQ_LIMIT
        };
        Self {
            model_dir: model_dir.into(),
            family,
            compute_type: ComputeType::Default,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get().min(8)),
            max_input_tokens: limit,
            max_output_tokens: limit,
            context_window_tokens: (family == ModelFamily::Lmt).then_some(LMT_CONTEXT_WINDOW),
            warmup: true,
        }
    }

    pub fn compute_type(mut self, compute_type: ComputeType) -> Self {
        self.compute_type = compute_type;
        self
    }

    /// CT2 intra-operation CPU threads. Must be at least one.
    pub fn threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }

    pub fn max_input_tokens(mut self, tokens: usize) -> Self {
        self.max_input_tokens = tokens;
        self
    }

    pub fn max_output_tokens(mut self, tokens: usize) -> Self {
        self.max_output_tokens = tokens;
        self
    }

    /// Override the LMT input-plus-output window. This can only lower a window
    /// confirmed by `source-config.json`. Not meaningful for seq2seq models.
    pub fn context_window_tokens(mut self, tokens: usize) -> Self {
        self.context_window_tokens = Some(tokens);
        self
    }

    /// Run a short greedy translation while loading (enabled by default).
    /// This initializes native lazy state and surfaces model errors before use.
    pub fn warmup(mut self, enabled: bool) -> Self {
        self.warmup = enabled;
        self
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    pub fn family(&self) -> ModelFamily {
        self.family
    }

    fn validate(&self) -> Result<()> {
        if self.threads == 0 {
            return Err(Error::InvalidConfig(
                "CT2 threads must be at least 1".into(),
            ));
        }
        if self.max_input_tokens == 0 || self.max_output_tokens == 0 {
            return Err(Error::InvalidConfig(
                "CT2 token limits must be at least 1".into(),
            ));
        }
        if self.family.is_seq2seq() {
            if self.context_window_tokens.is_some() {
                return Err(Error::InvalidConfig(
                    "context_window_tokens is only available for LMT models".into(),
                ));
            }
            if self.max_input_tokens > SEQ2SEQ_LIMIT || self.max_output_tokens > SEQ2SEQ_LIMIT {
                return Err(Error::InvalidConfig(
                    "this NLLB/Opus adapter limits input and output to 512 tokens".into(),
                ));
            }
        } else if self.context_window_tokens.is_none_or(|n| n < 2) {
            return Err(Error::InvalidConfig(
                "LMT context window must be at least 2".into(),
            ));
        }
        Ok(())
    }
}

enum Engine {
    Generator(ct2rs::sys::Generator),
    Translator(ct2rs::sys::Translator),
}

/// A loaded CPU translation model. Drop this value to release its native model.
///
/// The blocking methods should run on a worker thread when integrated in a UI.
/// No global cache, environment mutation, implicit model downloads, or task
/// queue is installed by this adapter.
pub struct Ct2Backend {
    config: Ct2Config,
    engine: Engine,
    source_tokenizer: HfTokenizer,
    // Marian uses a separate target vocabulary; LMT/NLLB share the source one.
    target_tokenizer: Option<HfTokenizer>,
    // Owned, reusable converters: no process-wide singleton or per-request
    // dictionary loading. These use the Apache-2.0 ferrous-opencc implementation.
    to_simplified: OpenCC,
    to_traditional: OpenCC,
}

impl Ct2Backend {
    pub fn load(mut config: Ct2Config) -> Result<Self> {
        config.validate()?;
        config.model_dir = dunce::canonicalize(&config.model_dir)
            .map_err(|error| model_error(&config.model_dir, error))?;
        check_files(&config)?;
        restrict_to_source_window(&mut config)?;

        let (source_file, target_file) = match config.family {
            ModelFamily::OpusEnZh | ModelFamily::OpusZhEn => {
                ("source-tokenizer.json", Some("target-tokenizer.json"))
            }
            _ => ("tokenizer.json", None),
        };
        let source_tokenizer = load_tokenizer(&config.model_dir.join(source_file))?;
        let target_tokenizer = target_file
            .map(|name| load_tokenizer(&config.model_dir.join(name)))
            .transpose()?;

        let mandatory_tokens: &[&str] = if config.family == ModelFamily::Lmt {
            &["<|im_start|>", "<|im_end|>", "<|endoftext|>"]
        } else {
            &["</s>"]
        };
        for &token in mandatory_tokens {
            require_token(&source_tokenizer, token, &config.model_dir)?;
            if let Some(target) = target_tokenizer.as_ref() {
                require_token(target, token, &config.model_dir)?;
            }
        }

        let to_simplified = load_chinese_converter(BuiltinConfig::T2s)?;
        let to_traditional = load_chinese_converter(BuiltinConfig::S2t)?;

        let native = ct2rs::Config {
            device: ct2rs::Device::CPU,
            compute_type: config.compute_type.native(),
            num_threads_per_replica: config.threads,
            // One synchronous request at a time; do not create an unbounded queue.
            max_queued_batches: 1,
            ..Default::default()
        };
        let engine = if config.family == ModelFamily::Lmt {
            Engine::Generator(
                ct2rs::sys::Generator::new(&config.model_dir, &native)
                    .map_err(|error| model_error(&config.model_dir, error))?,
            )
        } else {
            Engine::Translator(
                ct2rs::sys::Translator::new(&config.model_dir, &native)
                    .map_err(|error| model_error(&config.model_dir, error))?,
            )
        };
        let mut backend = Self {
            config,
            engine,
            source_tokenizer,
            target_tokenizer,
            to_simplified,
            to_traditional,
        };
        if backend.config.warmup {
            backend.warm_up()?;
        }
        Ok(backend)
    }

    pub fn config(&self) -> &Ct2Config {
        &self.config
    }

    fn warm_up(&mut self) -> Result<()> {
        let (source, target, text) = if self.config.family == ModelFamily::OpusZhEn {
            (Language::ChineseSimplified, Language::English, "你好。")
        } else {
            (Language::English, Language::ChineseSimplified, "Hello.")
        };
        let request = SegmentRequest {
            text,
            source_language: &source,
            target_language: &target,
            context: &[],
            glossary: &[],
        };
        let count = self.count_input_tokens(&request)?;
        // A deliberately small configured budget must not make constructor
        // warmup fail before the caller has submitted any actual text.
        if count.tokens > self.config.max_input_tokens || self.config.max_output_tokens < 32 {
            return Ok(());
        }
        if self
            .config
            .context_window_tokens
            .is_some_and(|window| count.tokens + 32 > window)
        {
            return Ok(());
        }
        let decoding = DecodingOptions {
            beam_size: 1,
            max_output_tokens: 32,
            length_penalty: 1.0,
            repetition_penalty: 1.0,
        };
        let output = self.translate(&request, &decoding).map_err(|error| {
            model_error(&self.config.model_dir, format!("warmup failed: {error}"))
        })?;
        if output.finish_reason != FinishReason::Stop || output.text.trim().is_empty() {
            return Err(model_error(
                &self.config.model_dir,
                "warmup did not produce a complete translation; check the model family and files",
            ));
        }
        Ok(())
    }

    fn prepare(&self, request: &SegmentRequest<'_>) -> Result<PreparedInput> {
        validate_request(self.config.family, request)?;
        if request.text.trim().is_empty() {
            return Ok(PreparedInput {
                tokens: Vec::new(),
                target_prefix: None,
            });
        }
        let encode = |text: &str| {
            ct2rs::Tokenizer::encode(&self.source_tokenizer, text)
                .map_err(|error| Error::Backend(format!("tokenization failed: {error}")))
        };
        match self.config.family {
            ModelFamily::Lmt => Ok(PreparedInput {
                tokens: encode(&lmt_prompt(request))?,
                target_prefix: None,
            }),
            ModelFamily::Nllb => {
                let source = nllb_language(request.source_language);
                let target = nllb_language(request.target_language);
                require_token(&self.source_tokenizer, source, &self.config.model_dir)?;
                require_token(&self.source_tokenizer, target, &self.config.model_dir)?;
                let mut tokens = vec![source.to_owned()];
                tokens.extend(encode(request.text)?);
                tokens.push("</s>".into());
                Ok(PreparedInput {
                    tokens,
                    target_prefix: Some(target),
                })
            }
            ModelFamily::OpusEnZh | ModelFamily::OpusZhEn => {
                let source = if self.config.family == ModelFamily::OpusZhEn {
                    self.to_simplified.convert(request.text)
                } else {
                    request.text.to_owned()
                };
                let mut tokens = encode(&source)?;
                tokens.push("</s>".into());
                Ok(PreparedInput {
                    tokens,
                    target_prefix: None,
                })
            }
        }
    }

    fn decode(&self, tokens: Vec<String>, target: &Language) -> Result<String> {
        let tokenizer = self
            .target_tokenizer
            .as_ref()
            .unwrap_or(&self.source_tokenizer);
        let text = ct2rs::Tokenizer::decode(tokenizer, tokens)
            .map_err(|error| Error::Backend(format!("detokenization failed: {error}")))?;
        let text = text.trim();
        Ok(match target {
            Language::ChineseSimplified => self.to_simplified.convert(text),
            Language::ChineseTraditional => self.to_traditional.convert(text),
            _ => text.to_owned(),
        })
    }
}

impl Backend for Ct2Backend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            name: self.config.family.name().into(),
            max_input_tokens: self.config.max_input_tokens,
            max_output_tokens: self.config.max_output_tokens,
            context_window_tokens: self.config.context_window_tokens,
            supports_context: false,
            supports_glossary: false,
            supports_beam_search: true,
        }
    }

    fn count_input_tokens(&self, request: &SegmentRequest<'_>) -> Result<TokenCount> {
        Ok(TokenCount {
            tokens: self.prepare(request)?.tokens.len(),
            exact: true,
        })
    }

    fn translate(
        &mut self,
        request: &SegmentRequest<'_>,
        decoding: &DecodingOptions,
    ) -> Result<BackendOutput> {
        validate_decoding(decoding, &self.config)?;
        let prepared = self.prepare(request)?;
        let input_count = prepared.tokens.len();
        validate_budget(input_count, decoding.max_output_tokens, &self.config)?;
        if prepared.tokens.is_empty() {
            return Ok(BackendOutput {
                text: request.text.to_owned(),
                finish_reason: FinishReason::Stop,
                input_tokens: Some(0),
                output_tokens: Some(0),
            });
        }

        let (tokens, stops) = match &self.engine {
            Engine::Generator(engine) => {
                let options = ct2rs::sys::GenerationOptions::<String, String, String> {
                    beam_size: decoding.beam_size,
                    max_length: decoding.max_output_tokens,
                    length_penalty: decoding.length_penalty,
                    repetition_penalty: decoding.repetition_penalty,
                    sampling_topk: 1,
                    include_prompt_in_result: false,
                    end_token: LMT_END_TOKENS
                        .iter()
                        .map(|token| (*token).to_owned())
                        .collect(),
                    return_end_token: true,
                    ..Default::default()
                };
                let results = engine
                    .generate_batch(&[prepared.tokens], &options, None)
                    .map_err(|error| Error::Backend(error.to_string()))?;
                let tokens = results
                    .into_iter()
                    .next()
                    .and_then(|result| result.sequences.into_iter().next())
                    .ok_or_else(|| {
                        Error::Backend("CT2 returned no generation hypothesis".into())
                    })?;
                (tokens, LMT_END_TOKENS.as_slice())
            }
            Engine::Translator(engine) => {
                let options = ct2rs::sys::TranslationOptions::<String, String> {
                    beam_size: decoding.beam_size,
                    max_decoding_length: decoding.max_output_tokens,
                    // Never allow CT2 to silently truncate an article fragment.
                    max_input_length: 0,
                    length_penalty: decoding.length_penalty,
                    repetition_penalty: decoding.repetition_penalty,
                    sampling_topk: 1,
                    end_token: vec!["</s>".into()],
                    return_end_token: true,
                    ..Default::default()
                };
                let results = if let Some(prefix) = prepared.target_prefix {
                    engine.translate_batch_with_target_prefix(
                        &[prepared.tokens],
                        &[vec![prefix]],
                        &options,
                        None,
                    )
                } else {
                    engine.translate_batch(&[prepared.tokens], &options, None)
                }
                .map_err(|error| Error::Backend(error.to_string()))?;
                let tokens = results
                    .into_iter()
                    .next()
                    .and_then(|result| result.hypotheses.into_iter().next())
                    .ok_or_else(|| {
                        Error::Backend("CT2 returned no translation hypothesis".into())
                    })?;
                (tokens, SEQ2SEQ_END_TOKENS.as_slice())
            }
        };
        // Counts are the exact native sequence lengths, including EOS and (for
        // NLLB) the forced target-language token, matching CT2's decoding budget.
        let output_count = tokens.len();
        let (tokens, finish_reason) = finish_tokens(
            tokens,
            stops,
            prepared.target_prefix,
            decoding.max_output_tokens,
        )?;
        let text = self.decode(tokens, request.target_language)?;
        if text.is_empty() && finish_reason == FinishReason::Stop {
            return Err(Error::Backend("model produced an empty translation".into()));
        }
        Ok(BackendOutput {
            text,
            finish_reason,
            input_tokens: Some(input_count),
            output_tokens: Some(output_count),
        })
    }
}

struct PreparedInput {
    tokens: Vec<String>,
    target_prefix: Option<&'static str>,
}

fn model_error(path: &Path, message: impl std::fmt::Display) -> Error {
    Error::Model {
        path: path.to_owned(),
        message: message.to_string(),
    }
}

fn load_chinese_converter(config: BuiltinConfig) -> Result<OpenCC> {
    OpenCC::from_config(config).map_err(|error| {
        Error::Backend(format!(
            "failed to initialize the embedded {config:?} Chinese conversion dictionary: {error}"
        ))
    })
}

fn check_files(config: &Ct2Config) -> Result<()> {
    if !config.model_dir.is_dir() {
        return Err(model_error(
            &config.model_dir,
            "model directory does not exist; provide the directory containing model.bin",
        ));
    }
    let required: &[&str] = match config.family {
        ModelFamily::Lmt => &[
            "model.bin",
            "config.json",
            "tokenizer.json",
            "vocabulary.json",
        ],
        ModelFamily::Nllb => &[
            "model.bin",
            "config.json",
            "tokenizer.json",
            "shared_vocabulary.json",
        ],
        ModelFamily::OpusEnZh | ModelFamily::OpusZhEn => &[
            "model.bin",
            "config.json",
            "source-tokenizer.json",
            "target-tokenizer.json",
        ],
    };
    for &file in required {
        let path = config.model_dir.join(file);
        if !path.is_file() {
            return Err(model_error(
                &path,
                "required model file is missing; keep the complete CTranslate2 converter output",
            ));
        }
    }
    // These model families use explicit source framing below. Reject a model
    // configured to add a second, hidden framing token: it would change both
    // the input text seen by CT2 and the supposedly exact token count.
    if config.family.is_seq2seq() {
        let path = config.model_dir.join("config.json");
        let contents = std::fs::read_to_string(&path).map_err(|error| model_error(&path, error))?;
        let native: serde_json::Value =
            serde_json::from_str(&contents).map_err(|error| model_error(&path, error))?;
        for key in ["add_source_bos", "add_source_eos"] {
            if native
                .get(key)
                .is_some_and(|value| value != &serde_json::Value::Bool(false))
            {
                return Err(model_error(
                    &path,
                    format!(
                        "{key} must be false for this adapter's explicit source-token framing; reconvert with the supplied script"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn load_tokenizer(path: &Path) -> Result<HfTokenizer> {
    let mut tokenizer = HfTokenizer::from_file(path).map_err(|error| model_error(path, error))?;
    tokenizer.disable_spacial_token();
    tokenizer
        .inner()
        .with_truncation(None)
        .map_err(|error| model_error(path, error))?;
    tokenizer.inner().with_padding(None);
    Ok(tokenizer)
}

fn require_token(tokenizer: &HfTokenizer, token: &str, model_path: &Path) -> Result<()> {
    if tokenizer.token_to_id(token).is_none() {
        return Err(model_error(
            model_path,
            format!("tokenizer does not contain required token {token:?}; check model family"),
        ));
    }
    Ok(())
}

fn restrict_to_source_window(config: &mut Ct2Config) -> Result<()> {
    if config.family != ModelFamily::Lmt {
        return Ok(());
    }
    let path = config.model_dir.join("source-config.json");
    let configured = config.context_window_tokens.unwrap_or(LMT_CONTEXT_WINDOW);
    let actual = if path.is_file() {
        let contents = std::fs::read_to_string(&path).map_err(|error| model_error(&path, error))?;
        let source: serde_json::Value =
            serde_json::from_str(&contents).map_err(|error| model_error(&path, error))?;
        let window = source
            .get("max_position_embeddings")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|&value| value >= 2)
            .ok_or_else(|| {
                model_error(
                    &path,
                    "max_position_embeddings must be an integer greater than 1",
                )
            })?;
        configured.min(window)
    } else {
        // ModelFamily::Lmt is specifically the official LMT-60 family, not a
        // generic decoder architecture. Do not infer a 131072 window from the
        // HuggingFace tokenizer metadata; the source model declares 32768.
        configured.min(LMT_CONTEXT_WINDOW)
    };
    config.context_window_tokens = Some(actual);
    config.max_input_tokens = config.max_input_tokens.min(actual - 1);
    config.max_output_tokens = config.max_output_tokens.min(actual - 1);
    Ok(())
}

fn validate_request(family: ModelFamily, request: &SegmentRequest<'_>) -> Result<()> {
    let from = request.source_language;
    let to = request.target_language;
    let supported = if from == to {
        false
    } else {
        match family {
            ModelFamily::Lmt => {
                !matches!(from, Language::Macedonian)
                    && !matches!(to, Language::Macedonian)
                    && from.name() != to.name()
            }
            ModelFamily::Nllb => true,
            ModelFamily::OpusEnZh => *from == Language::English && to.is_chinese(),
            ModelFamily::OpusZhEn => from.is_chinese() && *to == Language::English,
        }
    };
    if !supported {
        return Err(Error::UnsupportedLanguagePair {
            from: from.code().into(),
            to: to.code().into(),
        });
    }
    if !request.context.is_empty() || !request.glossary.is_empty() {
        return Err(Error::UnsupportedFeature {
            backend: family.name().into(),
            feature: if !request.context.is_empty() {
                "context prompting"
            } else {
                "glossary prompting"
            }
            .into(),
        });
    }
    if family == ModelFamily::Lmt {
        for marker in ["<|im_start|>", "<|im_end|>", "<|endoftext|>"] {
            if request.text.contains(marker) {
                return Err(Error::InvalidProtectedText(format!(
                    "source contains reserved LMT control token {marker}; protect it as literal content"
                )));
            }
        }
    }
    Ok(())
}

fn validate_decoding(decoding: &DecodingOptions, config: &Ct2Config) -> Result<()> {
    if decoding.beam_size == 0 || decoding.max_output_tokens == 0 {
        return Err(Error::InvalidConfig(
            "beam size and output token budget must be positive".into(),
        ));
    }
    if !decoding.length_penalty.is_finite() || decoding.length_penalty < 0.0 {
        return Err(Error::InvalidConfig(
            "length penalty must be finite and nonnegative".into(),
        ));
    }
    if !decoding.repetition_penalty.is_finite() || decoding.repetition_penalty <= 0.0 {
        return Err(Error::InvalidConfig(
            "repetition penalty must be finite and positive".into(),
        ));
    }
    if decoding.max_output_tokens > config.max_output_tokens {
        return Err(Error::InvalidConfig(format!(
            "requested {} output tokens exceeds backend limit {}",
            decoding.max_output_tokens, config.max_output_tokens
        )));
    }
    Ok(())
}

fn validate_budget(input: usize, output: usize, config: &Ct2Config) -> Result<()> {
    if input > config.max_input_tokens {
        return Err(Error::InputTooLong {
            tokens: input,
            limit: config.max_input_tokens,
        });
    }
    if let Some(window) = config.context_window_tokens {
        let total = input.checked_add(output).ok_or_else(|| {
            Error::InvalidConfig("input plus output token count overflowed".into())
        })?;
        if total > window {
            return Err(Error::InputTooLong {
                tokens: total,
                limit: window,
            });
        }
    }
    Ok(())
}

fn lmt_prompt(request: &SegmentRequest<'_>) -> String {
    format!(
        "<|im_start|>user\nTranslate the following text from {} into {}:\n{}: {}\n{}:<|im_end|>\n<|im_start|>assistant\n",
        request.source_language.name(),
        request.target_language.name(),
        request.source_language.name(),
        request.text,
        request.target_language.name(),
    )
}

fn finish_tokens(
    mut tokens: Vec<String>,
    stops: &[&str],
    target_prefix: Option<&str>,
    limit: usize,
) -> Result<(Vec<String>, FinishReason)> {
    let finish = if let Some(position) = tokens
        .iter()
        .position(|token| stops.contains(&token.as_str()))
    {
        if tokens[position..]
            .iter()
            .any(|token| !stops.contains(&token.as_str()))
        {
            return Err(Error::Backend(
                "model returned tokens after its end token".into(),
            ));
        }
        // CT2 may retain repeated terminal EOS tokens in a completed beam.
        tokens.truncate(position);
        FinishReason::Stop
    } else if tokens.len() >= limit {
        FinishReason::Length
    } else {
        FinishReason::Unknown
    };
    if let Some(prefix) = target_prefix {
        if tokens.first().map(String::as_str) != Some(prefix) {
            return Err(Error::Backend(
                "NLLB output is missing the forced target language token".into(),
            ));
        }
        tokens.remove(0);
    }
    if tokens
        .iter()
        .any(|token| matches!(token.as_str(), "<unk>" | "<pad>" | "<s>" | "<|im_start|>"))
    {
        return Err(Error::Backend(
            "model emitted an unexpected special or unknown token".into(),
        ));
    }
    Ok((tokens, finish))
}

fn nllb_language(language: &Language) -> &'static str {
    match language {
        Language::English => "eng_Latn",
        Language::ChineseSimplified => "zho_Hans",
        Language::ChineseTraditional => "zho_Hant",
        Language::Japanese => "jpn_Jpan",
        Language::Korean => "kor_Hang",
        Language::French => "fra_Latn",
        Language::German => "deu_Latn",
        Language::Spanish => "spa_Latn",
        Language::Portuguese => "por_Latn",
        Language::Italian => "ita_Latn",
        Language::Russian => "rus_Cyrl",
        Language::Arabic => "arb_Arab",
        Language::Indonesian => "ind_Latn",
        Language::Thai => "tha_Thai",
        Language::Vietnamese => "vie_Latn",
        Language::Turkish => "tur_Latn",
        Language::Hindi => "hin_Deva",
        Language::Malay => "zsm_Latn",
        Language::Dutch => "nld_Latn",
        Language::Swedish => "swe_Latn",
        Language::Danish => "dan_Latn",
        Language::Finnish => "fin_Latn",
        Language::Polish => "pol_Latn",
        Language::Czech => "ces_Latn",
        Language::Filipino => "tgl_Latn",
        Language::Persian => "pes_Arab",
        Language::Greek => "ell_Grek",
        Language::Hungarian => "hun_Latn",
        Language::Romanian => "ron_Latn",
        Language::Cantonese => "yue_Hant",
        Language::Macedonian => "mkd_Cyrl",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "article-translator-ct2-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn request(text: &str) -> SegmentRequest<'_> {
        SegmentRequest {
            text,
            source_language: &Language::English,
            target_language: &Language::ChineseSimplified,
            context: &[],
            glossary: &[],
        }
    }

    fn strings(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    #[test]
    fn prompt_keeps_multiline_article_content() {
        let prompt = lmt_prompt(&request("First line.\nSecond line.\n\nNext paragraph."));
        assert!(prompt.contains("First line.\nSecond line.\n\nNext paragraph."));
        assert!(prompt.ends_with("Chinese:<|im_end|>\n<|im_start|>assistant\n"));
    }

    #[test]
    fn distinguish_eos_from_length_and_unknown_termination() {
        let (tokens, reason) =
            finish_tokens(strings(&["a", "b", "</s>"]), &["</s>"], None, 3).unwrap();
        assert_eq!(reason, FinishReason::Stop);
        assert_eq!(tokens, strings(&["a", "b"]));
        let (_, reason) = finish_tokens(strings(&["a", "b"]), &["</s>"], None, 2).unwrap();
        assert_eq!(reason, FinishReason::Length);
        let (_, reason) = finish_tokens(strings(&["a"]), &["</s>"], None, 2).unwrap();
        assert_eq!(reason, FinishReason::Unknown);
    }

    #[test]
    fn nllb_prefix_and_eos_are_not_visible_in_translation() {
        let (tokens, reason) = finish_tokens(
            strings(&["zho_Hans", "▁世界", "</s>"]),
            &["</s>"],
            Some("zho_Hans"),
            512,
        )
        .unwrap();
        assert_eq!(reason, FinishReason::Stop);
        assert_eq!(tokens, strings(&["▁世界"]));
        assert!(
            finish_tokens(
                strings(&["▁世界", "</s>"]),
                &["</s>"],
                Some("zho_Hans"),
                512
            )
            .is_err()
        );
    }

    #[test]
    fn do_not_erase_output_after_eos_or_unknown_tokens() {
        assert!(finish_tokens(strings(&["ok", "</s>", "more"]), &["</s>"], None, 10).is_err());
        assert!(finish_tokens(strings(&["<unk>", "</s>"]), &["</s>"], None, 10).is_err());
        let (tokens, reason) =
            finish_tokens(strings(&["ok", "</s>", "</s>"]), &["</s>"], None, 10).unwrap();
        assert_eq!(tokens, strings(&["ok"]));
        assert_eq!(reason, FinishReason::Stop);
    }

    #[test]
    fn unsupported_direction_and_prompt_features_fail_explicitly() {
        assert!(matches!(
            validate_request(ModelFamily::OpusZhEn, &request("Hello")),
            Err(Error::UnsupportedLanguagePair { .. })
        ));
        let mut req = request("Hello");
        let glossary = [crate::GlossaryEntry {
            source: "Hello".into(),
            target: "你好".into(),
        }];
        req.glossary = &glossary;
        assert!(matches!(
            validate_request(ModelFamily::Lmt, &req),
            Err(Error::UnsupportedFeature { .. })
        ));
        assert!(matches!(
            validate_request(ModelFamily::Lmt, &request("literal <|im_start|>")),
            Err(Error::InvalidProtectedText(_))
        ));
    }

    #[test]
    fn native_budget_includes_output_reservation() {
        let config = Ct2Config::new("model", ModelFamily::Lmt)
            .max_input_tokens(100)
            .context_window_tokens(150);
        assert!(validate_budget(90, 60, &config).is_ok());
        assert!(matches!(
            validate_budget(90, 61, &config),
            Err(Error::InputTooLong {
                tokens: 151,
                limit: 150
            })
        ));
        assert!(matches!(
            validate_budget(101, 1, &config),
            Err(Error::InputTooLong {
                tokens: 101,
                limit: 100
            })
        ));
    }

    #[test]
    fn reject_invalid_configuration_before_loading_native_code() {
        assert!(matches!(
            Ct2Backend::load(Ct2Config::new("missing", ModelFamily::Lmt).threads(0)),
            Err(Error::InvalidConfig(_))
        ));
        assert!(
            Ct2Config::new("model", ModelFamily::Nllb)
                .max_input_tokens(513)
                .validate()
                .is_err()
        );
        assert!(
            Ct2Config::new("model", ModelFamily::OpusEnZh)
                .context_window_tokens(32768)
                .validate()
                .is_err()
        );
        let decoding = DecodingOptions {
            length_penalty: f32::NAN,
            ..Default::default()
        };
        assert!(validate_decoding(&decoding, &Ct2Config::new("model", ModelFamily::Lmt)).is_err());
    }

    #[test]
    fn chinese_language_tokens_preserve_requested_script() {
        assert_eq!(nllb_language(&Language::ChineseSimplified), "zho_Hans");
        assert_eq!(nllb_language(&Language::ChineseTraditional), "zho_Hant");
        assert_eq!(nllb_language(&Language::Macedonian), "mkd_Cyrl");
        let mut req = request("Hello");
        req.target_language = &Language::Macedonian;
        assert!(validate_request(ModelFamily::Lmt, &req).is_err());
        assert!(validate_request(ModelFamily::Nllb, &req).is_ok());
    }

    #[test]
    fn chinese_conversion_preserves_opaque_markers_and_remains_send() {
        fn assert_send<T: Send>() {}
        assert_send::<OpenCC>();
        assert_send::<Ct2Backend>();

        let simplified = "开放中文转换，⟦ATdeadbeef_0⟧；代码：⟦ATdeadbeef_1⟧。";
        let traditional = "開放中文轉換，⟦ATdeadbeef_0⟧；代碼：⟦ATdeadbeef_1⟧。";
        let t2s = load_chinese_converter(BuiltinConfig::T2s).unwrap();
        let s2t = load_chinese_converter(BuiltinConfig::S2t).unwrap();
        assert_eq!(t2s.convert(traditional), simplified);
        assert_eq!(s2t.convert(simplified), traditional);
        // Reusing the same instances must not retain or alter prior inputs.
        assert_eq!(s2t.convert("⟦ATdeadbeef_0⟧"), "⟦ATdeadbeef_0⟧");
        assert_eq!(t2s.convert("⟦ATdeadbeef_1⟧"), "⟦ATdeadbeef_1⟧");
    }

    #[test]
    fn packaged_tokenizer_cannot_silently_truncate_input() {
        let directory = FixtureDir::new();
        let path = directory.0.join("tokenizer.json");
        let fixture = serde_json::json!({
            "version": "1.0",
            "truncation": {"direction":"Right", "max_length":2, "strategy":"LongestFirst", "stride":0},
            "padding": null, "added_tokens": [], "normalizer": null,
            "pre_tokenizer": {"type":"Whitespace"}, "post_processor": null,
            "decoder": {"type":"WordPiece", "prefix":"##", "cleanup":false},
            "model": {"type":"WordLevel", "vocab":{"<unk>":0,"a":1,"b":2,"c":3,"</s>":4}, "unk_token":"<unk>"}
        });
        std::fs::write(&path, fixture.to_string()).unwrap();
        let tokenizer = load_tokenizer(&path).unwrap();
        assert_eq!(
            ct2rs::Tokenizer::encode(&tokenizer, "a b c").unwrap(),
            strings(&["a", "b", "c"])
        );
    }

    #[test]
    fn model_position_limit_overrides_tokenizer_or_caller_assumptions() {
        let directory = FixtureDir::new();
        std::fs::write(
            directory.0.join("source-config.json"),
            r#"{"max_position_embeddings":2048}"#,
        )
        .unwrap();
        let mut config =
            Ct2Config::new(&directory.0, ModelFamily::Lmt).context_window_tokens(131_072);
        restrict_to_source_window(&mut config).unwrap();
        assert_eq!(config.context_window_tokens, Some(2048));
        assert_eq!(config.max_input_tokens, 2047);
        let mut smaller =
            Ct2Config::new(&directory.0, ModelFamily::Lmt).context_window_tokens(1024);
        restrict_to_source_window(&mut smaller).unwrap();
        assert_eq!(smaller.context_window_tokens, Some(1024));
    }
}
