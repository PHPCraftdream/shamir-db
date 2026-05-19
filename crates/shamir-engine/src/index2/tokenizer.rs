//! Minimal tokenizers for FTS indexes.
//!
//! Zero-copy on ASCII paths (Cow::Borrowed), allocates only for
//! case-folding non-ASCII. No heavy deps — just std + manual
//! iteration.

use std::borrow::Cow;

pub trait Tokenizer: Send + Sync {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>>;
}

/// Split on whitespace, lowercase each token.
pub struct WhitespaceTokenizer;

impl Tokenizer for WhitespaceTokenizer {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>> {
        text.split_whitespace()
            .map(|word| {
                if word.bytes().all(|b| b.is_ascii_lowercase() || !b.is_ascii_alphabetic()) {
                    Cow::Borrowed(word)
                } else {
                    Cow::Owned(word.to_lowercase())
                }
            })
            .collect()
    }
}

/// Split on non-alphanumeric boundaries (unicode-aware), lowercase.
pub struct UnicodeTokenizer;

impl Tokenizer for UnicodeTokenizer {
    fn tokenize<'a>(&self, text: &'a str) -> Vec<Cow<'a, str>> {
        let mut tokens = Vec::new();
        let mut start = None;
        for (i, ch) in text.char_indices() {
            if ch.is_alphanumeric() {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s) = start.take() {
                let word = &text[s..i];
                tokens.push(lowercase_cow(word));
            }
        }
        if let Some(s) = start {
            tokens.push(lowercase_cow(&text[s..]));
        }
        tokens
    }
}

fn lowercase_cow(word: &str) -> Cow<'_, str> {
    if word.chars().all(|c| !c.is_uppercase()) {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(word.to_lowercase())
    }
}

/// Hash a token to u64 for posting keys.
pub fn token_hash(token: &str) -> u64 {
    use fxhash::FxHasher;
    use std::hash::Hasher;
    let mut h = FxHasher::default();
    h.write(token.as_bytes());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_basic() {
        let t = WhitespaceTokenizer;
        let tokens = t.tokenize("Hello World  foo");
        assert_eq!(tokens, vec!["hello", "world", "foo"]);
    }

    #[test]
    fn whitespace_already_lowercase() {
        let t = WhitespaceTokenizer;
        let tokens = t.tokenize("already lower 123");
        assert!(tokens.iter().all(|t| matches!(t, Cow::Borrowed(_))));
    }

    #[test]
    fn whitespace_empty() {
        let t = WhitespaceTokenizer;
        assert!(t.tokenize("").is_empty());
        assert!(t.tokenize("   ").is_empty());
    }

    #[test]
    fn unicode_splits_on_punctuation() {
        let t = UnicodeTokenizer;
        let tokens = t.tokenize("alice@example.com — hello!");
        assert_eq!(tokens, vec!["alice", "example", "com", "hello"]);
    }

    #[test]
    fn unicode_cyrillic() {
        let t = UnicodeTokenizer;
        let tokens = t.tokenize("Привет Мир");
        assert_eq!(tokens, vec!["привет", "мир"]);
    }

    #[test]
    fn unicode_mixed() {
        let t = UnicodeTokenizer;
        let tokens = t.tokenize("foo123 BAR baz");
        assert_eq!(tokens, vec!["foo123", "bar", "baz"]);
    }

    #[test]
    fn token_hash_deterministic() {
        assert_eq!(token_hash("hello"), token_hash("hello"));
        assert_ne!(token_hash("hello"), token_hash("world"));
    }
}
