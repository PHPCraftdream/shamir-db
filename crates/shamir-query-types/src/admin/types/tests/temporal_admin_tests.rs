//! Serde round-trip + behaviour tests for `Retention`, `PurgeHistoryOp`,
//! and `PurgeScope` (temporal T2 admin DTOs).

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::{ChangesSinceOp, PurgeHistoryOp, PurgeScope, Retention};
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
// Retention — serde + helpers
// ---------------------------------------------------------------------------

/// Default `Retention` = all-None = Forever.
#[test]
fn retention_default_is_forever() {
    let r = Retention::default();
    assert_eq!(r.max_age_secs, None);
    assert_eq!(r.max_count, None);
    assert_eq!(r.min_count, None);
    assert!(!r.is_current_only());
}

/// `Retention::current_only()` → `max_count == Some(0)`, rest None.
#[test]
fn retention_current_only() {
    let r = Retention::current_only();
    assert_eq!(r.max_count, Some(0));
    assert_eq!(r.max_age_secs, None);
    assert_eq!(r.min_count, None);
    assert!(r.is_current_only());
}

/// `is_current_only()` is false when `max_age_secs` or `min_count` are set
/// alongside `max_count: Some(0)`.
#[test]
fn retention_current_only_requires_all_three() {
    let r1 = Retention {
        max_count: Some(0),
        max_age_secs: Some(3600),
        ..Default::default()
    };
    assert!(!r1.is_current_only());

    let r2 = Retention {
        max_count: Some(0),
        min_count: Some(1),
        ..Default::default()
    };
    assert!(!r2.is_current_only());
}

/// `Retention` with all knobs set round-trips through msgpack.
#[test]
fn retention_all_knobs_round_trip() {
    let r = Retention {
        max_age_secs: Some(86400),
        max_count: Some(1000),
        min_count: Some(10),
    };
    let qv = to_qv(&r);
    assert_eq!(
        qv,
        mpack!({
            "max_age_secs": 86400_i64,
            "max_count": 1000_i64,
            "min_count": 10_i64
        })
    );

    let back: Retention = from_qv(qv);
    assert_eq!(back, r);
}

/// Default `Retention` (all-None) serializes to an empty map — no keys on wire.
#[test]
fn retention_default_serializes_empty() {
    let r = Retention::default();
    let qv = to_qv(&r);
    match &qv {
        QueryValue::Map(m) => assert!(m.is_empty()),
        other => panic!("expected empty Map, got {other:?}"),
    }
}

/// Partial retention (only `max_age_secs`) round-trips and omits the rest.
#[test]
fn retention_partial_round_trip() {
    let r = Retention {
        max_age_secs: Some(3600),
        ..Default::default()
    };
    let qv = to_qv(&r);
    assert_eq!(qv, mpack!({ "max_age_secs": 3600_i64 }));

    let back: Retention = from_qv(qv);
    assert_eq!(back, r);
}

// ---------------------------------------------------------------------------
// Retention::validate
// ---------------------------------------------------------------------------

/// `validate` rejects `min_count > max_count`.
#[test]
fn retention_validate_rejects_min_gt_max() {
    let r = Retention {
        max_count: Some(5),
        min_count: Some(10),
        ..Default::default()
    };
    assert!(r.validate().is_err());
}

/// `validate` accepts `min_count == max_count`.
#[test]
fn retention_validate_accepts_min_eq_max() {
    let r = Retention {
        max_count: Some(5),
        min_count: Some(5),
        ..Default::default()
    };
    assert!(r.validate().is_ok());
}

/// `validate` accepts `min_count < max_count`.
#[test]
fn retention_validate_accepts_min_lt_max() {
    let r = Retention {
        max_count: Some(100),
        min_count: Some(10),
        max_age_secs: Some(86400),
    };
    assert!(r.validate().is_ok());
}

/// `validate` accepts when only one of the two is set.
#[test]
fn retention_validate_accepts_single_bound() {
    assert!(Retention {
        max_count: Some(100),
        ..Default::default()
    }
    .validate()
    .is_ok());

    assert!(Retention {
        min_count: Some(10),
        ..Default::default()
    }
    .validate()
    .is_ok());

    assert!(Retention::default().validate().is_ok());
}

// ---------------------------------------------------------------------------
// PurgeHistoryOp / PurgeScope
// ---------------------------------------------------------------------------

/// `PurgeScope::OlderThan` round-trips.
#[test]
fn purge_scope_older_than_round_trip() {
    let scope = PurgeScope::OlderThan {
        timestamp: 1_700_000_000_000,
    };
    let qv = to_qv(&scope);
    assert_eq!(
        qv,
        mpack!({ "older_than": { "timestamp": @ QueryValue::Int(1_700_000_000_000_i64) } })
    );

    let back: PurgeScope = from_qv(qv);
    assert_eq!(back, scope);
}

