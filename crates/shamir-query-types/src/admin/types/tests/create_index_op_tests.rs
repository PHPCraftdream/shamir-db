//! Serde round-trip tests for `CreateIndexOp` covering-index `include` field.

use serde_json::json;

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
