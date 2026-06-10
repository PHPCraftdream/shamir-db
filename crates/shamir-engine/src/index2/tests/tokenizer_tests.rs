use crate::index2::kind::{StemLanguage, TokenizerKind};
use crate::index2::tokenizer::{
    build_tokenizer, token_hash, FullTokenizer, NgramTokenizer, Tokenizer, UnicodeTokenizer,
    WhitespaceTokenizer,
};
use std::borrow::Cow;

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

// ----- Full tokenizer: stemming -----

#[test]
fn english_stem_normalises_runs() {
    let t = FullTokenizer::new(StemLanguage::English, false, true);
    let running = t.tokenize("running");
    let runs = t.tokenize("runs");
    assert_eq!(
        running, runs,
        "running and runs should stem to the same token"
    );
}

#[test]
fn russian_stem_basic() {
    let t = FullTokenizer::new(StemLanguage::Russian, false, true);
    let tokens = t.tokenize("бежал");
    assert!(!tokens.is_empty(), "should produce at least one token");
    let stemmed = &tokens[0];
    assert!(!stemmed.is_empty());
}

// ----- Full tokenizer: stopwords -----

#[test]
fn english_stopwords_filtered() {
    let t = FullTokenizer::new(StemLanguage::English, true, false);
    let tokens = t.tokenize("the cat");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["cat"], "the should be filtered as stopword");
}

#[test]
fn russian_stopwords_filtered() {
    let t = FullTokenizer::new(StemLanguage::Russian, true, false);
    let tokens = t.tokenize("и кот");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["кот"], "и should be filtered as stopword");
}

// ----- build_tokenizer helper -----

#[test]
fn build_tokenizer_whitespace() {
    let t = build_tokenizer(&TokenizerKind::Whitespace);
    let tokens = t.tokenize("Hello World");
    assert_eq!(tokens, vec!["hello", "world"]);
}

#[test]
fn build_tokenizer_full_en() {
    let t = build_tokenizer(&TokenizerKind::Full {
        language: StemLanguage::English,
        stopwords: true,
        stem: true,
    });
    let tokens = t.tokenize("the cats are running");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert!(!strs.contains(&"the"), "stopword 'the' should be filtered");
    assert!(!strs.contains(&"are"), "stopword 'are' should be filtered");
}

// ----- Stemming: new languages -----

#[test]
fn french_stem_inflections() {
    let t = FullTokenizer::new(StemLanguage::French, false, true);
    let a = t.tokenize("chats");
    let b = t.tokenize("chat");
    assert_eq!(a, b, "French: chats and chat should stem identically");
}

#[test]
fn german_stem_produces_output() {
    let t = FullTokenizer::new(StemLanguage::German, false, true);
    let tokens = t.tokenize("laufen");
    assert!(!tokens.is_empty());
    assert!(!tokens[0].is_empty());
}

#[test]
fn spanish_stem_inflections() {
    let t = FullTokenizer::new(StemLanguage::Spanish, false, true);
    let a = t.tokenize("corriendo");
    let b = t.tokenize("corre");
    // Both should produce non-empty stems; Snowball may or may not
    // unify them but they must be non-empty.
    assert!(!a.is_empty());
    assert!(!b.is_empty());
}

#[test]
fn italian_stem_produces_output() {
    let t = FullTokenizer::new(StemLanguage::Italian, false, true);
    let tokens = t.tokenize("correndo");
    assert!(!tokens.is_empty());
    assert!(!tokens[0].is_empty());
}

#[test]
fn stopwords_graceful_for_unsupported_language() {
    // French has no curated stopword set — requesting stopwords
    // should be a no-op (no error, no filtering).
    let t = FullTokenizer::new(StemLanguage::French, true, false);
    let tokens = t.tokenize("le chat");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    // "le" is NOT filtered because there's no French stopword list.
    assert_eq!(strs, vec!["le", "chat"]);
}

// ----- NgramTokenizer -----

#[test]
fn ngram_hello_n3() {
    let t = NgramTokenizer::new(3);
    let tokens = t.tokenize("hello");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["hel", "ell", "llo"]);
}

#[test]
fn ngram_short_word() {
    let t = NgramTokenizer::new(3);
    let tokens = t.tokenize("hi");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["hi"], "word shorter than n emitted whole");
}

#[test]
fn ngram_n2_abc() {
    let t = NgramTokenizer::new(2);
    let tokens = t.tokenize("abc");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["ab", "bc"]);
}

#[test]
fn ngram_splits_on_punctuation() {
    let t = NgramTokenizer::new(2);
    let tokens = t.tokenize("ab-cd");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["ab", "cd"]);
}

#[test]
fn ngram_zero_treated_as_one() {
    let t = NgramTokenizer::new(0);
    let tokens = t.tokenize("hi");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["h", "i"]);
}

#[test]
fn ngram_lowercases() {
    let t = NgramTokenizer::new(3);
    let tokens = t.tokenize("HELLO");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["hel", "ell", "llo"]);
}

// ----- build_tokenizer: ngram + full French -----

#[test]
fn build_tokenizer_ngram() {
    let t = build_tokenizer(&TokenizerKind::Ngram { n: 3 });
    let tokens = t.tokenize("hello");
    let strs: Vec<&str> = tokens.iter().map(|c| c.as_ref()).collect();
    assert_eq!(strs, vec!["hel", "ell", "llo"]);
}

#[test]
fn build_tokenizer_full_french() {
    let t = build_tokenizer(&TokenizerKind::Full {
        language: StemLanguage::French,
        stopwords: true,
        stem: true,
    });
    let tokens = t.tokenize("chats");
    assert!(!tokens.is_empty(), "French stemmer should produce output");
}