/// `PurgeScope::OlderThanAge` round-trips.
#[test]
fn purge_scope_older_than_age_round_trip() {
    let scope = PurgeScope::OlderThanAge { age_secs: 86400 };
    let qv = to_qv(&scope);
    assert_eq!(qv, mpack!({ "older_than_age": { "age_secs": 86400_i64 } }));

    let back: PurgeScope = from_qv(qv);
    assert_eq!(back, scope);
}

/// `PurgeHistoryOp` with `OlderThan` scope round-trips.
#[test]
fn purge_history_op_older_than_round_trip() {
    let op = PurgeHistoryOp {
        purge_history: "users".to_string(),
        repo: "main".to_string(),
        scope: PurgeScope::OlderThan {
            timestamp: 1_600_000_000_000,
        },
    };
    let qv = to_qv(&op);
    assert_eq!(
        qv,
        mpack!({
            "purge_history": "users",
            "repo": "main",
            "scope": { "older_than": { "timestamp": @ QueryValue::Int(1_600_000_000_000_i64) } }
        })
    );

    let back: PurgeHistoryOp = from_qv(qv);
    assert_eq!(back, op);
}

/// `PurgeHistoryOp` deserializes from a payload that omits `repo` —
/// defaults to `"main"`.
#[test]
fn purge_history_op_repo_defaults_to_main() {
    let qv = mpack!({
        "purge_history": "events",
        "scope": { "older_than_age": { "age_secs": 3600_i64 } }
    });
    let op: PurgeHistoryOp = from_qv(qv);
    assert_eq!(op.purge_history, "events");
    assert_eq!(op.repo, "main");
    assert_eq!(op.scope, PurgeScope::OlderThanAge { age_secs: 3600 });
}

// ---------------------------------------------------------------------------
// ChangesSinceOp — serde + BatchOp dispatch (T4-changes-since)
// ---------------------------------------------------------------------------

/// `ChangesSinceOp` round-trips with an explicit repo and limit.
#[test]
fn changes_since_op_round_trip() {
    let op = ChangesSinceOp {
        changes_since: 42,
        repo: "main".to_string(),
        limit: Some(500),
    };
    let qv = to_qv(&op);
    assert_eq!(
        qv,
        mpack!({
            "changes_since": 42_i64,
            "repo": "main",
            "limit": 500_i64
        })
    );

    let back: ChangesSinceOp = from_qv(qv);
    assert_eq!(back, op);
}

/// `ChangesSinceOp` omits `limit` when `None` and defaults `repo` to `"main"`.
#[test]
fn changes_since_op_defaults_repo_and_omits_limit() {
    let op = ChangesSinceOp {
        changes_since: 7,
        repo: "main".to_string(),
        limit: None,
    };
    let qv = to_qv(&op);
    // `limit` is skipped; `repo` is present (default_fn only affects decode).
    assert_eq!(
        qv.get("changes_since").and_then(QueryValue::as_i64),
        Some(7)
    );
    assert_eq!(qv.get("repo").and_then(QueryValue::as_str), Some("main"));
    assert!(qv.get("limit").is_none(), "limit must be omitted");

    // Payload without `repo` deserializes with repo == "main".
    let minimal = mpack!({ "changes_since": 7_i64 });
    let back: ChangesSinceOp = from_qv(minimal);
    assert_eq!(back.repo, "main");
    assert_eq!(back.limit, None);
    assert_eq!(back.changes_since, 7);
}

/// A payload with a `changes_since` key deserializes to `BatchOp::ChangesSince`.
#[test]
fn batch_op_dispatch_changes_since() {
    let j = mpack!({
        "changes_since": 10_i64,
        "repo": "main",
        "limit": 100_i64
    });
    let op: BatchOp = from_qv(j);
    match &op {
        BatchOp::ChangesSince(cs) => {
            assert_eq!(cs.changes_since, 10);
            assert_eq!(cs.repo, "main");
            assert_eq!(cs.limit, Some(100));
        }
        other => panic!("expected ChangesSince, got {other:?}"),
    }
    assert!(op.is_admin(), "ChangesSince is an admin op");
    assert!(op.table_ref().is_none(), "ChangesSince has no table_ref");

    // Round-trip back through serialize.
    let back_qv = to_qv(&op);
    let op2: BatchOp = from_qv(back_qv);
    assert_eq!(op, op2);
}

/// The new `changes_since` discriminator does not disturb existing BatchOp
/// parsing — an `insert_into` payload still parses to `BatchOp::Insert`, and
/// a `purge_history` payload still parses to `BatchOp::PurgeHistory`.
#[test]
fn changes_since_does_not_break_existing_batch_op_parsing() {
    let ins = mpack!({ "insert_into": "users", "values": [] });
    let ins_op: BatchOp = from_qv(ins);
    assert!(matches!(ins_op, BatchOp::Insert(_)));

    let ph = mpack!({
        "purge_history": "events",
        "scope": { "older_than_age": { "age_secs": 60_i64 } }
    });
    let ph_op: BatchOp = from_qv(ph);
    assert!(matches!(ph_op, BatchOp::PurgeHistory(_)));
}
