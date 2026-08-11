//! Tests for typed CreateIndex constructors.
//!
//! Three test groups:
//!
//! 1. **Byte-identity** (`typed_constructors_produce_byte_identical_output`):
//!    proves the typed constructors produce the same wire bytes as the
//!    equivalent stringly `.build()` calls on the happy path.
//! 2. **Builder-mixing conflict** (`typed_constructors_reject_stale_state`):
//!    proves the typed constructors reject prior stringly setter calls that
//!    would be silently discarded (F-7, #1075).
//! 3. **Empty fields** (`typed_constructors_reject_empty_fields`):
//!    proves the typed constructors reject empty field parameters (F-8,
//!    #1075).

use shamir_query_builder::ddl::{
    create_index, CreateIndexBuildError, Metric, Quantization, Tokenizer,
};
use shamir_query_types::batch::BatchOp;
use std::num::NonZeroU32;

// ============================================================================
// Group 1: byte-identity (happy path)
// ============================================================================

/// Prove that typed constructors produce the same wire bytes as stringly calls.
#[test]
fn typed_constructors_produce_byte_identical_output() {
    // .hash() vs .fields(...).build()
    let stringly_op = create_index("idx_regular", "users")
        .fields(vec![vec!["email".to_string()]])
        .build();
    let typed_op = create_index("idx_regular", "users")
        .hash(vec![vec!["email".to_string()]])
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .unique_index() vs .fields(...).unique().build()
    let stringly_op = create_index("idx_unique", "users")
        .fields(vec![vec!["email".to_string()]])
        .unique()
        .build();
    let typed_op = create_index("idx_unique", "users")
        .unique_index(vec![vec!["email".to_string()]])
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .sorted_index() vs .field(...).sorted().build()
    let stringly_op = create_index("idx_age", "users")
        .field("age")
        .sorted()
        .build();
    let typed_op = create_index("idx_age", "users")
        .sorted_index(vec!["age".to_string()])
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .sorted_with_include() vs .field(...).sorted().include(...).build()
    let stringly_op = create_index("idx_sorted_inc", "users")
        .field("age")
        .sorted()
        .include(vec![vec!["email".to_string()]])
        .build();
    let typed_op = create_index("idx_sorted_inc", "users")
        .sorted_with_include(vec!["age".to_string()], vec![vec!["email".to_string()]])
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .fts() vs .field(...).index_type("fts").fts_tokenizer(...).build()
    let stringly_op = create_index("idx_fts", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("whitespace")
        .build();
    let typed_op = create_index("idx_fts", "posts")
        .fts(vec!["body".to_string()], Tokenizer::Whitespace)
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .fts_with_language() vs .field(...).index_type("fts").fts_tokenizer(...).fts_language(...).build()
    let stringly_op = create_index("idx_fts_lang", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("unicode")
        .fts_language("en")
        .build();
    let typed_op = create_index("idx_fts_lang", "posts")
        .fts_with_language(
            vec!["body".to_string()],
            Tokenizer::Unicode,
            Some("en".to_string()),
        )
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .functional() vs .field(...).index_type("functional").functional_op(...).build()
    let stringly_op = create_index("idx_func", "users")
        .field("email")
        .index_type("functional")
        .functional_op("lower")
        .build();
    let typed_op = create_index("idx_func", "users")
        .functional(vec!["email".to_string()], "lower")
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .vector() with Off vs .field(...).index_type("vector").vector_dim(...).vector_metric(...).build()
    let dim = NonZeroU32::new(384).unwrap();
    let stringly_op = create_index("idx_vector", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(384)
        .vector_metric("cosine")
        .build();
    let typed_op = create_index("idx_vector", "docs")
        .vector(
            vec!["embedding".to_string()],
            dim,
            Metric::Cosine,
            Quantization::Off,
        )
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .vector() with Sq8 vs .field(...).index_type("vector").vector_dim(...).vector_metric(...).vector_quantization(...).build()
    let dim = NonZeroU32::new(256).unwrap();
    let stringly_op = create_index("idx_vec_sq8", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(256)
        .vector_metric("cosine")
        .vector_quantization("sq8")
        .build();
    let typed_op = create_index("idx_vec_sq8", "docs")
        .vector(
            vec!["embedding".to_string()],
            dim,
            Metric::Cosine,
            Quantization::Sq8,
        )
        .unwrap();
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));
}

// ============================================================================
// Group 2: builder-mixing conflict detection (F-7)
// ============================================================================

/// Prove that `.hash()` rejects stale builder state from prior stringly setters.
#[test]
fn hash_rejects_stale_state() {
    // The data-corrupting scenario from the bug report: .unique().hash(...)
    // silently produced `unique: false` before the fix.
    let err = create_index("idx", "users")
        .unique()
        .hash(vec![vec!["email".to_string()]])
        .expect_err(".unique().hash() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "hash",
                field: "unique"
            }
        ),
        "got {err:?}"
    );

    // .sorted() is also a conflict for .hash().
    let err = create_index("idx", "users")
        .sorted()
        .hash(vec![vec!["email".to_string()]])
        .expect_err(".sorted().hash() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "hash",
                field: "sorted"
            }
        ),
        "got {err:?}"
    );

    // A prior .fields() call is stale — the parameter to .hash() is authoritative.
    let err = create_index("idx", "users")
        .fields(vec![vec!["stale".to_string()]])
        .hash(vec![vec!["email".to_string()]])
        .expect_err(".fields().hash() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "hash",
                field: "fields"
            }
        ),
        "got {err:?}"
    );

    // A prior .vector_dim() call is stale.
    let err = create_index("idx", "users")
        .vector_dim(768)
        .hash(vec![vec!["email".to_string()]])
        .expect_err(".vector_dim().hash() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "hash",
                field: "vector_dim"
            }
        ),
        "got {err:?}"
    );

    // .include() without .sorted() is stale for .hash().
    let err = create_index("idx", "users")
        .include(vec![vec!["x".to_string()]])
        .hash(vec![vec!["email".to_string()]])
        .expect_err(".include().hash() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "hash",
                field: "include"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.unique_index()` rejects stale builder state — but a prior
/// `.unique()` call is redundant (not a conflict) since `bool` has no sentinel.
#[test]
fn unique_index_rejects_stale_state() {
    // .unique() before .unique_index() is REDUNDANT — not a conflict.
    // It must succeed (bool has no "never touched" sentinel).
    let op = create_index("idx", "users")
        .unique()
        .unique_index(vec![vec!["email".to_string()]]);
    assert!(
        op.is_ok(),
        "redundant .unique() before .unique_index() must not reject"
    );

    // .sorted() IS a conflict — unique_index produces sorted: false.
    let err = create_index("idx", "users")
        .sorted()
        .unique_index(vec![vec!["email".to_string()]])
        .expect_err(".sorted().unique_index() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "unique_index",
                field: "sorted"
            }
        ),
        "got {err:?}"
    );

    // .index_type() is a conflict.
    let err = create_index("idx", "users")
        .index_type("btree")
        .unique_index(vec![vec!["email".to_string()]])
        .expect_err(".index_type().unique_index() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "unique_index",
                field: "index_type"
            }
        ),
        "got {err:?}"
    );

    // A prior .fields() call is stale.
    let err = create_index("idx", "users")
        .field("stale")
        .unique_index(vec![vec!["email".to_string()]])
        .expect_err(".field().unique_index() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "unique_index",
                field: "fields"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.sorted_index()` rejects stale state — but a prior `.sorted()`
/// is redundant (not a conflict).
#[test]
fn sorted_index_rejects_stale_state() {
    // .sorted() before .sorted_index() is REDUNDANT — not a conflict.
    let op = create_index("idx", "users")
        .sorted()
        .sorted_index(vec!["age".to_string()]);
    assert!(
        op.is_ok(),
        "redundant .sorted() before .sorted_index() must not reject"
    );

    // .unique() IS a conflict — sorted_index produces unique: false.
    let err = create_index("idx", "users")
        .unique()
        .sorted_index(vec!["age".to_string()])
        .expect_err(".unique().sorted_index() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "sorted_index",
                field: "unique"
            }
        ),
        "got {err:?}"
    );

    // .include() is stale — sorted_index takes no include parameter.
    let err = create_index("idx", "users")
        .include(vec![vec!["email".to_string()]])
        .sorted_index(vec!["age".to_string()])
        .expect_err(".include().sorted_index() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "sorted_index",
                field: "include"
            }
        ),
        "got {err:?}"
    );

    // .index_type() is a conflict.
    let err = create_index("idx", "users")
        .index_type("btree")
        .sorted_index(vec!["age".to_string()])
        .expect_err(".index_type().sorted_index() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "sorted_index",
                field: "index_type"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.sorted_with_include()` rejects stale state — but a prior
/// `.sorted()` is redundant (not a conflict).
#[test]
fn sorted_with_include_rejects_stale_state() {
    // .sorted() before .sorted_with_include() is REDUNDANT — not a conflict.
    let op = create_index("idx", "users")
        .sorted()
        .sorted_with_include(vec!["age".to_string()], vec![vec!["email".to_string()]]);
    assert!(
        op.is_ok(),
        "redundant .sorted() before .sorted_with_include() must not reject"
    );

    // .include() before .sorted_with_include() is stale — the include parameter
    // is authoritative.
    let err = create_index("idx", "users")
        .include(vec![vec!["stale".to_string()]])
        .sorted_with_include(vec!["age".to_string()], vec![vec!["email".to_string()]])
        .expect_err(".include().sorted_with_include() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "sorted_with_include",
                field: "include"
            }
        ),
        "got {err:?}"
    );

    // .unique() is a conflict.
    let err = create_index("idx", "users")
        .unique()
        .sorted_with_include(vec!["age".to_string()], vec![vec!["email".to_string()]])
        .expect_err(".unique().sorted_with_include() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "sorted_with_include",
                field: "unique"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.fts()` rejects stale builder state.
#[test]
fn fts_rejects_stale_state() {
    // .fts_tokenizer() is stale — the tokenizer parameter is authoritative.
    let err = create_index("idx", "posts")
        .fts_tokenizer("unicode")
        .fts(vec!["body".to_string()], Tokenizer::Whitespace)
        .expect_err(".fts_tokenizer().fts() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "fts_with_language",
                field: "fts_tokenizer"
            }
        ),
        "got {err:?}"
    );

    // .fts_language() is stale — .fts() passes language: None.
    let err = create_index("idx", "posts")
        .fts_language("en")
        .fts(vec!["body".to_string()], Tokenizer::Whitespace)
        .expect_err(".fts_language().fts() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "fts_with_language",
                field: "fts_language"
            }
        ),
        "got {err:?}"
    );

    // .unique() is a conflict.
    let err = create_index("idx", "posts")
        .unique()
        .fts(vec!["body".to_string()], Tokenizer::Whitespace)
        .expect_err(".unique().fts() must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "fts_with_language",
                field: "unique"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.fts_with_language()` rejects stale builder state.
#[test]
fn fts_with_language_rejects_stale_state() {
    // .fts_tokenizer() is stale — the tokenizer parameter is authoritative.
    let err = create_index("idx", "posts")
        .fts_tokenizer("whitespace")
        .fts_with_language(
            vec!["body".to_string()],
            Tokenizer::Unicode,
            Some("en".to_string()),
        )
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "fts_with_language",
                field: "fts_tokenizer"
            }
        ),
        "got {err:?}"
    );

    // .vector_dim() is a conflict.
    let err = create_index("idx", "posts")
        .vector_dim(128)
        .fts_with_language(vec!["body".to_string()], Tokenizer::Unicode, None)
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "fts_with_language",
                field: "vector_dim"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.functional()` rejects stale builder state.
#[test]
fn functional_rejects_stale_state() {
    // .functional_op() is stale — the func parameter is authoritative.
    let err = create_index("idx", "users")
        .functional_op("upper")
        .functional(vec!["email".to_string()], "lower")
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "functional_with_args",
                field: "functional_op"
            }
        ),
        "got {err:?}"
    );

    // .functional_args() is stale.
    let err = create_index("idx", "users")
        .functional_args(vec![shamir_types::types::value::QueryValue::from("arg")])
        .functional(vec!["email".to_string()], "lower")
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "functional_with_args",
                field: "functional_args"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.functional_with_args()` rejects stale builder state.
#[test]
fn functional_with_args_rejects_stale_state() {
    // .functional_op() is stale — the func parameter is authoritative.
    let err = create_index("idx", "users")
        .functional_op("upper")
        .functional_with_args(vec!["email".to_string()], "lower", Vec::new())
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "functional_with_args",
                field: "functional_op"
            }
        ),
        "got {err:?}"
    );

    // .sorted() is a conflict.
    let err = create_index("idx", "users")
        .sorted()
        .functional_with_args(vec!["email".to_string()], "lower", Vec::new())
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "functional_with_args",
                field: "sorted"
            }
        ),
        "got {err:?}"
    );
}

/// Prove that `.vector()` rejects stale builder state.
#[test]
fn vector_rejects_stale_state() {
    let dim = NonZeroU32::new(384).unwrap();

    // .vector_dim() is stale — the dim parameter is authoritative.
    let err = create_index("idx", "docs")
        .vector_dim(768)
        .vector(
            vec!["embedding".to_string()],
            dim,
            Metric::Cosine,
            Quantization::Off,
        )
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "vector",
                field: "vector_dim"
            }
        ),
        "got {err:?}"
    );

    // .vector_metric() is stale — the metric parameter is authoritative.
    let err = create_index("idx", "docs")
        .vector_metric("l2")
        .vector(
            vec!["embedding".to_string()],
            dim,
            Metric::Cosine,
            Quantization::Off,
        )
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "vector",
                field: "vector_metric"
            }
        ),
        "got {err:?}"
    );

    // .unique() is a conflict.
    let err = create_index("idx", "docs")
        .unique()
        .vector(
            vec!["embedding".to_string()],
            dim,
            Metric::Cosine,
            Quantization::Off,
        )
        .expect_err("must reject");
    assert!(
        matches!(
            err,
            CreateIndexBuildError::ConflictingBuilderState {
                method: "vector",
                field: "unique"
            }
        ),
        "got {err:?}"
    );
}

// ============================================================================
// Group 3: empty fields (F-8)
// ============================================================================

/// Prove that all typed constructors reject empty field parameters.
#[test]
fn typed_constructors_reject_empty_fields() {
    // .hash() with empty outer vec
    let err = create_index("idx", "users")
        .hash(Vec::<Vec<String>>::new())
        .expect_err("empty .hash() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .unique_index() with empty outer vec
    let err = create_index("idx", "users")
        .unique_index(Vec::<Vec<String>>::new())
        .expect_err("empty .unique_index() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .sorted_index() with empty path
    let err = create_index("idx", "users")
        .sorted_index(Vec::<String>::new())
        .expect_err("empty .sorted_index() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .sorted_with_include() with empty path
    let err = create_index("idx", "users")
        .sorted_with_include(Vec::<String>::new(), vec![vec!["x".to_string()]])
        .expect_err("empty .sorted_with_include() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .fts() with empty path
    let err = create_index("idx", "posts")
        .fts(Vec::<String>::new(), Tokenizer::Whitespace)
        .expect_err("empty .fts() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .fts_with_language() with empty path
    let err = create_index("idx", "posts")
        .fts_with_language(Vec::<String>::new(), Tokenizer::Whitespace, None)
        .expect_err("empty .fts_with_language() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .functional() with empty path
    let err = create_index("idx", "users")
        .functional(Vec::<String>::new(), "lower")
        .expect_err("empty .functional() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .functional_with_args() with empty path
    let err = create_index("idx", "users")
        .functional_with_args(Vec::<String>::new(), "lower", Vec::new())
        .expect_err("empty .functional_with_args() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );

    // .vector() with empty path
    let dim = NonZeroU32::new(384).unwrap();
    let err = create_index("idx", "docs")
        .vector(Vec::<String>::new(), dim, Metric::Cosine, Quantization::Off)
        .expect_err("empty .vector() must reject");
    assert!(
        matches!(err, CreateIndexBuildError::EmptyFields),
        "got {err:?}"
    );
}

// ============================================================================
// Helpers
// ============================================================================

/// Helper: serialize a `BatchOp` to msgpack and return the hex string.
fn msgpack_hex(op: &BatchOp) -> String {
    let bytes = rmp_serde::to_vec_named(op).expect("msgpack encode");
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
