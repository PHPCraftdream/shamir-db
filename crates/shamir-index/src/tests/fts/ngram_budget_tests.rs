use crate::backend::IndexError;
use crate::fts::ngram_budget::{MAX_NGRAM_BYTES_PER_VALUE, MAX_NGRAM_TOKENS_PER_VALUE};
use crate::kind::{StemLanguage, TokenizerKind};
use crate::tokenizer::{build_tokenizer, token_hash};
use shamir_collections::TFxMap;

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
fn rv_index_0695aada_streamed_tokens_match_materialized_output() {
    let over_budget = "A1b2C3d4".repeat(9_000);
    // The budget applies to the EMITTED n-gram bytes, not the raw value:
    // emitted total = (chars - n + 1) * n. For one 72_000-char word with
    // n = 255 that is ~18.3 MB, above MAX_NGRAM_BYTES_PER_VALUE (16 MiB).
    let n = 255_usize;
    let emitted_bytes = (over_budget.len() - n + 1) * n;
    assert!(emitted_bytes as u64 > MAX_NGRAM_BYTES_PER_VALUE);
    let tokenizer = build_tokenizer(&TokenizerKind::Ngram { n: 255 });
    assert!(tokenizer.try_tokenize(&over_budget).is_err());
    let materialized = tokenizer.tokenize(&over_budget);
    let mut expected = TFxMap::default();
    for token in &materialized {
        *expected.entry(token_hash(token)).or_insert(0_u32) += 1;
    }
    let mut streamed = TFxMap::default();
    let mut streamed_len = 0_u32;
    tokenizer.for_each_token_hash(&over_budget, |hash| {
        streamed_len = streamed_len.wrapping_add(1);
        *streamed.entry(hash).or_insert(0_u32) += 1;
    });
    assert_eq!(streamed, expected);
    assert_eq!(streamed_len as usize, materialized.len());
    let cases = ["A é!中 İX", "short", "soup 0123!xyz", over_budget.as_str()];
    for n in [1, 2, 5, 255] {
        let tokenizer = build_tokenizer(&TokenizerKind::Ngram { n });
        for text in cases {
            let expected: Vec<_> = tokenizer
                .tokenize(text)
                .iter()
                .map(|token| token_hash(token))
                .collect();
            let mut actual = Vec::new();
            tokenizer.for_each_token_hash(text, |hash| actual.push(hash));
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn rv_index_0695aada_streamed_hash_frequency_and_length_parity() {
    for (n, text) in [
        (1, "İa é!中"),
        (2, "ab ab x"),
        (4, "cat ox"),
        (255, "A1b2C3d4"),
    ] {
        let tokenizer = build_tokenizer(&TokenizerKind::Ngram { n });
        let tokens = tokenizer.tokenize(text);
        let mut expected = TFxMap::default();
        for token in &tokens {
            *expected.entry(token_hash(token)).or_insert(0_u32) += 1;
        }
        let mut actual = TFxMap::default();
        let mut len = 0_u32;
        tokenizer.for_each_token_hash(text, |hash| {
            len = len.wrapping_add(1);
            *actual.entry(hash).or_insert(0_u32) += 1;
        });
        assert_eq!(actual, expected);
        assert_eq!(len as usize, tokens.len());
    }
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
