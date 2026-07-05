//! Serde round-trip tests for `CreateIndexOp` covering-index `include` field.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::CreateIndexOp;

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

/// Confirm that `include` round-trips through msgpack and that the
/// serialized form matches the expected wire shape.
#[test]
fn create_index_op_include_round_trip() {
    let op = CreateIndexOp {
        create_index: "idx_email".to_string(),
        table: "users".to_string(),
        fields: vec![vec!["score".to_string()]],
        unique: false,
        sorted: true,
        repo: "main".to_string(),
        index_type: None,
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: vec![vec!["email".to_string()], vec!["name".to_string()]],
        if_not_exists: false,
    };

    let qv = to_qv(&op);
    assert_eq!(qv.get("include"), Some(&mpack!([["email"], ["name"]])));

    let back: CreateIndexOp = from_qv(qv);
    assert_eq!(back, op);
}

/// `functional_args` with actual `QueryValue` data round-trips through
/// msgpack — confirming that `QueryValue` is wire-compatible with the
/// plain-scalar msgpack form.
///
/// This is the migration correctness gate: an integer arg stays an integer,
/// a string arg stays a string, and the `Option<Vec<QueryValue>>` envelope
/// (present / absent) round-trips correctly in both directions.
#[test]
fn functional_args_msgpack_round_trip() {
    // --- present: integer + string args ---
    let op_with_args = CreateIndexOp {
        create_index: "mod_idx".to_string(),
        table: "items".to_string(),
        fields: vec![vec!["price".to_string()]],
        unique: false,
        sorted: false,
        repo: "main".to_string(),
        index_type: Some("functional".to_string()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("mod".to_string()),
        functional_args: Some(vec![mpack!(10), mpack!("base")]),
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };

    let bytes = rmp_serde::to_vec_named(&op_with_args).expect("msgpack serialize");
    let back: CreateIndexOp = rmp_serde::from_slice(&bytes).expect("msgpack deserialize");
    assert_eq!(
        back, op_with_args,
        "functional_args round-trip mismatch (present)"
    );

    // The integer arg must decode as Int(10), not something else.
    let args = back.functional_args.as_deref().expect("args present");
    assert_eq!(args[0], mpack!(10), "integer arg preserved as Int");
    assert_eq!(args[1], mpack!("base"), "string arg preserved as Str");

    // --- absent: None must survive round-trip and be omitted from bytes ---
    let op_no_args = CreateIndexOp {
        create_index: "lower_idx".to_string(),
        table: "users".to_string(),
        fields: vec![vec!["name".to_string()]],
        unique: false,
        sorted: false,
        repo: "main".to_string(),
        index_type: Some("functional".to_string()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".to_string()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };

    let bytes_no_args = rmp_serde::to_vec_named(&op_no_args).expect("msgpack serialize (no args)");
    let back_no_args: CreateIndexOp =
        rmp_serde::from_slice(&bytes_no_args).expect("msgpack deserialize (no args)");
    assert_eq!(
        back_no_args, op_no_args,
        "functional_args round-trip mismatch (absent)"
    );
    assert!(
        back_no_args.functional_args.is_none(),
        "functional_args absent after round-trip"
    );
}

/// Without `include`, the field must be absent in the serialized output
/// (skip_serializing_if = "Vec::is_empty") and parse correctly from
/// a payload that doesn't have the key at all.
#[test]
fn create_index_op_no_include_omitted() {
    let op = CreateIndexOp {
        create_index: "idx_name".to_string(),
        table: "users".to_string(),
        fields: vec![vec!["name".to_string()]],
        unique: false,
        sorted: true,
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
    };

    let qv = to_qv(&op);
    // `include` must NOT appear in the serialized output.
    assert!(
        qv.get("include").is_none(),
        "expected 'include' to be absent, got: {qv:?}"
    );

    // Old payload without `include` key must parse with include = [].
    let old_qv = mpack!({
        "create_index": "idx_name",
        "table": "users",
        "fields": [["name"]],
        "sorted": true,
        "repo": "main"
    });
    let parsed: CreateIndexOp = from_qv(old_qv);
    assert!(parsed.include.is_empty());
}

// =========================================================================
// V5.2 (#411) — vector_quantization wire back-compat
// =========================================================================

/// Old payload WITHOUT `vector_quantization` (pre-#411 client) must parse
/// to `None` — the legacy unquantized f32 path, bit-for-bit.
#[test]
fn vector_quantization_absent_parses_to_none() {
    let old_qv = mpack!({
        "create_index": "vec_idx",
        "table": "docs",
        "fields": [["embedding"]],
        "index_type": "vector",
        "vector_dim": 128,
        "vector_metric": "cosine",
        "repo": "main"
    });
    let parsed: CreateIndexOp = from_qv(old_qv);
    assert!(
        parsed.vector_quantization.is_none(),
        "pre-#411 payload should parse vector_quantization to None"
    );
}

/// `vector_quantization: "sq8"` round-trips through msgpack and appears in
/// the serialized output.
#[test]
fn vector_quantization_sq8_round_trip() {
    let op = CreateIndexOp {
        create_index: "vec_idx".to_string(),
        table: "docs".to_string(),
        fields: vec![vec!["embedding".to_string()]],
        unique: false,
        sorted: false,
        repo: "main".to_string(),
        index_type: Some("vector".to_string()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(128),
        vector_metric: Some("cosine".to_string()),
        vector_quantization: Some("sq8".to_string()),
        include: Vec::new(),
        if_not_exists: false,
    };

    let qv = to_qv(&op);
    assert_eq!(
        qv.get("vector_quantization"),
        Some(&mpack!("sq8")),
        "vector_quantization must appear in serialized output"
    );

    let back: CreateIndexOp = from_qv(qv);
    assert_eq!(back.vector_quantization.as_deref(), Some("sq8"));
    assert_eq!(back, op);
}

/// `vector_quantization` absent → NOT in serialized output
/// (skip_serializing_if = "Option::is_none").
#[test]
fn vector_quantization_none_omitted_from_output() {
    let op = CreateIndexOp {
        create_index: "vec_idx".to_string(),
        table: "docs".to_string(),
        fields: vec![vec!["embedding".to_string()]],
        unique: false,
        sorted: false,
        repo: "main".to_string(),
        index_type: Some("vector".to_string()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(128),
        vector_metric: Some("cosine".to_string()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };

    let qv = to_qv(&op);
    assert!(
        qv.get("vector_quantization").is_none(),
        "vector_quantization must be absent when None, got: {qv:?}"
    );
}
