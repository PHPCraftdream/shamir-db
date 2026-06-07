//! Serde round-trip + BatchOp dispatch tests for the T3 DDL retention
//! wiring: `CreateTableOp.retention` (optional) and `SetRetentionOp`.

use serde_json::json;

use crate::admin::{CreateTableOp, Retention, SetRetentionOp};
use crate::batch::BatchOp;

// ---------------------------------------------------------------------------
// CreateTableOp — optional `retention` field (backward-compat)
// ---------------------------------------------------------------------------

/// `CreateTableOp` without `retention` serializes WITHOUT the key and
/// deserializes to `None` — byte-identical to the pre-T3 wire shape.
#[test]
fn create_table_op_without_retention_omits_key() {
    let op = CreateTableOp {
        create_table: "users".to_string(),
        repo: "main".to_string(),
        if_not_exists: false,
        retention: None,
    };
    let j = serde_json::to_value(&op).expect("serialize");
    assert!(
        j.get("retention").is_none(),
        "expected 'retention' absent, got: {j}"
    );
    assert_eq!(j["create_table"], json!("users"));
    assert_eq!(j["repo"], json!("main"));
}

/// Old JSON (pre-T3, no `retention` key) deserializes to `retention: None`.
#[test]
fn create_table_op_backward_compat_old_json() {
    let old = json!({
        "create_table": "orders",
        "repo": "main"
    });
    let op: CreateTableOp = serde_json::from_value(old).expect("deserialize old json");
    assert_eq!(op.create_table, "orders");
    assert_eq!(op.repo, "main");
    assert_eq!(op.retention, None);
}

/// `CreateTableOp` WITH `retention` round-trips through JSON.
#[test]
fn create_table_op_with_retention_round_trip() {
    let op = CreateTableOp {
        create_table: "events".to_string(),
        repo: "main".to_string(),
        if_not_exists: false,
        retention: Some(Retention {
            max_count: Some(5),
            ..Default::default()
        }),
    };
    let j = serde_json::to_value(&op).expect("serialize");
    assert_eq!(
        j["retention"],
        json!({ "max_count": 5 }),
        "retention sub-object must match"
    );

    let back: CreateTableOp = serde_json::from_value(j).expect("deserialize");
    assert_eq!(back, op);
}

/// `CreateTableOp` with all retention knobs set round-trips.
#[test]
fn create_table_op_full_retention_round_trip() {
    let op = CreateTableOp {
        create_table: "audit".to_string(),
        repo: "logs".to_string(),
        if_not_exists: true,
        retention: Some(Retention {
            max_age_secs: Some(86400),
            max_count: Some(1000),
            min_count: Some(10),
        }),
    };
    let j = serde_json::to_value(&op).expect("serialize");
    let back: CreateTableOp = serde_json::from_value(j).expect("deserialize");
    assert_eq!(back, op);
}

// ---------------------------------------------------------------------------
// SetRetentionOp — serde round-trip
// ---------------------------------------------------------------------------

/// `SetRetentionOp` round-trips through JSON with the expected wire shape.
#[test]
fn set_retention_op_round_trip() {
    let op = SetRetentionOp {
        set_retention: "users".to_string(),
        repo: "main".to_string(),
        retention: Retention {
            max_count: Some(5),
            ..Default::default()
        },
    };
    let j = serde_json::to_value(&op).expect("serialize");
    assert_eq!(
        j,
        json!({
            "set_retention": "users",
            "repo": "main",
            "retention": { "max_count": 5 }
        })
    );

    let back: SetRetentionOp = serde_json::from_value(j).expect("deserialize");
    assert_eq!(back, op);
}

/// `SetRetentionOp` defaults `repo` to `"main"` when absent on the wire.
#[test]
fn set_retention_op_repo_defaults_to_main() {
    let j = json!({
        "set_retention": "events",
        "retention": { "max_age_secs": 3600 }
    });
    let op: SetRetentionOp = serde_json::from_value(j).expect("deserialize");
    assert_eq!(op.set_retention, "events");
    assert_eq!(op.repo, "main");
    assert_eq!(op.retention.max_age_secs, Some(3600));
}

// ---------------------------------------------------------------------------
// BatchOp dispatch — `set_retention` discriminator key
// ---------------------------------------------------------------------------

/// JSON with a `set_retention` key deserializes to `BatchOp::SetRetention`.
#[test]
fn batch_op_dispatch_set_retention() {
    let j = json!({
        "set_retention": "users",
        "repo": "main",
        "retention": { "max_count": 5 }
    });
    let op: BatchOp = serde_json::from_value(j).expect("deserialize BatchOp");
    match &op {
        BatchOp::SetRetention(sr) => {
            assert_eq!(sr.set_retention, "users");
            assert_eq!(sr.retention.max_count, Some(5));
        }
        other => panic!("expected SetRetention, got {other:?}"),
    }
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());

    // Round-trip back through serialize.
    let back = serde_json::to_value(&op).expect("serialize");
    let op2: BatchOp = serde_json::from_value(back).expect("deserialize");
    assert_eq!(op, op2);
}

/// `CreateTable` JSON without `retention` still dispatches to
/// `BatchOp::CreateTable` with `retention: None` (backward-compat at
/// the BatchOp level).
#[test]
fn batch_op_dispatch_create_table_without_retention() {
    let j = json!({
        "create_table": "products",
        "repo": "main"
    });
    let op: BatchOp = serde_json::from_value(j).expect("deserialize BatchOp");
    match &op {
        BatchOp::CreateTable(ct) => {
            assert_eq!(ct.create_table, "products");
            assert_eq!(ct.retention, None);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}
