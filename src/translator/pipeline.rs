use crate::{
    DecodingOptions, Document, GlossaryEntry,
    translator::{
        backend::Backend, backend::BackendCapabilities, backend::ContextEntry,
        backend::FinishReason, backend::SegmentRequest, chunk, document, error::Error,
        error::Result, language::Language,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Cancellation is observed between model calls, not inside native beam search.
#[derive(Clone, Default, Debug)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlossaryMode {
    /// Checks preferred target terms after translation; works with every backend.
    AuditOnly,
    /// Soft prompt plus audit. Requires explicit backend support; not a guarantee.
    PromptAndAudit,
}

#[derive(Clone, Debug)]
struct Options {
    from: Language,
    to: Language,
    decoding: DecodingOptions,
    max_chunk_tokens: usize,
    context_paragraphs: usize,
    glossary: Vec<GlossaryEntry>,
    glossary_mode: GlossaryMode,
    retry_splits: usize,
    cancellation: CancellationToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind {
    EstimatedTokenBudget,
    ContextReduced,
    SplitInput,
    RetriedOutput,
    MissingGlossaryTerm,
    SuspiciousLength,
    RepeatedText,
}

/// Heuristics for human review, never an accuracy/confidence score.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub block_index: Option<usize>,
    pub kind: DiagnosticKind,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockTranslation {
    pub index: usize,
    pub source_range: Range<usize>,
    pub source: String,
    pub translation: String,
    pub model_calls: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranslationReport {
    pub text: String,
    pub backend: String,
    pub source_language: Language,
    pub target_language: Language,
    pub source_sha256: String,
    pub blocks: Vec<BlockTranslation>,
    pub diagnostics: Vec<Diagnostic>,
    /// Includes retries and contextual tokens, when the backend supplies counts.
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub model_calls: usize,
}

/// Completed blocks are emitted before the next block starts, so callers can
/// persist incremental work. A final report is returned only on full success.
#[derive(Clone, Debug)]
pub enum Progress {
    Started { total_blocks: usize },
    BlockStarted { index: usize, total_blocks: usize },
    Retrying { index: usize, reason: String },
    BlockFinished(BlockTranslation),
    Finished { total_blocks: usize },
}

pub struct TranslatorBuilder<B> {
    backend: B,
    options: Options,
}

impl<B: Backend> TranslatorBuilder<B> {
    pub fn decoding(mut self, options: DecodingOptions) -> Self {
        self.options.decoding = options;
        self
    }
    pub fn max_chunk_tokens(mut self, tokens: usize) -> Self {
        self.options.max_chunk_tokens = tokens;
        self
    }
    /// Number of preceding completed blocks supplied as reference to a capable
    /// backend. Unsupported use is rejected rather than silently ignored.
    pub fn context_paragraphs(mut self, count: usize) -> Self {
        self.options.context_paragraphs = count;
        self
    }
    pub fn glossary(mut self, entries: impl IntoIterator<Item = GlossaryEntry>) -> Self {
        self.options.glossary = entries.into_iter().collect();
        self
    }
    pub fn glossary_mode(mut self, mode: GlossaryMode) -> Self {
        self.options.glossary_mode = mode;
        self
    }
    /// Extra bisections after a truncated or malformed model response; default 2.
    pub fn retry_splits(mut self, count: usize) -> Self {
        self.options.retry_splits = count;
        self
    }
    pub fn cancellation_token(mut self, token: CancellationToken) -> Self {
        self.options.cancellation = token;
        self
    }
    pub fn build(self) -> Result<Translator<B>> {
        let caps = self.backend.capabilities();
        self.options.decoding.validate()?;
        if self.options.from == self.options.to {
            return Err(Error::InvalidConfig(
                "source and target languages must differ".into(),
            ));
        }
        if caps.max_input_tokens == 0
            || caps.max_output_tokens == 0
            || self.options.max_chunk_tokens == 0
            || self.options.decoding.max_output_tokens > caps.max_output_tokens
            || caps
                .context_window_tokens
                .is_some_and(|n| n <= self.options.decoding.max_output_tokens)
        {
            return Err(Error::InvalidConfig(
                "invalid input/output/context budgets".into(),
            ));
        }
        if self.options.context_paragraphs > 0 && !caps.supports_context {
            return Err(Error::UnsupportedFeature {
                backend: caps.name,
                feature: "cross-block context".into(),
            });
        }
        if self.options.glossary_mode == GlossaryMode::PromptAndAudit && !caps.supports_glossary {
            return Err(Error::UnsupportedFeature {
                backend: caps.name,
                feature: "glossary prompting; use AuditOnly".into(),
            });
        }
        if !caps.supports_beam_search
            && (self.options.decoding.beam_size > 1
                || self.options.decoding.length_penalty != 1.0
                || self.options.decoding.repetition_penalty != 1.0)
        {
            return Err(Error::UnsupportedFeature {
                backend: caps.name,
                feature: "beam search or native decoding penalties".into(),
            });
        }
        let mut terms = std::collections::HashMap::new();
        for term in &self.options.glossary {
            if term.source.trim().is_empty() || term.target.trim().is_empty() {
                return Err(Error::InvalidConfig(
                    "glossary entries must not be empty".into(),
                ));
            }
            let key = term.source.to_lowercase();
            if let Some(previous) = terms.insert(key, term.target.as_str()) {
                if previous != term.target {
                    return Err(Error::InvalidConfig(
                        "conflicting glossary translations".into(),
                    ));
                }
            }
        }
        if self.options.retry_splits > 8 {
            return Err(Error::InvalidConfig(
                "retry_splits must be <= 8 to bound retries".into(),
            ));
        }
        Ok(Translator {
            backend: self.backend,
            options: self.options,
        })
    }
}

/// Owns one loaded backend; no global state, background thread or lossy queue.
pub struct Translator<B> {
    backend: B,
    options: Options,
}

impl<B: Backend> Translator<B> {
    pub fn builder(backend: B, from: Language, to: Language) -> TranslatorBuilder<B> {
        let caps = backend.capabilities();
        let decoding = DecodingOptions {
            beam_size: if caps.supports_beam_search { 5 } else { 1 },
            max_output_tokens: 1024.min(caps.max_output_tokens),
            ..DecodingOptions::default()
        };
        TranslatorBuilder {
            backend,
            options: Options {
                from,
                to,
                decoding,
                max_chunk_tokens: if caps.supports_context { 4096 } else { 1024 }
                    .min(caps.max_input_tokens),
                context_paragraphs: if caps.supports_context { 2 } else { 0 },
                glossary: Vec::new(),
                glossary_mode: GlossaryMode::AuditOnly,
                retry_splits: 2,
                cancellation: CancellationToken::new(),
            },
        }
    }
    pub fn new(backend: B, from: Language, to: Language) -> Result<Self> {
        Self::builder(backend, from, to).build()
    }
    pub fn into_backend(self) -> B {
        self.backend
    }
    pub fn translate_text(&mut self, text: &str) -> Result<TranslationReport> {
        self.translate(&Document::plain(text))
    }
    pub fn translate_markdown(&mut self, text: &str) -> Result<TranslationReport> {
        self.translate(&Document::markdown(text))
    }
    pub fn translate(&mut self, document: &Document) -> Result<TranslationReport> {
        self.translate_with_progress(document, |_| {})
    }
    pub fn translate_with_progress<F: FnMut(&Progress)>(
        &mut self,
        document: &Document,
        mut on_progress: F,
    ) -> Result<TranslationReport> {
        if self.options.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let plan = document::plan(document)?;
        let total = plan.blocks.len();
        let caps = self.backend.capabilities();
        let mut runner = Runner {
            backend: &mut self.backend,
            options: &self.options,
            caps: caps.clone(),
            progress: &mut on_progress,
            diagnostics: Vec::new(),
            index: 0,
            model_calls: 0,
            input_tokens: Some(0),
            output_tokens: Some(0),
            estimated_reported: false,
        };
        (runner.progress)(&Progress::Started {
            total_blocks: total,
        });
        let mut replacements = Vec::with_capacity(total);
        let mut blocks = Vec::with_capacity(total);
        let mut context = Vec::new();
        for (index, block) in plan.blocks.iter().enumerate() {
            runner.index = index;
            runner.check_cancel()?;
            (runner.progress)(&Progress::BlockStarted {
                index,
                total_blocks: total,
            });
            runner.check_cancel()?;
            let calls_before = runner.model_calls;
            let masked = runner
                .piece(&block.text, &context, 0, self.options.retry_splits)
                .map_err(|source| segment_error(index, source))?;
            let translated = block
                .restore(&masked)
                .and_then(|text| document::validate_block(&plan, index, &text).map(|_| text))
                .map_err(|source| segment_error(index, source))?;
            let source = plan.source[block.range.clone()].to_owned();
            runner.audit(&block.text, &masked);
            let record = BlockTranslation {
                index,
                source_range: block.range.clone(),
                source: source.clone(),
                translation: translated.clone(),
                model_calls: runner.model_calls - calls_before,
            };
            (runner.progress)(&Progress::BlockFinished(record.clone()));
            if self.options.context_paragraphs > 0 {
                context.push(ContextEntry {
                    source,
                    translation: translated.clone(),
                });
                if context.len() > self.options.context_paragraphs {
                    context.remove(0);
                }
            }
            replacements.push(translated);
            blocks.push(record);
        }
        runner.check_cancel()?;
        let text = document::render(&plan, &replacements)?;
        (runner.progress)(&Progress::Finished {
            total_blocks: total,
        });
        Ok(TranslationReport {
            text,
            backend: caps.name,
            source_language: self.options.from,
            target_language: self.options.to,
            source_sha256: format!("{:x}", Sha256::digest(plan.source.as_bytes())),
            blocks,
            diagnostics: runner.diagnostics,
            input_tokens: runner.input_tokens,
            output_tokens: runner.output_tokens,
            model_calls: runner.model_calls,
        })
    }
}

struct Runner<'a, B, F> {
    backend: &'a mut B,
    options: &'a Options,
    caps: BackendCapabilities,
    progress: &'a mut F,
    diagnostics: Vec<Diagnostic>,
    index: usize,
    model_calls: usize,
    input_tokens: Option<usize>,
    output_tokens: Option<usize>,
    estimated_reported: bool,
}

impl<B: Backend, F: FnMut(&Progress)> Runner<'_, B, F> {
    fn check_cancel(&self) -> Result<()> {
        if self.options.cancellation.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    fn diagnostic(&mut self, kind: DiagnosticKind, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            block_index: Some(self.index),
            kind,
            message: message.into(),
        });
    }
    fn piece(
        &mut self,
        text: &str,
        context: &[ContextEntry],
        depth: usize,
        retries: usize,
    ) -> Result<String> {
        self.check_cancel()?;
        if depth > 32 {
            return Err(Error::InvalidConfig("maximum split depth exceeded".into()));
        }
        let visible = without_markers(text);
        if !visible.chars().any(char::is_alphabetic) {
            return Ok(text.to_owned());
        }
        let leading = &text[..text.len() - text.trim_start().len()];
        let trailing = &text[text.trim_end().len()..];
        let text = text.trim();
        let glossary: Vec<_> = if self.options.glossary_mode == GlossaryMode::PromptAndAudit {
            self.options
                .glossary
                .iter()
                .filter(|entry| contains_term(&visible, &entry.source))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let mut context = context.to_vec();
        let limit = self
            .options
            .max_chunk_tokens
            .min(self.caps.max_input_tokens)
            .min(
                self.caps
                    .context_window_tokens
                    .map(|window| window - self.options.decoding.max_output_tokens)
                    .unwrap_or(usize::MAX),
            );
        let mut reduced = false;
        let count = loop {
            let request = SegmentRequest {
                text,
                source_language: &self.options.from,
                target_language: &self.options.to,
                context: &context,
                glossary: &glossary,
            };
            let count = self.backend.count_input_tokens(&request)?;
            if count.tokens <= limit || context.is_empty() {
                break count;
            }
            context.remove(0);
            reduced = true;
        };
        if reduced {
            self.diagnostic(
                DiagnosticKind::ContextReduced,
                "Oldest reference blocks removed to fit the input budget",
            );
        }
        if !count.exact && !self.estimated_reported {
            self.diagnostic(DiagnosticKind::EstimatedTokenBudget,"HTTP backend uses a conservative byte-based estimate; configure the actual server context window");
            self.estimated_reported = true;
        }
        if count.tokens > limit {
            self.diagnostic(
                DiagnosticKind::SplitInput,
                "Input split to fit the backend token budget",
            );
            let inner = self.split(
                text,
                &context,
                depth,
                retries,
                Error::InputTooLong {
                    tokens: count.tokens,
                    limit,
                },
            )?;
            return Ok(format!("{leading}{inner}{trailing}"));
        }
        let request = SegmentRequest {
            text,
            source_language: &self.options.from,
            target_language: &self.options.to,
            context: &context,
            glossary: &glossary,
        };
        self.check_cancel()?;
        self.model_calls += 1;
        let output = self.backend.translate(&request, &self.options.decoding);
        self.check_cancel()?;
        let validated = match output {
            Ok(output) => {
                self.input_tokens = add_usage(self.input_tokens, output.input_tokens);
                self.output_tokens = add_usage(self.output_tokens, output.output_tokens);
                match output.finish_reason {
                    FinishReason::Length => Err(Error::OutputTruncated),
                    FinishReason::Unknown => Err(Error::Backend(
                        "server did not confirm normal generation termination".into(),
                    )),
                    FinishReason::Stop => {
                        let translation = output.text.trim();
                        if !without_markers(translation)
                            .chars()
                            .any(char::is_alphanumeric)
                        {
                            Err(Error::InvalidProtectedText(
                                "output contains no translated prose".into(),
                            ))
                        } else {
                            chunk::verify_markers(text, translation).map(|_| translation.to_owned())
                        }
                    }
                }
            }
            Err(error) => Err(error),
        };
        let translated = match validated {
            Ok(text) => text,
            Err(error @ (Error::OutputTruncated | Error::InvalidProtectedText(_)))
                if retries > 0 =>
            {
                let reason = error.to_string();
                self.diagnostic(DiagnosticKind::RetriedOutput, reason.clone());
                (self.progress)(&Progress::Retrying {
                    index: self.index,
                    reason,
                });
                self.split(text, &context, depth, retries - 1, error)?
            }
            Err(error) => return Err(error),
        };
        Ok(format!("{leading}{translated}{trailing}"))
    }
    fn split(
        &mut self,
        text: &str,
        context: &[ContextEntry],
        depth: usize,
        retries: usize,
        error: Error,
    ) -> Result<String> {
        let Some((left, gap, right)) = chunk::bisect(text) else {
            return Err(error);
        };
        let translated_left = self.piece(left, context, depth + 1, retries)?;
        let mut next_context = context.to_vec();
        if self.options.context_paragraphs > 0 {
            next_context.push(ContextEntry {
                source: left.to_owned(),
                translation: translated_left.clone(),
            });
            while next_context.len() > self.options.context_paragraphs {
                next_context.remove(0);
            }
        }
        let translated_right = self.piece(right, &next_context, depth + 1, retries)?;
        let join = if gap.is_empty()
            && !matches!(
                self.options.to,
                Language::ChineseSimplified
                    | Language::ChineseTraditional
                    | Language::Japanese
                    | Language::Cantonese
            )
            && translated_left
                .chars()
                .last()
                .is_some_and(char::is_alphanumeric)
            && translated_right
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
        {
            " "
        } else {
            gap
        };
        let result = format!("{translated_left}{join}{translated_right}");
        chunk::verify_markers(text, &result)?;
        Ok(result)
    }
    fn audit(&mut self, source: &str, translation: &str) {
        let visible = without_markers(source);
        let translation = without_markers(translation);
        for entry in &self.options.glossary {
            if contains_term(&visible, &entry.source) && !contains_term(&translation, &entry.target)
            {
                self.diagnostic(
                    DiagnosticKind::MissingGlossaryTerm,
                    format!(
                        "Preferred term not found: {} → {}",
                        entry.source, entry.target
                    ),
                );
            }
        }
        let source_len = visible.chars().filter(|c| !c.is_whitespace()).count();
        let target_len = translation.chars().filter(|c| !c.is_whitespace()).count();
        if source_len >= 40
            && (target_len * 10 < source_len || target_len > source_len.saturating_mul(8))
        {
            self.diagnostic(
                DiagnosticKind::SuspiciousLength,
                "Unusual source/translation length ratio; review completeness manually",
            );
        }
        let words: Vec<_> = translation.split_whitespace().collect();
        if words
            .windows(4)
            .any(|v| v[0] == v[1] && v[1] == v[2] && v[2] == v[3])
        {
            self.diagnostic(
                DiagnosticKind::RepeatedText,
                "Four identical consecutive words; review possible repetition",
            );
        }
    }
}

fn segment_error(index: usize, source: Error) -> Error {
    if matches!(source, Error::Cancelled) {
        Error::Cancelled
    } else {
        Error::Segment {
            index,
            source: Box::new(source),
        }
    }
}

fn add_usage(total: Option<usize>, next: Option<usize>) -> Option<usize> {
    total.zip(next).and_then(|(a, b)| a.checked_add(b))
}

fn without_markers(text: &str) -> String {
    let mut visible = text.to_owned();
    if let Ok(markers) = chunk::markers(text) {
        for marker in markers {
            visible = visible.replace(marker, "");
        }
    }
    visible
}

/// ASCII terms match full words; CJK and other non-ASCII terms use substring
/// matching. This is an audit heuristic, not morphological terminology analysis.
fn contains_term(text: &str, term: &str) -> bool {
    let text = text.to_lowercase();
    let term = term.to_lowercase();
    if term.is_empty() {
        return false;
    }
    text.match_indices(&term).any(|(i, _)| {
        if !term.is_ascii() {
            return true;
        }
        let first = term.chars().next().unwrap();
        let last = term.chars().last().unwrap();
        let before = text[..i].chars().next_back();
        let after = text[i + term.len()..].chars().next();
        (!first.is_alphanumeric() || !before.is_some_and(|c| c.is_alphanumeric() || c == '_'))
            && (!last.is_alphanumeric() || !after.is_some_and(|c| c.is_alphanumeric() || c == '_'))
    })
}
