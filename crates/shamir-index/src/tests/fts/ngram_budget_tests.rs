use crate::backends::backend::IndexError;
use crate::fts::ngram_budget::{MAX_NGRAM_BYTES_PER_VALUE, MAX_NGRAM_TOKENS_PER_VALUE};
use crate::kind::{StemLanguage, TokenizerKind};
use crate::tokenizer::build_tokenizer;

fn assert_budget(error: IndexError, kind: &str, limit: u64, observed: u64) {
    assert!(matches!(error, IndexError::TokenBudgetExceeded {
        limit_kind, limit: actual_limit, observed: actual_observed
    } if limit_kind == kind && actual_limit == limit && actual_observed == observed));
}

#[test]
fn count_boundary_and_cumulative_words() {
    let t = build_tokenizer(&TokenizerKind::Ngram { n: 1 });
    let half = "a".repeat((MAX_NGRAM_TOKENS_PER_VALUE / 2) as usize);
    let text = format!("{half} {half}");
    assert_eq!(
        t.try_tokenize(&text).unwrap().len(),
        MAX_NGRAM_TOKENS_PER_VALUE as usize
    );
    let excess = format!("{text} a");
    assert_budget(
        t.try_tokenize(&excess).unwrap_err(),
        "tokens",
        MAX_NGRAM_TOKENS_PER_VALUE,
        MAX_NGRAM_TOKENS_PER_VALUE + 1,
    );
}

#[test]
fn bytes_boundary_and_cumulative_words() {
    let t = build_tokenizer(&TokenizerKind::Ngram { n: 17 });
    // Each 17-character word yields exactly one 17-byte gram.
    let words = (MAX_NGRAM_BYTES_PER_VALUE / 17) as usize;
    let remainder = (MAX_NGRAM_BYTES_PER_VALUE % 17) as usize;
    let text = format!(
        "{}{}",
        "abcdefghijklmnopq ".repeat(words),
        "a".repeat(remainder)
    );
    assert!(t.try_tokenize(&text).is_ok());
    let excess = format!("{text} a");
    assert_budget(
        t.try_tokenize(&excess).unwrap_err(),
        "bytes",
        MAX_NGRAM_BYTES_PER_VALUE,
        MAX_NGRAM_BYTES_PER_VALUE + 1,
    );
}

#[test]
fn lowercase_expansion_is_budgeted_after_conversion() {
    let t = build_tokenizer(&TokenizerKind::Ngram { n: 1 });
    let text = "İ".repeat((MAX_NGRAM_TOKENS_PER_VALUE / 2 + 1) as usize);
    assert_budget(
        t.try_tokenize(&text).unwrap_err(),
        "tokens",
        MAX_NGRAM_TOKENS_PER_VALUE,
        MAX_NGRAM_TOKENS_PER_VALUE + 2,
    );
    assert_eq!(t.try_tokenize("İX").unwrap(), vec!["i", "\u{307}", "x"]);
}

#[test]
fn parity_order_zero_and_short_words() {
    for n in [0, 1, 2, 5] {
        let t = build_tokenizer(&TokenizerKind::Ngram { n });
        for text in ["", "A é!中 İX", "abcd xy", "!!!"] {
            assert_eq!(t.try_tokenize(text).unwrap(), t.tokenize(text));
        }
    }
    let t = build_tokenizer(&TokenizerKind::Ngram { n: 2 });
    assert_eq!(
        t.try_tokenize("Abc X İZ").unwrap(),
        vec!["ab", "bc", "x", "i\u{307}", "\u{307}z"]
    );
    let t = build_tokenizer(&TokenizerKind::Ngram { n: 0 });
    assert_eq!(t.try_tokenize("AB c").unwrap(), vec!["a", "b", "c"]);
}

#[test]
fn budget_error_converts_to_database_error() {
    let error = IndexError::TokenBudgetExceeded {
        limit_kind: "bytes",
        limit: 10,
        observed: 11,
    };
    let converted: shamir_storage::error::DbError = error.into();
    assert!(
        matches!(converted, shamir_storage::error::DbError::Internal(message)
        if message.contains("bytes") && message.contains("11"))
    );
}

#[test]
fn non_ngram_output_is_unchanged() {
    for kind in [
        TokenizerKind::Whitespace,
        TokenizerKind::Unicode,
        TokenizerKind::Full {
            language: StemLanguage::English,
            stopwords: true,
            stem: true,
        },
    ] {
        let t = build_tokenizer(&kind);
        assert_eq!(
            t.try_tokenize("The Running İ cat!").unwrap(),
            t.tokenize("The Running İ cat!")
        );
    }
}
