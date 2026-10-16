//! Serde round-trip + BatchOp dispatch tests for the T3 DDL retention
//! wiring: `CreateTableOp.retention` (optional) and `SetRetentionOp`.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::{CreateTableOp, Retention, SetRetentionOp};
use crate::batch::BatchOp;

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

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
        schema: None,
    };
    let j = to_qv(&op);
    assert!(
        j.get("retention").is_none(),
        "expected 'retention' absent, got: {j:?}"
    );
    assert_eq!(
        j.get("create_table").and_then(QueryValue::as_str),
        Some("users")
    );
    assert_eq!(j.get("repo").and_then(QueryValue::as_str), Some("main"));
}

/// Old payload (pre-T3, no `retention` key) deserializes to `retention: None`.
#[test]
fn create_table_op_backward_compat_old_payload() {
    let old = mpack!({
        "create_table": "orders",
        "repo": "main"
    });
    let op: CreateTableOp = from_qv(old);
    assert_eq!(op.create_table, "orders");
    assert_eq!(op.repo, "main");
    assert_eq!(op.retention, None);
}

/// `CreateTableOp` WITH `retention` round-trips through msgpack.
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
        schema: None,
    };
    let j = to_qv(&op);
    assert_eq!(
        j.get("retention"),
        Some(&mpack!({ "max_count": 5_i64 })),
        "retention sub-object must match"
    );

    let back: CreateTableOp = from_qv(j);
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
        schema: None,
    };
    let j = to_qv(&op);
    let back: CreateTableOp = from_qv(j);
    assert_eq!(back, op);
}

// ---------------------------------------------------------------------------
// SetRetentionOp — serde round-trip
// ---------------------------------------------------------------------------

/// `SetRetentionOp` round-trips through msgpack with the expected wire shape.
#[test]
fn set_retention_op_round_trip() {
    let op = SetRetentionOp {
        set_retention: "users".to_string(),
        repo: "main".to_string(),
        retention: Retention {
            max_count: Some(5),
            ..Default::default()
        },
        hmac: None,
    };
    let j = to_qv(&op);
    assert_eq!(
        j,
        mpack!({
            "set_retention": "users",
            "repo": "main",
            "retention": { "max_count": 5_i64 }
        })
    );

    let back: SetRetentionOp = from_qv(j);
    assert_eq!(back, op);
}

/// `SetRetentionOp` defaults `repo` to `"main"` when absent on the wire.
#[test]
fn set_retention_op_repo_defaults_to_main() {
    let j = mpack!({
        "set_retention": "events",
        "retention": { "max_age_secs": 3600_i64 }
    });
    let op: SetRetentionOp = from_qv(j);
    assert_eq!(op.set_retention, "events");
    assert_eq!(op.repo, "main");
    assert_eq!(op.retention.max_age_secs, Some(3600));
}

// ---------------------------------------------------------------------------
// BatchOp dispatch — `set_retention` discriminator key
// ---------------------------------------------------------------------------

/// A payload with a `set_retention` key deserializes to `BatchOp::SetRetention`.
#[test]
fn batch_op_dispatch_set_retention() {
    let j = mpack!({
        "set_retention": "users",
        "repo": "main",
        "retention": { "max_count": 5_i64 }
    });
    let op: BatchOp = from_qv(j);
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
    let back_qv = to_qv(&op);
    let op2: BatchOp = from_qv(back_qv);
    assert_eq!(op, op2);
}

/// `CreateTable` payload without `retention` still dispatches to
/// `BatchOp::CreateTable` with `retention: None` (backward-compat at
/// the BatchOp level).
#[test]
fn batch_op_dispatch_create_table_without_retention() {
    let j = mpack!({
        "create_table": "products",
        "repo": "main"
    });
    let op: BatchOp = from_qv(j);
    match &op {
        BatchOp::CreateTable(ct) => {
            assert_eq!(ct.create_table, "products");
            assert_eq!(ct.retention, None);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}
