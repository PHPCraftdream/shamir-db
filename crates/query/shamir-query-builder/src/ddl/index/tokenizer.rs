//! FTS tokenizer types.
//!
//! Defines the tokenizer options for full-text search indexes.

/// Full-text search tokenizer.
///
/// Determines how text is split into tokens for indexing and search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tokenizer {
    /// Whitespace tokenizer: splits on any whitespace character.
    Whitespace,
    /// Unicode tokenizer: uses Unicode-aware text segmentation.
    Unicode,
}

impl Tokenizer {
    /// Returns the wire representation of this tokenizer.
    pub fn as_str(self) -> &'static str {
        match self {
            Tokenizer::Whitespace => "whitespace",
            Tokenizer::Unicode => "unicode",
        }
    }
}

impl From<Tokenizer> for String {
    fn from(tokenizer: Tokenizer) -> Self {
        tokenizer.as_str().to_string()
    }
}
