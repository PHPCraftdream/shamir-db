use crate::index2::kind::{StemLanguage, TokenizerKind};
use crate::table::table_manager::fts_tokenizer_from_dsl;

// ---- None / whitespace fallback ----

#[test]
fn none_yields_whitespace() {
    assert!(
        matches!(fts_tokenizer_from_dsl(None), TokenizerKind::Whitespace),
        "None → Whitespace"
    );
}

#[test]
fn explicit_whitespace() {
    assert!(
        matches!(
            fts_tokenizer_from_dsl(Some("whitespace")),
            TokenizerKind::Whitespace
        ),
        "\"whitespace\" → Whitespace"
    );
}

// ---- unicode ----

#[test]
fn unicode() {
    assert!(
        matches!(
            fts_tokenizer_from_dsl(Some("unicode")),
            TokenizerKind::Unicode
        ),
        "\"unicode\" → Unicode"
    );
}

// ---- ngram ----

#[test]
fn ngram_bare_defaults_to_3() {
    match fts_tokenizer_from_dsl(Some("ngram")) {
        TokenizerKind::Ngram { n } => assert_eq!(n, 3),
        other => panic!("expected Ngram, got {other:?}"),
    }
}

#[test]
fn ngram2() {
    match fts_tokenizer_from_dsl(Some("ngram2")) {
        TokenizerKind::Ngram { n } => assert_eq!(n, 2),
        other => panic!("expected Ngram{{n:2}}, got {other:?}"),
    }
}

#[test]
fn ngram9() {
    match fts_tokenizer_from_dsl(Some("ngram9")) {
        TokenizerKind::Ngram { n } => assert_eq!(n, 9),
        other => panic!("expected Ngram{{n:9}}, got {other:?}"),
    }
}

#[test]
fn ngram_non_numeric_tail_falls_back_to_3() {
    match fts_tokenizer_from_dsl(Some("ngramX")) {
        TokenizerKind::Ngram { n } => assert_eq!(n, 3, "non-numeric tail → default n=3"),
        other => panic!("expected Ngram, got {other:?}"),
    }
}

// ---- stemmed ----

#[test]
fn stemmed_en() {
    match fts_tokenizer_from_dsl(Some("stemmed_en")) {
        TokenizerKind::Full {
            language,
            stopwords,
            stem,
        } => {
            assert_eq!(language, StemLanguage::English);
            assert!(stopwords);
            assert!(stem);
        }
        other => panic!("expected Full(English), got {other:?}"),
    }
}

#[test]
fn stemmed_english() {
    match fts_tokenizer_from_dsl(Some("stemmed_english")) {
        TokenizerKind::Full {
            language,
            stopwords,
            stem,
        } => {
            assert_eq!(language, StemLanguage::English);
            assert!(stopwords);
            assert!(stem);
        }
        other => panic!("expected Full(English), got {other:?}"),
    }
}

#[test]
fn stemmed_fr() {
    match fts_tokenizer_from_dsl(Some("stemmed_fr")) {
        TokenizerKind::Full {
            language,
            stopwords,
            stem,
        } => {
            assert_eq!(language, StemLanguage::French);
            assert!(stopwords);
            assert!(stem);
        }
        other => panic!("expected Full(French), got {other:?}"),
    }
}

#[test]
fn stemmed_french() {
    match fts_tokenizer_from_dsl(Some("stemmed_french")) {
        TokenizerKind::Full {
            language,
            stopwords,
            stem,
        } => {
            assert_eq!(language, StemLanguage::French);
            assert!(stopwords);
            assert!(stem);
        }
        other => panic!("expected Full(French), got {other:?}"),
    }
}

#[test]
fn stemmed_ru() {
    match fts_tokenizer_from_dsl(Some("stemmed_ru")) {
        TokenizerKind::Full {
            language,
            stopwords,
            stem,
        } => {
            assert_eq!(language, StemLanguage::Russian);
            assert!(stopwords);
            assert!(stem);
        }
        other => panic!("expected Full(Russian), got {other:?}"),
    }
}

// ---- unknown stemmed language → Whitespace fallback ----

#[test]
fn stemmed_klingon_falls_back_to_whitespace() {
    assert!(
        matches!(
            fts_tokenizer_from_dsl(Some("stemmed_klingon")),
            TokenizerKind::Whitespace
        ),
        "unknown language → Whitespace fallback"
    );
}

// ---- garbage → Whitespace fallback ----

#[test]
fn garbage_falls_back_to_whitespace() {
    assert!(
        matches!(
            fts_tokenizer_from_dsl(Some("garbage")),
            TokenizerKind::Whitespace
        ),
        "unrecognised spec → Whitespace fallback"
    );
}
