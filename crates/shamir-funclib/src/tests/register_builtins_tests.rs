//! Top-level wiring tests for [`crate::register_builtins`].
//!
//! These verify that every category module is actually registered under its
//! folder prefix, and that previously colliding names now coexist.

use crate::register_builtins;

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
        ("strings", "strings/lower"),
        ("arrays", "arrays/length"),
        ("cast", "cast/to_int"),
        ("datetime", "datetime/now"),
        ("json", "json/type_of"),
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
    // json/keys and object/keys both exist (previously last-wins dropped one).
    assert!(
        reg.get("json/keys").is_some(),
        "json/keys missing — collision not resolved"
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
