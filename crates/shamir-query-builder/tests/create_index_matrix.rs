//! Matrix-driven CREATE INDEX validation + wire-contract test.
//!
//! Reads `tests/fixtures/create_index_matrix.json` (the shared fixture consumed
//! by BOTH this Rust test and the TS test
//! `create_index_matrix.test.ts`), drives `CreateIndex::try_build()` for each
//! case, and asserts:
//!
//! * **accept** cases: `try_build()` returns `Ok`, and the serialized wire bytes
//!   match the `wire_hex` captured in the fixture.
//! * **reject** cases: `try_build()` returns `Err`, and the error message
//!   contains the `reason_contains` substring (case-insensitive).
//!
//! This file replaces the former `create_index_try_build_msgpack.rs` (which
//! hand-wrote each case as a separate `#[test]` fn). The matrix JSON is now the
//! single source of truth — adding a case there automatically extends both the
//! Rust and TS test surfaces.
//!
//! The wire round-trip test (Part B of the old file — builder bytes → QueryValue
//! → BatchOp) is preserved here as a separate `#[test]` because it is about
//! server decode-path compatibility, not about accept/reject validation.

use shamir_query_builder::ddl::{create_index, CreateIndex, CreateIndexBuildError};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

use serde::Deserialize;

// ============================================================================
// Fixture schema + loader
// ============================================================================

/// One field path in the matrix (e.g. `["email"]` → `vec!["email".to_string()]`).
type FieldPath = Vec<String>;

/// The declarative input for a single CREATE INDEX case.
#[derive(Debug, Deserialize)]
struct CaseInput {
    name: String,
    table: String,
    fields: Vec<FieldPath>,
    #[serde(default)]
    unique: bool,
    #[serde(default)]
    sorted: bool,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    index_type: Option<String>,
    #[serde(default)]
    fts_tokenizer: Option<String>,
    #[serde(default)]
    fts_language: Option<String>,
    #[serde(default)]
    functional_op: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // parsed for fixture-completeness; no test case uses it yet
    functional_args: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    vector_dim: Option<u32>,
    #[serde(default)]
    vector_metric: Option<String>,
    #[serde(default)]
    vector_quantization: Option<String>,
    #[serde(default)]
    include: Option<Vec<FieldPath>>,
    #[serde(default)]
    if_not_exists: bool,
}

/// A single matrix case (valid or invalid).
#[derive(Debug, Deserialize)]
#[serde(tag = "expect", rename_all = "lowercase")]
enum MatrixCase {
    Accept {
        name: String,
        input: CaseInput,
        wire_hex: String,
    },
    Reject {
        name: String,
        input: CaseInput,
        reason_contains: String,
    },
}

/// The top-level fixture object.
#[derive(Debug, Deserialize)]
struct Fixture {
    #[serde(default)]
    _comment: Option<String>,
    #[serde(default)]
    _key_order_note: Option<String>,
    #[serde(default)]
    _value_notes: Option<serde_json::Value>,
    #[serde(default)]
    _consumer_notes: Option<serde_json::Value>,
    cases: Vec<MatrixCase>,
}

fn load_fixture() -> Fixture {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/create_index_matrix.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture JSON: {e}"))
}

// ============================================================================
// Input → builder mapping
// ============================================================================

/// Map a matrix `CaseInput` to the Rust `CreateIndex` fluent builder.
fn build_from_input(input: &CaseInput) -> CreateIndex {
    let mut b = create_index(input.name.clone(), input.table.clone()).fields(input.fields.clone());
    if input.unique {
        b = b.unique();
    }
    if input.sorted {
        b = b.sorted();
    }
    if let Some(repo) = &input.repo {
        b = b.repo(repo.clone());
    }
    if let Some(t) = &input.index_type {
        b = b.index_type(t.clone());
    }
    if let Some(tok) = &input.fts_tokenizer {
        b = b.fts_tokenizer(tok.clone());
    }
    if let Some(lang) = &input.fts_language {
        b = b.fts_language(lang.clone());
    }
    if let Some(op) = &input.functional_op {
        b = b.functional_op(op.clone());
    }
    if let Some(dim) = input.vector_dim {
        b = b.vector_dim(dim);
    }
    if let Some(metric) = &input.vector_metric {
        b = b.vector_metric(metric.clone());
    }
    if let Some(q) = &input.vector_quantization {
        b = b.vector_quantization(q.clone());
    }
    if let Some(inc) = &input.include {
        b = b.include(inc.clone());
    }
    if input.if_not_exists {
        b = b.if_not_exists();
    }
    b
}

