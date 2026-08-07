//! Tests for typed CreateIndex constructors.
//!
//! Proves that typed constructors produce byte-identical output to the
//! equivalent stringly `.build()` calls.

use shamir_query_builder::ddl::{create_index, Metric, Quantization, Tokenizer};
use shamir_query_types::batch::BatchOp;
use std::num::NonZeroU32;

/// Prove that typed constructors produce the same wire bytes as stringly calls.
#[test]
fn typed_constructors_produce_byte_identical_output() {
    // .hash() vs .fields(...).build()
    let stringly_op = create_index("idx_regular", "users")
        .fields(vec![vec!["email".to_string()]])
        .build();
    let typed_op = create_index("idx_regular", "users").hash(vec![vec!["email".to_string()]]);
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .unique() vs .fields(...).unique().build()
    let stringly_op = create_index("idx_unique", "users")
        .fields(vec![vec!["email".to_string()]])
        .unique()
        .build();
    let typed_op =
        create_index("idx_unique", "users").unique_index(vec![vec!["email".to_string()]]);
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .sorted() vs .field(...).sorted().build()
    let stringly_op = create_index("idx_age", "users")
        .field("age")
        .sorted()
        .build();
    let typed_op = create_index("idx_age", "users").sorted_index(vec!["age".to_string()]);
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .sorted_with_include() vs .field(...).sorted().include(...).build()
    let stringly_op = create_index("idx_sorted_inc", "users")
        .field("age")
        .sorted()
        .include(vec![vec!["email".to_string()]])
        .build();
    let typed_op = create_index("idx_sorted_inc", "users")
        .sorted_with_include(vec!["age".to_string()], vec![vec!["email".to_string()]]);
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .fts() vs .field(...).index_type("fts").fts_tokenizer(...).build()
    let stringly_op = create_index("idx_fts", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("whitespace")
        .build();
    let typed_op =
        create_index("idx_fts", "posts").fts(vec!["body".to_string()], Tokenizer::Whitespace);
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .fts_with_language() vs .field(...).index_type("fts").fts_tokenizer(...).fts_language(...).build()
    let stringly_op = create_index("idx_fts_lang", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("unicode")
        .fts_language("en")
        .build();
    let typed_op = create_index("idx_fts_lang", "posts").fts_with_language(
        vec!["body".to_string()],
        Tokenizer::Unicode,
        Some("en".to_string()),
    );
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .functional() vs .field(...).index_type("functional").functional_op(...).build()
    let stringly_op = create_index("idx_func", "users")
        .field("email")
        .index_type("functional")
        .functional_op("lower")
        .build();
    let typed_op = create_index("idx_func", "users").functional(vec!["email".to_string()], "lower");
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));

    // .vector() with Off vs .field(...).index_type("vector").vector_dim(...).vector_metric(...).build()
    let dim = NonZeroU32::new(384).unwrap();
    let stringly_op = create_index("idx_vector", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(384)
        .vector_metric("cosine")
        .build();
    let typed_op = create_index("idx_vector", "docs").vector(
        vec!["embedding".to_string()],
        dim,
        Metric::Cosine,
        Quantization::Off,
    );
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
    let typed_op = create_index("idx_vec_sq8", "docs").vector(
        vec!["embedding".to_string()],
        dim,
        Metric::Cosine,
        Quantization::Sq8,
    );
    assert_eq!(msgpack_hex(&stringly_op), msgpack_hex(&typed_op));
}

/// Helper: serialize a `BatchOp` to msgpack and return the hex string.
fn msgpack_hex(op: &BatchOp) -> String {
    let bytes = rmp_serde::to_vec_named(op).expect("msgpack encode");
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
