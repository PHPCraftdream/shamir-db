//! Cross-language msgpack fixture + wire round-trip for the distinct valid
//! `CreateIndex` shapes, plus the invalid-combination rejection parity that
//! `try_build()` enforces.
//!
//! This is the Rust half of the client/server parity contract (F-81 #908). It
//! pins three invariants:
//!
//! * **Part A — fixture regression:** rebuilding each canonical valid shape
//!   through `CreateIndex::try_build()` and serializing with
//!   `rmp_serde::to_vec_named` reproduces the exact hex bytes captured in
//!   [`fixtures/create_index_try_build_msgpack.json`]. Any drift in the Rust
//!   wire shape (a renamed field, a reordered struct field, a changed
//!   `skip_serializing_if`) fails here.
//!
//! * **Part B — wire round-trip:** the bytes the builder produces decode back
//!   to the *same* `BatchOp` variant through the server's decode path
//!   (`bytes → QueryValue → BatchOp`), proving the builder's output is
//!   server-parseable.
//!
//! * **Part C — invalid-combination rejection (parity):** `try_build()` rejects
//!   every invalid combination that `admin_table_index.rs` rejects at
//!   DDL-execution time. Same invalid input, same rejection, now enforced
//!   twice (defense in depth) instead of once.
//!
//! Determinism: every input string is a fixed literal. `rmp_serde::to_vec_named`
//! emits maps with keys in **struct declaration order**.

use shamir_query_builder::ddl::{create_index, CreateIndexBuildError};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;
use std::collections::BTreeMap;

// ============================================================================
// Canonical valid shapes — one entry per distinct CreateIndex family.
// ============================================================================

/// Build each of the distinct valid `CreateIndex` shapes through
/// `try_build()`, returning the op label + the finalized `BatchOp`.
///
/// Every shape goes through `try_build().unwrap()` to prove the validating
/// path accepts it.
fn canonical_valid_ops() -> Vec<(&'static str, BatchOp)> {
    vec![
        // (a) Regular (btree default) — hash-keyed equality index.
        (
            "regular",
            create_index("idx_regular", "users")
                .field("email")
                .try_build()
                .unwrap(),
        ),
        // (b) Unique — hash-keyed unique index with constraint check.
        (
            "unique",
            create_index("idx_unique", "users")
                .field("email")
                .unique()
                .try_build()
                .unwrap(),
        ),
        // (c) Sorted + include — value-ordered index with a covering field.
        (
            "sorted_include",
            create_index("idx_sorted_inc", "users")
                .field("age")
                .sorted()
                .include([vec!["email".to_string()]])
                .try_build()
                .unwrap(),
        ),
        // (d) FTS — full-text search index.
        (
            "fts",
            create_index("idx_fts", "posts")
                .field("body")
                .index_type("fts")
                .fts_tokenizer("whitespace")
                .try_build()
                .unwrap(),
        ),
        // (e) Functional — functional index (lower operator).
        (
            "functional",
            create_index("idx_func", "users")
                .field("email")
                .index_type("functional")
                .functional_op("lower")
                .try_build()
                .unwrap(),
        ),
        // (f) Vector — HNSW vector index.
        (
            "vector",
            create_index("idx_vector", "docs")
                .field("embedding")
                .index_type("vector")
                .vector_dim(384)
                .vector_metric("cosine")
                .try_build()
                .unwrap(),
        ),
    ]
}

/// Load the fixture, returning only the label → hex entries (ignoring the
/// `_`-prefixed documentation keys).
fn load_fixture() -> BTreeMap<String, String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/create_index_try_build_msgpack.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
    let raw: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture JSON: {e}"));
    let obj = raw.as_object().expect("fixture root must be a JSON object");
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if k.starts_with('_') {
            continue;
        }
        let hex = v
            .as_str()
            .unwrap_or_else(|| panic!("fixture entry `{k}` must be a hex string, got {v}"));
        out.insert(k.clone(), hex.to_string());
    }
    out
}

// ============================================================================
// Part A — fixture regression: builder output matches the captured hex.
// ============================================================================

/// Helper: serialize an op and print the hex (used to (re)generate the
/// fixture JSON — run with `--nocapture` to see the hex on stdout).
#[test]
fn generate_fixture_hex() {
    for (label, op) in canonical_valid_ops() {
        let bytes =
            rmp_serde::to_vec_named(&op).unwrap_or_else(|e| panic!("serialize {label}: {e}"));
        let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        println!("GEN  \"{label}\": \"{hex}\",");
    }
}