// ============================================================================
// Tests
// ============================================================================

/// Helper: serialize a BatchOp to hex.
fn batch_op_to_hex(op: &BatchOp) -> String {
    let bytes = rmp_serde::to_vec_named(op).expect("serialize op");
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[test]
fn matrix_all_accept_cases_build_and_match_wire_hex() {
    let fx = load_fixture();
    for case in &fx.cases {
        let MatrixCase::Accept {
            name,
            input,
            wire_hex,
        } = case
        else {
            continue;
        };
        let builder = build_from_input(input);
        let result = builder.try_build();
        let op = result.unwrap_or_else(|e| {
            panic!("case `{name}`: expected accept but try_build() returned Err: {e}")
        });

        let actual_hex = batch_op_to_hex(&op);
        assert_eq!(
            actual_hex, *wire_hex,
            "case `{name}`: wire-shape drift. Builder produced {actual_hex}, fixture expects {wire_hex}. \
             Check for renamed fields, reordered struct fields, or a changed skip_serializing_if."
        );
    }
}

#[test]
fn matrix_all_reject_cases_fail_with_reason() {
    let fx = load_fixture();
    for case in &fx.cases {
        let MatrixCase::Reject {
            name,
            input,
            reason_contains,
        } = case
        else {
            continue;
        };
        let builder = build_from_input(input);
        let result = builder.try_build();
        let err = match result {
            Ok(_) => panic!("case `{name}`: expected reject but try_build() returned Ok"),
            Err(e) => e,
        };
        let err_str = err.to_string();
        assert!(
            err_str
                .to_lowercase()
                .contains(&reason_contains.to_lowercase()),
            "case `{name}`: error message `{err_str}` does not contain `{reason_contains}`"
        );
    }
}

/// Map a `CreateIndexBuildError` to a stable variant tag. Used ONLY by
/// `matrix_reject_cases_cover_all_error_variants` to prove ACTUAL coverage —
/// not a count comparison, which a prior version of this test relied on and
/// which an `@oh` review found could pass at 12 reject cases even if the
/// SOLE case for some variant were deleted (as long as some OTHER variant
/// picked up a second case to keep the total at 12). Every arm is required
/// (no `_ =>`), so adding a 13th `CreateIndexBuildError` variant without
/// updating this function is a compile error, not a silent coverage gap.
fn variant_tag(e: &CreateIndexBuildError) -> &'static str {
    match e {
        CreateIndexBuildError::UniqueAndSorted => "UniqueAndSorted",
        CreateIndexBuildError::IncludeWithoutSorted => "IncludeWithoutSorted",
        CreateIndexBuildError::SortedMultiField { .. } => "SortedMultiField",
        CreateIndexBuildError::EmptyFields => "EmptyFields",
        CreateIndexBuildError::UniqueUnsupportedForType { .. } => "UniqueUnsupportedForType",
        CreateIndexBuildError::SortedUnsupportedForType { .. } => "SortedUnsupportedForType",
        CreateIndexBuildError::VectorDimRequired => "VectorDimRequired",
        CreateIndexBuildError::UnknownVectorMetric { .. } => "UnknownVectorMetric",
        CreateIndexBuildError::VectorOptionsOnNonVectorIndex => "VectorOptionsOnNonVectorIndex",
        CreateIndexBuildError::FtsOptionsOnNonFtsIndex => "FtsOptionsOnNonFtsIndex",
        CreateIndexBuildError::FunctionalOptionsOnNonFunctionalIndex => {
            "FunctionalOptionsOnNonFunctionalIndex"
        }
        CreateIndexBuildError::IncludeUnsupportedForType { .. } => "IncludeUnsupportedForType",
    }
}

