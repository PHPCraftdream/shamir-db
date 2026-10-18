//! Tests for the typed [`IndexSpec`] IR and its conversions.
//!
//! These test the NEW internal conversion surface directly (not just its
//! outward effect through `CreateIndex::try_build()`):
//!
//! - one round-trip test per variant: an [`IndexSpec`] built by hand flattens
//!   via [`IndexSpec::into_op`] into exactly the expected [`CreateIndexOp`];
//! - one rejection test per `CreateIndexBuildError` variant: the
//!   `TryFrom<&CreateIndex>` conversion produces the same `Err` variant the
//!   pre-refactor inline `try_build()` did;
//! - a `NonZeroU32` proof that `vector_dim == 0` is unrepresentable;
//! - a parity test that, for every input `try_build()` accepts, the produced
//!   wire bytes are byte-identical to the (unchanged) infallible `build()` —
//!   i.e. `IndexSpec` is a pure refactor with zero wire drift.

use std::num::NonZeroU32;

use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::batch::BatchOp;

use crate::ddl::{create_index, CreateIndex, CreateIndexBuildError, IndexSpec};

/// Serialize a `BatchOp` to lowercase hex (same encoding the matrix fixtures use).
fn batch_op_to_hex(op: &BatchOp) -> String {
    let bytes = rmp_serde::to_vec_named(op).expect("serialize op");
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Pull the `CreateIndexOp` out of a `BatchOp::CreateIndex(_)` (panic otherwise).
fn unwrap_create_index_op(op: BatchOp) -> CreateIndexOp {
    match op {
        BatchOp::CreateIndex(op) => op,
        other => panic!("expected BatchOp::CreateIndex, got {other:?}"),
    }
}

// ============================================================================
// Per-variant round-trip: IndexSpec -> into_op -> CreateIndexOp
// ============================================================================

#[test]
fn hash_variant_round_trips_to_op() {
    let spec = IndexSpec::Hash {
        fields: vec![vec!["email".to_string()]],
        unique: true,
        index_type: None,
    };
    let op = spec.into_op(
        "idx".to_string(),
        "users".to_string(),
        "main".to_string(),
        false,
    );
    assert_eq!(
        op,
        CreateIndexOp {
            create_index: "idx".to_string(),
            table: "users".to_string(),
            fields: vec![vec!["email".to_string()]],
            unique: true,
            sorted: false,
            repo: "main".to_string(),
            index_type: None,
            fts_tokenizer: None,
            fts_language: None,
            functional_op: None,
            functional_args: None,
            vector_dim: None,
            vector_metric: None,
            vector_quantization: None,
            include: Vec::new(),
            if_not_exists: false,
        }
    );
}

#[test]
fn sorted_variant_round_trips_to_op() {
    let spec = IndexSpec::Sorted {
        field: vec!["age".to_string()],
        include: vec![vec!["email".to_string()]],
        index_type: None,
    };
    let op = spec.into_op(
        "idx".to_string(),
        "users".to_string(),
        "main".to_string(),
        false,
    );
    // `field` (singular) re-expands to a single-element `fields`.
    assert_eq!(op.fields, vec![vec!["age".to_string()]]);
    assert!(op.sorted);
    assert!(!op.unique);
    assert_eq!(op.include, vec![vec!["email".to_string()]]);
    assert_eq!(op.index_type, None);
}

#[test]
fn fts_variant_round_trips_to_op() {
    let spec = IndexSpec::Fts {
        fields: vec![vec!["body".to_string()]],
        tokenizer: Some("whitespace".to_string()),
        language: Some("en".to_string()),
    };
    let op = spec.into_op(
        "idx".to_string(),
        "posts".to_string(),
        "main".to_string(),
        false,
    );
    assert_eq!(op.index_type.as_deref(), Some("fts"));
    assert_eq!(op.fts_tokenizer.as_deref(), Some("whitespace"));
    assert_eq!(op.fts_language.as_deref(), Some("en"));
    assert!(op.vector_dim.is_none());
    assert!(op.include.is_empty());
}

#[test]
fn functional_variant_round_trips_to_op() {
    let spec = IndexSpec::Functional {
        fields: vec![vec!["email".to_string()]],
        op: Some("lower".to_string()),
        args: None,
    };
    let op = spec.into_op(
        "idx".to_string(),
        "users".to_string(),
        "main".to_string(),
        false,
    );
    assert_eq!(op.index_type.as_deref(), Some("functional"));
    assert_eq!(op.functional_op.as_deref(), Some("lower"));
    assert!(op.functional_args.is_none());
}

#[test]
fn vector_variant_round_trips_to_op() {
    let spec = IndexSpec::Vector {
        fields: vec![vec!["embedding".to_string()]],
        dim: NonZeroU32::new(384).unwrap(),
        metric: Some("cosine".to_string()),
        quantization: Some("sq8".to_string()),
    };
    let op = spec.into_op(
        "idx".to_string(),
        "docs".to_string(),
        "main".to_string(),
        false,
    );
    assert_eq!(op.index_type.as_deref(), Some("vector"));
    assert_eq!(op.vector_dim, Some(384));
    assert_eq!(op.vector_metric.as_deref(), Some("cosine"));
    assert_eq!(op.vector_quantization.as_deref(), Some("sq8"));
}

// ============================================================================
// NonZeroU32 makes vector_dim == 0 unrepresentable
// ============================================================================

#[test]
fn vector_dim_zero_is_unrepresentable() {
    // 1. NonZeroU32 cannot hold 0 at the type level — the only constructor
    //    yields `None` for 0, so you cannot spell
    //    `IndexSpec::Vector { dim: <zero>, .. }` without going through this
    //    (fallible) constructor.
    assert_eq!(NonZeroU32::new(0), None);

    // 2. Therefore the TryFrom conversion maps BOTH an omitted and an explicit
    //    zero `vector_dim` to `VectorDimRequired` *before* an `IndexSpec::Vector`
    //    can ever be built — the variant simply has no slot for a zero/absent dim.
    let omitted = create_index("idx", "docs")
        .field("embedding")
        .index_type("vector");
    assert!(matches!(
        IndexSpec::try_from(&omitted),
        Err(CreateIndexBuildError::VectorDimRequired)
    ));

    let zero = create_index("idx", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(0);
    assert!(matches!(
        IndexSpec::try_from(&zero),
        Err(CreateIndexBuildError::VectorDimRequired)
    ));
}

// ============================================================================
// Invalid combinations -> the correct CreateIndexBuildError variant
// (testing TryFrom<&CreateIndex> directly, not through try_build())
// ============================================================================

#[test]
fn try_from_rejects_empty_fields() {
    let b = create_index("idx", "users");
    assert!(matches!(
        IndexSpec::try_from(&b),
        Err(CreateIndexBuildError::EmptyFields)
    ));
}

#[test]
fn try_from_rejects_unique_unsupported_for_type() {
    let b = create_index("idx", "users")
        .field("embedding")
        .unique()
        .index_type("vector")
        .vector_dim(128);
    match IndexSpec::try_from(&b) {
        Err(CreateIndexBuildError::UniqueUnsupportedForType { index_type }) => {
            assert_eq!(index_type, "vector");
        }
        other => panic!("expected UniqueUnsupportedForType, got {other:?}"),
    }
}

#[test]
fn try_from_rejects_sorted_unsupported_for_type() {
    let b = create_index("idx", "posts")
        .field("body")
        .sorted()
        .index_type("fts");
    match IndexSpec::try_from(&b) {
        Err(CreateIndexBuildError::SortedUnsupportedForType { index_type }) => {
            assert_eq!(index_type, "fts");
        }
        other => panic!("expected SortedUnsupportedForType, got {other:?}"),
    }
}

#[test]
fn try_from_rejects_unknown_vector_metric() {
    let b = create_index("idx", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(128)
        .vector_metric("consine"); // typo
    match IndexSpec::try_from(&b) {
        Err(CreateIndexBuildError::UnknownVectorMetric { metric }) => {
            assert_eq!(metric, "consine");
        }
        other => panic!("expected UnknownVectorMetric, got {other:?}"),
    }
}

#[test]
fn try_from_rejects_vector_options_on_non_vector() {
    let b = create_index("idx", "users").field("email").vector_dim(128);
    assert!(matches!(
        IndexSpec::try_from(&b),
        Err(CreateIndexBuildError::VectorOptionsOnNonVectorIndex)
    ));
}

#[test]
fn try_from_rejects_fts_options_on_non_fts() {
    let b = create_index("idx", "users")
        .field("email")
        .fts_tokenizer("whitespace");
    assert!(matches!(
        IndexSpec::try_from(&b),
        Err(CreateIndexBuildError::FtsOptionsOnNonFtsIndex)
    ));
}

#[test]
fn try_from_rejects_functional_options_on_non_functional() {
    let b = create_index("idx", "users")
        .field("email")
        .functional_op("lower");
    assert!(matches!(
        IndexSpec::try_from(&b),
        Err(CreateIndexBuildError::FunctionalOptionsOnNonFunctionalIndex)
    ));
}

#[test]
fn try_from_rejects_include_unsupported_for_type() {
    let b = create_index("idx", "posts")
        .field("body")
        .index_type("fts")
        .include([vec!["title".to_string()]]);
    match IndexSpec::try_from(&b) {
        Err(CreateIndexBuildError::IncludeUnsupportedForType { index_type }) => {
            assert_eq!(index_type, "fts");
        }
        other => panic!("expected IncludeUnsupportedForType, got {other:?}"),
    }
}

#[test]
fn try_from_rejects_unique_and_sorted() {
    let b = create_index("idx", "users")
        .field("email")
        .unique()
        .sorted();
    assert!(matches!(
        IndexSpec::try_from(&b),
        Err(CreateIndexBuildError::UniqueAndSorted)
    ));
}

#[test]
fn try_from_rejects_include_without_sorted() {
    let b = create_index("idx", "users")
        .field("email")
        .include([vec!["name".to_string()]]);
    assert!(matches!(
        IndexSpec::try_from(&b),
        Err(CreateIndexBuildError::IncludeWithoutSorted)
    ));
}

#[test]
fn try_from_rejects_sorted_multi_field() {
    let b = create_index("idx", "users")
        .fields([vec!["a".to_string()], vec!["b".to_string()]])
        .sorted();
    match IndexSpec::try_from(&b) {
        Err(CreateIndexBuildError::SortedMultiField { field_count }) => {
            assert_eq!(field_count, 2);
        }
        other => panic!("expected SortedMultiField, got {other:?}"),
    }
}

// ============================================================================
// Parity: try_build() and build() produce byte-identical wire for valid inputs
// ============================================================================

/// For every input `try_build()` accepts, the produced `BatchOp` must be
/// byte-identical to the (unchanged, infallible) `build()` — `IndexSpec` is a
/// pure internal refactor with zero wire drift. This is the direct
/// byte-identity proof (mirrors the matrix `wire_hex` check but driven through
/// `build()` rather than a fixture hex literal).
#[test]
fn try_build_is_byte_identical_to_build_for_valid_inputs() {
    let builders: Vec<Box<dyn Fn() -> CreateIndex>> = vec![
        Box::new(|| create_index("idx", "users").field("email")),
        Box::new(|| create_index("idx", "users").field("email").unique()),
        Box::new(|| {
            create_index("idx", "users")
                .field("age")
                .sorted()
                .include([vec!["email".to_string()]])
        }),
        Box::new(|| {
            create_index("idx", "posts")
                .field("body")
                .index_type("fts")
                .fts_tokenizer("whitespace")
        }),
        Box::new(|| {
            create_index("idx", "posts")
                .field("body")
                .index_type("fts")
                .fts_tokenizer("unicode")
                .fts_language("en")
        }),
        Box::new(|| {
            create_index("idx", "users")
                .field("email")
                .index_type("functional")
                .functional_op("lower")
        }),
        Box::new(|| {
            create_index("idx", "docs")
                .field("embedding")
                .index_type("vector")
                .vector_dim(384)
                .vector_metric("cosine")
        }),
        Box::new(|| {
            create_index("idx", "docs")
                .field("embedding")
                .index_type("vector")
                .vector_dim(256)
                .vector_metric("cosine")
                .vector_quantization("sq8")
        }),
        Box::new(|| create_index("idx", "users").field("email").if_not_exists()),
    ];

    for mk in &builders {
        let build_op = mk().build();
        let try_op = mk().try_build().expect("expected try_build() to accept");
        assert_eq!(
            batch_op_to_hex(&build_op),
            batch_op_to_hex(&try_op),
            "wire drift: try_build() differs from build()"
        );
    }
}

/// A builder explicitly setting `.index_type("btree")` (a no-op default that no
/// production call site uses today, but which the pre-refactor `try_build()`
/// accepted) must still round-trip byte-identically: `into_op` preserves the
/// btree-family `index_type` verbatim rather than normalizing it to `None`.
#[test]
fn explicit_btree_index_type_is_preserved_through_try_build() {
    let build_op = unwrap_create_index_op(
        create_index("idx", "users")
            .field("email")
            .index_type("btree")
            .build(),
    );
    assert_eq!(build_op.index_type.as_deref(), Some("btree"));

    let try_op = unwrap_create_index_op(
        create_index("idx", "users")
            .field("email")
            .index_type("btree")
            .try_build()
            .expect("btree is a valid btree-family index_type"),
    );
    assert_eq!(try_op.index_type.as_deref(), Some("btree"));
    assert_eq!(try_op, build_op, "try_build diverged from build for btree");
}

/// Sanity: an accepted vector index without an explicit `vector_metric` still
/// carries `vector_metric = None` on the wire (server defaults to cosine) —
/// `IndexSpec::Vector.metric` is `Option<String>` precisely to preserve this.
#[test]
fn vector_index_without_metric_preserves_none_on_wire() {
    let op = unwrap_create_index_op(
        create_index("idx", "docs")
            .field("embedding")
            .index_type("vector")
            .vector_dim(128)
            .try_build()
            .expect("vector index without metric is accepted"),
    );
    assert_eq!(op.index_type.as_deref(), Some("vector"));
    assert_eq!(op.vector_dim, Some(128));
    assert!(op.vector_metric.is_none());
}
