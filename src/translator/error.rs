use std::path::PathBuf;

/// Typed failures. No inference error is silently converted to untranslated text.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("unsupported language pair: {from} -> {to}")]
    UnsupportedLanguagePair { from: String, to: String },
    #[error("{backend} does not support {feature}")]
    UnsupportedFeature { backend: String, feature: String },
    #[error("cannot load model at {path}: {message}")]
    Model { path: PathBuf, message: String },
    #[error("backend error: {0}")]
    Backend(String),
    #[error("input requires {tokens} tokens; limit is {limit}")]
    InputTooLong { tokens: usize, limit: usize },
    #[error("generation did not finish before its output limit")]
    OutputTruncated,
    #[error("protected text was changed: {0}")]
    InvalidProtectedText(String),
    #[error("translation cancelled between model calls")]
    Cancelled,
    #[error("block {index} failed: {source}")]
    Segment {
        index: usize,
        #[source]
        source: Box<Error>,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