#[test]
fn matrix_reject_cases_cover_all_error_variants() {
    // Actually run every reject case through try_build() and collect the
    // REAL error variant each one produces — not just the case count. A
    // variant can legitimately have >1 case (#1004 added a second
    // `VectorDimRequired` case, `vector_dim_zero_rejected`, alongside the
    // pre-existing omitted-dim case, as a boundary-value pair), so counting
    // cases can never prove per-variant coverage on its own.
    let fx = load_fixture();
    let mut covered: shamir_collections::TSet<&'static str> = shamir_collections::TSet::default();
    let mut case_count = 0usize;
    for case in &fx.cases {
        let MatrixCase::Reject { name, input, .. } = case else {
            continue;
        };
        case_count += 1;
        let err = build_from_input(input)
            .try_build()
            .err()
            .unwrap_or_else(|| {
                panic!("case `{name}`: expected reject but try_build() returned Ok")
            });
        covered.insert(variant_tag(&err));
    }

    let expected_variants = [
        "UniqueAndSorted",
        "IncludeWithoutSorted",
        "SortedMultiField",
        "EmptyFields",
        "UniqueUnsupportedForType",
        "SortedUnsupportedForType",
        "VectorDimRequired",
        "UnknownVectorMetric",
        "VectorOptionsOnNonVectorIndex",
        "FtsOptionsOnNonFtsIndex",
        "FunctionalOptionsOnNonFunctionalIndex",
        "IncludeUnsupportedForType",
    ];
    assert_eq!(
        expected_variants.len(),
        12,
        "sanity: expected exactly 12 error variants"
    );
    for variant in expected_variants {
        assert!(
            covered.contains(variant),
            "matrix has no reject case that actually produces \
             CreateIndexBuildError::{variant} — a case count alone cannot \
             prove this, {case_count} reject cases were run and produced: {covered:?}"
        );
    }
}

/// Build() (the infallible path) must remain permissive — it does NOT reject
/// invalid combos. This proves try_build() is the opt-in validating sibling.
#[test]
fn build_remains_permissive_for_invalid_combos() {
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

/// Wire round-trip: builder bytes → QueryValue → BatchOp (server decode path).
/// Preserved from the original create_index_try_build_msgpack.rs — proves the
/// builder's serialized output is server-parseable.
#[test]
fn try_build_output_round_trips_through_server_decode_path() {
    let fx = load_fixture();
    for case in &fx.cases {
        let MatrixCase::Accept { name, input, .. } = case else {
            continue;
        };
        let op = build_from_input(input)
            .try_build()
            .unwrap_or_else(|e| panic!("case `{name}`: try_build() failed during round-trip: {e}"));

        // 1. Builder → wire bytes.
        let bytes =
            rmp_serde::to_vec_named(&op).unwrap_or_else(|e| panic!("serialize {name}: {e}"));

        // 2. Wire bytes → QueryValue (the intermediate map form the server
        //    buffers in BatchOp::deserialize).
        let qv: QueryValue = rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("decode {name} bytes → QueryValue: {e}"));

        // 3. QueryValue → BatchOp via the server's discriminator dispatch.
        let qv_bytes = rmp_serde::to_vec_named(&qv)
            .unwrap_or_else(|e| panic!("re-encode {name} QueryValue: {e}"));
        let decoded: BatchOp = rmp_serde::from_slice(&qv_bytes)
            .unwrap_or_else(|e| panic!("decode {name} QueryValue → BatchOp: {e}"));

        assert_eq!(
            decoded, op,
            "wire round-trip mismatch for `{name}`: the builder's bytes did not decode back to the same BatchOp variant"
        );
    }
}

/// Helper: (re)generate the hex for all accept cases — run with `--nocapture`
/// to see the hex on stdout. Useful when the wire shape changes.
#[test]
fn generate_accept_case_hex() {
    let fx = load_fixture();
    for case in &fx.cases {
        let MatrixCase::Accept { name, input, .. } = case else {
            continue;
        };
        let op = build_from_input(input).try_build().expect("accept case");
        let hex = batch_op_to_hex(&op);
        println!("GEN  \"{name}\": \"{hex}\",");
    }
}