#[test]
fn fixture_has_exactly_six_shapes() {
    let fx = load_fixture();
    let canonical: Vec<&str> = canonical_valid_ops().iter().map(|(k, _)| *k).collect();
    let fixture_keys: Vec<&str> = fx.keys().map(|s| s.as_str()).collect();
    assert_eq!(
        fixture_keys.len(),
        6,
        "fixture must pin exactly 6 shapes; got {fixture_keys:?}"
    );
    for label in &canonical {
        assert!(
            fx.contains_key(*label),
            "fixture missing `{label}`; have {:?}",
            fx.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn builder_reproduces_fixture_hex() {
    let fx = load_fixture();
    for (label, op) in canonical_valid_ops() {
        let bytes =
            rmp_serde::to_vec_named(&op).unwrap_or_else(|e| panic!("serialize {label}: {e}"));
        let actual_hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        let expected_hex = fx
            .get(label)
            .unwrap_or_else(|| panic!("fixture has no entry for `{label}`"));
        assert_eq!(
            actual_hex, *expected_hex,
            "wire-shape drift for `{label}`: builder produced {actual_hex}, fixture expects {expected_hex}. \
             Check for renamed fields, reordered struct fields, or a changed skip_serializing_if."
        );
    }
}

// ============================================================================
// Part B — wire round-trip: builder bytes → QueryValue → BatchOp (server path).
// ============================================================================

#[test]
fn try_build_output_round_trips_through_server_decode_path() {
    for (label, op) in canonical_valid_ops() {
        // 1. Builder → wire bytes.
        let bytes =
            rmp_serde::to_vec_named(&op).unwrap_or_else(|e| panic!("serialize {label}: {e}"));

        // 2. Wire bytes → QueryValue (the intermediate map form the server
        //    buffers in `BatchOp::deserialize`).
        let qv: QueryValue = rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("decode {label} bytes → QueryValue: {e}"));

        // 3. QueryValue → BatchOp via the server's discriminator dispatch.
        let qv_bytes = rmp_serde::to_vec_named(&qv)
            .unwrap_or_else(|e| panic!("re-encode {label} QueryValue: {e}"));
        let decoded: BatchOp = rmp_serde::from_slice(&qv_bytes)
            .unwrap_or_else(|e| panic!("decode {label} QueryValue → BatchOp: {e}"));

        assert_eq!(
            decoded, op,
            "wire round-trip mismatch for `{label}`: the builder's bytes did not decode back to the same BatchOp variant"
        );
    }
}

// ============================================================================
// Part C — invalid-combination rejection (client/server parity).
// ============================================================================
//
// Each test builds an invalid op that the server rejects in
// `admin_table_index.rs` (lines 386-390) and asserts `try_build()` rejects
// it with the matching `CreateIndexBuildError` variant — BEFORE any server
// round trip.

/// `unique && sorted` is rejected with [`CreateIndexBuildError::UniqueAndSorted`].
/// Server message: "Index cannot be both sorted and unique".
#[test]
fn try_build_rejects_unique_and_sorted() {
    let result = create_index("idx_bad", "users")
        .field("email")
        .unique()
        .sorted()
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::UniqueAndSorted),
        "unique + sorted must be rejected at construction time"
    );
}

/// `include` without `sorted` is rejected with
/// [`CreateIndexBuildError::IncludeWithoutSorted`]. Server message:
/// "include is only valid for sorted indexes".
#[test]
fn try_build_rejects_include_without_sorted() {
    let result = create_index("idx_bad", "users")
        .field("email")
        .include([vec!["name".to_string()]])
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::IncludeWithoutSorted),
        "include without sorted must be rejected at construction time"
    );
}

/// `sorted` with ≠1 fields is rejected with
/// [`CreateIndexBuildError::SortedMultiField`]. Server message:
/// "Sorted index requires exactly one field (composite TBD)".
#[test]
fn try_build_rejects_sorted_multi_field() {
    let result = create_index("idx_bad", "users")
        .fields([vec!["a".to_string()], vec!["b".to_string()]])
        .sorted()
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::SortedMultiField { field_count: 2 }),
        "sorted with 2 fields must be rejected at construction time"
    );
}

/// Sanity: `build()` (the infallible path) does NOT reject these — it stays
/// permissive for backward compatibility. This proves `try_build()` is the
/// *opt-in* validating sibling, not a replacement.
#[test]
fn build_remains_permissive_for_invalid_combos() {
    // unique + sorted — build() accepts it; try_build() does not.
    let op = create_index("idx_bad", "users")
        .field("email")
        .unique()
        .sorted()
        .build();
    assert!(
        matches!(op, BatchOp::CreateIndex(_)),
        "build() must remain permissive (not reject unique+sorted)"
    );
    // try_build() rejects the same input.
    assert!(matches!(
        create_index("idx_bad", "users")
            .field("email")
            .unique()
            .sorted()
            .try_build(),
        Err(CreateIndexBuildError::UniqueAndSorted)
    ));
}

// ============================================================================
// Part C (continued) — P1-6 (#970): cross-type validation rejection parity.
// ============================================================================
//
// Each test mirrors a NEW check added in P1-6, which the server enforces in
// `admin_table_index.rs` BEFORE the non-btree dispatch (closing the
// "unique silently ignored for non-btree types" hole).

/// No fields → [`CreateIndexBuildError::EmptyFields`].
/// Server message: "CREATE INDEX requires at least one field".
#[test]
fn try_build_rejects_empty_fields() {
    let result = create_index("idx_bad", "users").try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::EmptyFields),
        "empty fields must be rejected at construction time"
    );
}

