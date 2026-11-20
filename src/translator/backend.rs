use crate::translator::{error::Error, error::Result, language::Language};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct BackendCapabilities {
    pub name: String,
    pub max_input_tokens: usize,
    pub max_output_tokens: usize,
    pub context_window_tokens: Option<usize>,
    pub supports_context: bool,
    pub supports_glossary: bool,
    pub supports_beam_search: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextEntry {
    pub source: String,
    pub translation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlossaryEntry {
    pub source: String,
    pub target: String,
}

impl GlossaryEntry {
    pub fn new(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
        }
    }
}

pub struct SegmentRequest<'a> {
    pub text: &'a str,
    pub source_language: &'a Language,
    pub target_language: &'a Language,
    pub context: &'a [ContextEntry],
    pub glossary: &'a [GlossaryEntry],
}

#[derive(Clone, Copy, Debug)]
pub struct TokenCount {
    pub tokens: usize,
    /// False for conservative estimates used without the server's tokenizer.
    pub exact: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct BackendOutput {
    pub text: String,
    pub finish_reason: FinishReason,
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
}

/// Deterministic decoding settings. Chat adapters support beam_size=1 and
/// neutral penalties only; unsupported choices produce a typed error.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecodingOptions {
    pub beam_size: usize,
    pub max_output_tokens: usize,
    pub length_penalty: f32,
    pub repetition_penalty: f32,
}

impl Default for DecodingOptions {
    fn default() -> Self {
        Self {
            beam_size: 5,
            max_output_tokens: 1024,
            length_penalty: 1.0,
            repetition_penalty: 1.0,
        }
    }
}

impl DecodingOptions {
    pub fn validate(&self) -> Result<()> {
        if self.beam_size == 0 || self.max_output_tokens == 0 {
            return Err(Error::InvalidConfig(
                "beam_size and max_output_tokens must be positive".into(),
            ));
        }
        if !self.length_penalty.is_finite()
            || self.length_penalty < 0.0
            || !self.repetition_penalty.is_finite()
            || self.repetition_penalty <= 0.0
        {
            return Err(Error::InvalidConfig("invalid decoding penalties".into()));
        }
        Ok(())
    }
}

/// Model lifetime is separate from requests. Implement this trait to reuse the
/// document pipeline with another inference runtime. Calls are synchronous.
pub trait Backend: Send {
    fn capabilities(&self) -> BackendCapabilities;
    fn count_input_tokens(&self, request: &SegmentRequest<'_>) -> Result<TokenCount>;
    fn translate(
        &mut self,
        request: &SegmentRequest<'_>,
        decoding: &DecodingOptions,
    ) -> Result<BackendOutput>;
}

impl<T: Backend + ?Sized> Backend for Box<T> {
    fn capabilities(&self) -> BackendCapabilities {
        (**self).capabilities()
    }
    fn count_input_tokens(&self, r: &SegmentRequest<'_>) -> Result<TokenCount> {
        (**self).count_input_tokens(r)
    }
    fn translate(&mut self, r: &SegmentRequest<'_>, d: &DecodingOptions) -> Result<BackendOutput> {
        (**self).translate(r, d)
    }
}
