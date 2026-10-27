//! Top-level wiring tests for [`crate::register_builtins`].
//!
//! These verify that every category module is actually registered under its
//! folder prefix, and that previously colliding names now coexist.

use crate::register_builtins;
use shamir_types::types::value::QueryValue;

#[test]
fn register_builtins_is_non_empty() {
    let reg = register_builtins();
    assert!(
        !reg.is_empty(),
        "register_builtins produced an empty registry"
    );
    // With folder-qualified names no collisions occur; all functions survive.
    assert!(
        reg.len() >= 130,
        "expected at least 130 functions (no drops), got {}",
        reg.len()
    );
}

#[test]
fn register_builtins_contains_a_sample_from_each_category() {
    let reg = register_builtins();
    // One representative name per category, now folder-qualified.
    let samples = [
        ("math", "math/abs"),
        ("null", "null/coalesce"),
        ("strings", "strings/lower"),
        ("arrays", "arrays/length"),
        ("cast", "cast/to_int"),
        ("datetime", "datetime/now"),
        ("value_nav", "value_nav/type_of"),
        ("validate", "validate/is_email"),
        ("encode", "encode/base64_enc"),
        ("object", "object/entries"),
        ("text", "text/slugify"),
        ("crypto", "crypto/sha256"),
    ];
    for (category, name) in samples {
        assert!(
            reg.get(name).is_some(),
            "category `{category}` missing from register_builtins (no `{name}`)"
        );
    }
}

#[test]
fn previously_colliding_names_now_coexist() {
    let reg = register_builtins();
    // value_nav/keys and object/keys both exist (previously last-wins dropped one).
    assert!(
        reg.get("value_nav/keys").is_some(),
        "value_nav/keys missing — collision not resolved"
    );
    assert!(
        reg.get("object/keys").is_some(),
        "object/keys missing — collision not resolved"
    );
    // math/min and arrays/min both exist.
    assert!(
        reg.get("math/min").is_some(),
        "math/min missing — collision not resolved"
    );
    assert!(
        reg.get("arrays/min").is_some(),
        "arrays/min missing — collision not resolved"
    );
    // math/max and arrays/max both exist.
    assert!(
        reg.get("math/max").is_some(),
        "math/max missing — collision not resolved"
    );
    assert!(
        reg.get("arrays/max").is_some(),
        "arrays/max missing — collision not resolved"
    );
}

#[test]
fn rv_funclib_1ef53ab4_arg_i64_f64_exclusive_upper_bound() {
    // `arg_i64`'s F64 arm (registry.rs): 2^63 == i64::MAX as f64 must be
    // rejected ("out_of_range") instead of being admitted and saturating
    // in the `as i64` cast. Surfaced through `datetime/to_epoch_s`.
    let reg = register_builtins();
    let two_pow_63 = 9_223_372_036_854_775_808.0f64;
    assert_eq!(
        reg.call("datetime/to_epoch_s", &[QueryValue::F64(two_pow_63)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}
