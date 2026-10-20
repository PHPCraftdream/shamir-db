//! Per-function `/gen` tests — impure/non-deterministic functions.
//!
//! These tests verify output SHAPE (UUID format, range, length) rather than
//! exact values, since every call produces a fresh random result.

use crate::gen;
use crate::register_builtins;
use crate::registry::ScalarRegistry;
use regex::Regex;
use shamir_types::types::value::QueryValue;
use std::sync::LazyLock;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    gen::register(&mut r);
    r
}

/// Strict v4 UUID regex: version nibble is `4`, variant nibble is `8/9/a/b`.
static UUID_V4_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$").unwrap()
});

#[test]
fn uuid_v4_format_is_canonical_v4() {
    let r = reg();
    match r.call("uuid_v4", &[]).unwrap() {
        QueryValue::Str(s) => {
            assert!(
                UUID_V4_RE.is_match(&s),
                "generated UUID `{s}` does not match the v4 canonical format"
            );
        }
        other => panic!("expected Str, got {other:?}"),
    }
}

#[test]
fn uuid_v4_two_calls_differ() {
    // Two calls produce DIFFERENT strings — the function is non-deterministic.
    // (Collision probability for 122 random bits is astronomically low; this
    // is not a retry-on-collision mechanism, just a sanity check.)
    let r = reg();
    let a = r.call("uuid_v4", &[]).unwrap();
    let b = r.call("uuid_v4", &[]).unwrap();
    assert_ne!(a, b, "two uuid_v4() calls produced the same value");
}

#[test]
fn uuid_v4_impurity_metadata() {
    let r = reg();
    let e = r.get("uuid_v4").unwrap();
    assert!(!e.pure, "uuid_v4 must be impure");
    assert!(!e.deterministic, "uuid_v4 must be non-deterministic");
    assert!(!e.trusted_pure, "uuid_v4 must not be trusted_pure");
}

#[test]
fn uuid_v4_arity_rejects_args() {
    let r = reg();
    assert_eq!(
        r.call("uuid_v4", &[QueryValue::Int(1)]).unwrap_err().code,
        "arity"
    );
}

// ---------------------------------------------------------------------------
// random()
// ---------------------------------------------------------------------------

#[test]
fn random_in_unit_range() {
    let r = reg();
    for _ in 0..100 {
        match r.call("random", &[]).unwrap() {
            QueryValue::F64(f) => {
                assert!(
                    (0.0..1.0).contains(&f),
                    "random() returned {f}, outside [0.0, 1.0)"
                );
            }
            QueryValue::Dec(d) => {
                // v_f64 stores as Dec; check range via f64 conversion.
                let f = d.to_string().parse::<f64>().unwrap();
                assert!(
                    (0.0..1.0).contains(&f),
                    "random() returned {d}, outside [0.0, 1.0)"
                );
            }
            other => panic!("expected numeric, got {other:?}"),
        }
    }
}

#[test]
fn random_impurity_metadata() {
    let r = reg();
    let e = r.get("random").unwrap();
    assert!(!e.pure);
    assert!(!e.deterministic);
}

// ---------------------------------------------------------------------------
// random_bytes(n)
// ---------------------------------------------------------------------------

#[test]
fn random_bytes_correct_length() {
    let r = reg();
    for n in [0, 1, 16, 32, 256] {
        match r.call("random_bytes", &[QueryValue::Int(n)]).unwrap() {
            QueryValue::Bin(b) => assert_eq!(b.len(), n as usize, "random_bytes({n}) length"),
            other => panic!("expected Bin, got {other:?}"),
        }
    }
}

#[test]
fn random_bytes_negative_is_error() {
    let r = reg();
    assert_eq!(
        r.call("random_bytes", &[QueryValue::Int(-1)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn random_bytes_impurity_metadata() {
    let r = reg();
    let e = r.get("random_bytes").unwrap();
    assert!(!e.pure);
    assert!(!e.deterministic);
}

// ---------------------------------------------------------------------------
// register_builtins wiring regression
// ---------------------------------------------------------------------------

#[test]
fn register_builtins_exposes_gen_folder_qualified_names() {
    let reg = register_builtins();
    for name in ["gen/uuid_v4", "gen/random", "gen/random_bytes"] {
        assert!(
            reg.get(name).is_some(),
            "expected `{name}` in register_builtins()"
        );
    }
    // Smoke-test: dispatch uuid_v4 through the top-level registry.
    match reg.call("gen/uuid_v4", &[]).unwrap() {
        QueryValue::Str(s) => assert!(UUID_V4_RE.is_match(&s)),
        other => panic!("expected Str, got {other:?}"),
    }
}