/// `unique` with `index_type("vector")` →
/// [`CreateIndexBuildError::UniqueUnsupportedForType`].
/// Server message: "`unique` is not supported for 'vector' indexes; ...".
#[test]
fn try_build_rejects_unique_with_vector() {
    let result = create_index("idx_bad", "users")
        .field("embedding")
        .unique()
        .index_type("vector")
        .vector_dim(128)
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::UniqueUnsupportedForType {
            index_type: "vector".to_string()
        }),
        "unique + vector must be rejected at construction time"
    );
}

/// `sorted` with `index_type("fts")` →
/// [`CreateIndexBuildError::SortedUnsupportedForType`].
/// Server message: "`sorted` is not supported for 'fts' indexes".
#[test]
fn try_build_rejects_sorted_with_fts() {
    let result = create_index("idx_bad", "posts")
        .field("body")
        .sorted()
        .index_type("fts")
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::SortedUnsupportedForType {
            index_type: "fts".to_string()
        }),
        "sorted + fts must be rejected at construction time"
    );
}

/// Vector index with no `vector_dim` →
/// [`CreateIndexBuildError::VectorDimRequired`].
/// Server message: "vector index requires `vector_dim` > 0".
#[test]
fn try_build_rejects_vector_missing_dim() {
    let result = create_index("idx_bad", "docs")
        .field("embedding")
        .index_type("vector")
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::VectorDimRequired),
        "vector index without vector_dim must be rejected at construction time"
    );
}

/// Vector index with `vector_dim(0)` →
/// [`CreateIndexBuildError::VectorDimRequired`].
#[test]
fn try_build_rejects_vector_zero_dim() {
    let result = create_index("idx_bad", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(0)
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::VectorDimRequired),
        "vector index with vector_dim=0 must be rejected at construction time"
    );
}

/// Vector index with `vector_dim(1)` is accepted (boundary positive).
#[test]
fn try_build_accepts_vector_dim_one() {
    let result = create_index("idx_ok", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(1)
        .try_build();
    assert!(result.is_ok(), "vector_dim=1 must be accepted");
}

/// Vector index with misspelled metric →
/// [`CreateIndexBuildError::UnknownVectorMetric`].
/// Server message: "unknown vector_metric 'consine'; expected ...".
#[test]
fn try_build_rejects_unknown_vector_metric() {
    let result = create_index("idx_bad", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(128)
        .vector_metric("consine")
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::UnknownVectorMetric {
            metric: "consine".to_string()
        }),
        "unknown vector_metric must be rejected at construction time"
    );
}

/// Vector options on a non-vector index →
/// [`CreateIndexBuildError::VectorOptionsOnNonVectorIndex`].
/// Server message: "vector_dim/... are only valid for 'vector' indexes".
#[test]
fn try_build_rejects_vector_options_on_non_vector() {
    let result = create_index("idx_bad", "users")
        .field("email")
        .vector_dim(128)
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::VectorOptionsOnNonVectorIndex),
        "vector_dim on a non-vector index must be rejected"
    );
}

/// FTS options on a non-FTS index →
/// [`CreateIndexBuildError::FtsOptionsOnNonFtsIndex`].
/// Server message: "fts_tokenizer/fts_language are only valid for 'fts' indexes".
#[test]
fn try_build_rejects_fts_options_on_non_fts() {
    let result = create_index("idx_bad", "users")
        .field("email")
        .fts_tokenizer("whitespace")
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::FtsOptionsOnNonFtsIndex),
        "fts_tokenizer on a non-fts index must be rejected"
    );
}

/// Functional options on a non-functional index →
/// [`CreateIndexBuildError::FunctionalOptionsOnNonFunctionalIndex`].
/// Server message: "functional_op/functional_args are only valid for 'functional' indexes".
#[test]
fn try_build_rejects_functional_options_on_non_functional() {
    let result = create_index("idx_bad", "users")
        .field("email")
        .functional_op("lower")
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::FunctionalOptionsOnNonFunctionalIndex),
        "functional_op on a non-functional index must be rejected"
    );
}

/// `include` with `index_type("fts")` →
/// [`CreateIndexBuildError::IncludeUnsupportedForType`].
/// Server message: "`include` is not supported for 'fts' indexes; covering
/// fields are only valid for sorted indexes".
#[test]
fn try_build_rejects_include_with_fts() {
    let result = create_index("idx_bad", "posts")
        .field("body")
        .index_type("fts")
        .include([vec!["title".to_string()]])
        .try_build();
    assert_eq!(
        result,
        Err(CreateIndexBuildError::IncludeUnsupportedForType {
            index_type: "fts".to_string()
        }),
        "include + fts must be rejected at construction time"
    );
}

/// Boundary: valid vector / fts / functional shapes are NOT rejected.
#[test]
fn try_build_accepts_valid_specialized_indexes() {
    // vector with all valid options.
    assert!(create_index("idx_vec", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(384)
        .vector_metric("cosine")
        .try_build()
        .is_ok());
    // fts with valid tokenizer.
    assert!(create_index("idx_fts", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("unicode")
        .try_build()
        .is_ok());
    // functional with valid op.
    assert!(create_index("idx_fn", "users")
        .field("email")
        .index_type("functional")
        .functional_op("lower")
        .try_build()
        .is_ok());
}
