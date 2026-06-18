//! Serde round-trip tests for `CreateIndexOp` covering-index `include` field.

use serde_json::json;
use shamir_types::mpack;

use crate::admin::CreateIndexOp;

/// Confirm that `include` round-trips through JSON and that the
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
        include: vec![vec!["email".to_string()], vec!["name".to_string()]],
        if_not_exists: false,
    };

    let json_val = serde_json::to_value(&op).expect("serialize");
    assert_eq!(json_val["include"], json!([["email"], ["name"]]));

    let back: CreateIndexOp = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, op);
}

/// `functional_args` with actual `QueryValue` data round-trips through
/// msgpack — confirming that `QueryValue` is wire-compatible with the
/// plain-scalar msgpack form that `serde_json::Value` would have produced.
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

/// Without `include`, the field must be absent in the serialized JSON
/// (skip_serializing_if = "Vec::is_empty") and parse correctly from
/// JSON that doesn't have the key at all.
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
        include: Vec::new(),
        if_not_exists: false,
    };

    let json_val = serde_json::to_value(&op).expect("serialize");
    // `include` must NOT appear in the serialized output.
    assert!(
        json_val.get("include").is_none(),
        "expected 'include' to be absent, got: {json_val}"
    );

    // Old JSON without `include` key must parse with include = [].
    let old_json = json!({
        "create_index": "idx_name",
        "table": "users",
        "fields": [["name"]],
        "sorted": true,
        "repo": "main"
    });
    let parsed: CreateIndexOp = serde_json::from_value(old_json).expect("deserialize old json");
    assert!(parsed.include.is_empty());
}
