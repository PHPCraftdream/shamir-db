//! Per-function `/text` tests — at least one correct-result assert and one
//! error/edge case per registered function.

use crate::registry::{v_int, ScalarRegistry};
use crate::text;
use rust_decimal::prelude::ToPrimitive;
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    text::register(&mut r);
    r
}

fn s(v: &str) -> QueryValue {
    QueryValue::Str(v.to_string())
}

#[test]
fn normalize_nfc_composes() {
    let r = reg();
    // "e" + combining acute accent (U+0301) -> precomposed "é" (U+00E9).
    let decomposed = "e\u{0301}";
    let composed = "\u{00e9}";
    assert_eq!(
        r.call("normalize_nfc", &[s(decomposed)]).unwrap(),
        s(composed)
    );
    // error: wrong type
    assert_eq!(
        r.call("normalize_nfc", &[QueryValue::Int(1)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn normalize_nfkc_folds_compatibility() {
    let r = reg();
    // Fullwidth digit "１" (U+FF11) folds to ASCII "1" under NFKC.
    assert_eq!(r.call("normalize_nfkc", &[s("\u{ff11}")]).unwrap(), s("1"));
    // error: arity (missing arg)
    assert_eq!(r.call("normalize_nfkc", &[]).unwrap_err().code, "arity");
}

#[test]
fn slugify_basic_and_edges() {
    let r = reg();
    assert_eq!(
        r.call("slugify", &[s("  Hello, World!  ")]).unwrap(),
        s("hello-world")
    );
    // collapses runs and trims edges
    assert_eq!(r.call("slugify", &[s("a---b__c")]).unwrap(), s("a-b-c"));
    // edge: all-separator input yields empty slug
    assert_eq!(r.call("slugify", &[s("***")]).unwrap(), s(""));
}

#[test]
fn levenshtein_distance() {
    let r = reg();
    assert_eq!(
        r.call("levenshtein", &[s("kitten"), s("sitting")]).unwrap(),
        v_int(3)
    );
    // identical strings -> 0
    assert_eq!(
        r.call("levenshtein", &[s("abc"), s("abc")]).unwrap(),
        v_int(0)
    );
    // error: wrong type for second arg
    assert_eq!(
        r.call("levenshtein", &[s("abc"), QueryValue::Int(1)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn jaro_winkler_similarity() {
    let r = reg();
    // identical strings -> exactly 1.0
    match r.call("jaro_winkler", &[s("abc"), s("abc")]).unwrap() {
        QueryValue::Dec(d) => assert_eq!(d.to_f64().unwrap(), 1.0),
        other => panic!("expected Dec, got {other:?}"),
    }
    // similar strings: shared prefix boosts score into (0, 1)
    match r.call("jaro_winkler", &[s("martha"), s("marhta")]).unwrap() {
        QueryValue::Dec(d) => {
            let v = d.to_f64().unwrap();
            assert!(v > 0.9 && v < 1.0, "got {v}");
        }
        other => panic!("expected Dec, got {other:?}"),
    }
    // error: arity (only one arg)
    assert_eq!(r.call("jaro_winkler", &[s("a")]).unwrap_err().code, "arity");
}

#[test]
fn word_count_tokens() {
    let r = reg();
    assert_eq!(
        r.call("word_count", &[s("  the quick  brown\tfox ")])
            .unwrap(),
        v_int(4)
    );
    // edge: empty / whitespace-only string -> 0
    assert_eq!(r.call("word_count", &[s("   ")]).unwrap(), v_int(0));
}

#[test]
fn truncate_ellipsis_shortens() {
    let r = reg();
    // shortened: 3 kept chars + ellipsis, max = 4
    assert_eq!(
        r.call("truncate_ellipsis", &[s("abcdef"), QueryValue::Int(4)])
            .unwrap(),
        s("abc…")
    );
    // no truncation when within max
    assert_eq!(
        r.call("truncate_ellipsis", &[s("abc"), QueryValue::Int(5)])
            .unwrap(),
        s("abc")
    );
    // edge: max == 0 yields empty
    assert_eq!(
        r.call("truncate_ellipsis", &[s("abc"), QueryValue::Int(0)])
            .unwrap(),
        s("")
    );
    // error: negative max
    assert_eq!(
        r.call("truncate_ellipsis", &[s("abc"), QueryValue::Int(-1)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}
