//! Serde round-trip + behaviour tests for `Retention`, `PurgeHistoryOp`,
//! and `PurgeScope` (temporal T2 admin DTOs).

use serde_json::json;

use crate::admin::{PurgeHistoryOp, PurgeScope, Retention};

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

/// `Retention` with all knobs set round-trips through JSON.
#[test]
fn retention_all_knobs_round_trip() {
    let r = Retention {
        max_age_secs: Some(86400),
        max_count: Some(1000),
        min_count: Some(10),
    };
    let json_val = serde_json::to_value(&r).expect("serialize");
    assert_eq!(
        json_val,
        json!({
            "max_age_secs": 86400,
            "max_count": 1000,
            "min_count": 10
        })
    );

    let back: Retention = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, r);
}

/// Default `Retention` (all-None) serializes to `{}` — no keys on wire.
#[test]
fn retention_default_serializes_empty() {
    let r = Retention::default();
    let json_val = serde_json::to_value(&r).expect("serialize");
    assert!(json_val.as_object().unwrap().is_empty());
}

/// Partial retention (only `max_age_secs`) round-trips and omits the rest.
#[test]
fn retention_partial_round_trip() {
    let r = Retention {
        max_age_secs: Some(3600),
        ..Default::default()
    };
    let json_val = serde_json::to_value(&r).expect("serialize");
    assert_eq!(json_val, json!({ "max_age_secs": 3600 }));

    let back: Retention = serde_json::from_value(json_val).expect("deserialize");
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
    let json_val = serde_json::to_value(&scope).expect("serialize");
    assert_eq!(
        json_val,
        json!({ "older_than": { "timestamp": 1700000000000u64 } })
    );

    let back: PurgeScope = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, scope);
}

/// `PurgeScope::OlderThanAge` round-trips.
#[test]
fn purge_scope_older_than_age_round_trip() {
    let scope = PurgeScope::OlderThanAge { age_secs: 86400 };
    let json_val = serde_json::to_value(&scope).expect("serialize");
    assert_eq!(json_val, json!({ "older_than_age": { "age_secs": 86400 } }));

    let back: PurgeScope = serde_json::from_value(json_val).expect("deserialize");
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
    let json_val = serde_json::to_value(&op).expect("serialize");
    assert_eq!(
        json_val,
        json!({
            "purge_history": "users",
            "repo": "main",
            "scope": { "older_than": { "timestamp": 1600000000000u64 } }
        })
    );

    let back: PurgeHistoryOp = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(back, op);
}

/// `PurgeHistoryOp` deserializes from JSON that omits `repo` —
/// defaults to `"main"`.
#[test]
fn purge_history_op_repo_defaults_to_main() {
    let json_val = json!({
        "purge_history": "events",
        "scope": { "older_than_age": { "age_secs": 3600 } }
    });
    let op: PurgeHistoryOp = serde_json::from_value(json_val).expect("deserialize");
    assert_eq!(op.purge_history, "events");
    assert_eq!(op.repo, "main");
    assert_eq!(op.scope, PurgeScope::OlderThanAge { age_secs: 3600 });
}
