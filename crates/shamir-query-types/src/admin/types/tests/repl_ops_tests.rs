//! Serde round-trip tests for the replication-DDL DTOs
//! (`crates/shamir-query-types/src/admin/types/repl_ops.rs`).
//!
//! These tests exercise the DTOs directly (not through [`BatchOp`]
//! dispatch — that is covered by `batch/tests/batch_types_tests.rs`).
//! They pin the wire shape of each op struct and the nested enums
//! (`ReplDirection`, `ReplMode`, `SubAction`, `ReplScope`, `ReplStream`)
//! so a future rename is a conscious decision, not a silent break.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::{
    AlterSubscriptionOp, CreatePublicationOp, CreateReplicationProfileOp, CreateSubscriptionOp,
    DropPublicationOp, DropReplicationProfileOp, DropSubscriptionOp, ListPublicationsOp,
    ListSubscriptionsOp, ReplDirection, ReplMode, ReplScope, ReplStream, ReplicationStatusOp,
    SubAction,
};

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// ReplScope — optional repo/table fields
// ---------------------------------------------------------------------------

#[test]
fn repl_scope_db_only_omits_repo_and_table() {
    let scope = ReplScope {
        db: "app".to_string(),
        repo: None,
        table: None,
    };
    let qv = to_qv(&scope);
    assert_eq!(qv.get("db").and_then(QueryValue::as_str), Some("app"));
    assert!(qv.get("repo").is_none(), "repo must be omitted: {qv:?}");
    assert!(qv.get("table").is_none(), "table must be omitted: {qv:?}");
    let back: ReplScope = from_qv(qv);
    assert_eq!(back, scope);
}

#[test]
fn repl_scope_full_roundtrips() {
    let scope = ReplScope {
        db: "app".to_string(),
        repo: Some("main".to_string()),
        table: Some("users".to_string()),
    };
    let back: ReplScope = from_qv(to_qv(&scope));
    assert_eq!(back, scope);
}

// ---------------------------------------------------------------------------
// ReplDirection / ReplMode — snake_case enum wire shape + defaults
// ---------------------------------------------------------------------------

#[test]
fn repl_direction_snake_case_roundtrip() {
    for (v, expect) in [
        (ReplDirection::Pull, "pull"),
        (ReplDirection::Push, "push"),
        (ReplDirection::Both, "both"),
    ] {
        let qv = to_qv(&v);
        assert_eq!(qv.as_str(), Some(expect));
        let back: ReplDirection = from_qv(qv);
        assert_eq!(back, v);
    }
}

#[test]
fn repl_mode_snake_case_roundtrip() {
    for (v, expect) in [
        (ReplMode::ReadOnly, "read_only"),
        (ReplMode::ReadWrite, "read_write"),
    ] {
        let qv = to_qv(&v);
        assert_eq!(qv.as_str(), Some(expect));
        let back: ReplMode = from_qv(qv);
        assert_eq!(back, v);
    }
}

// ---------------------------------------------------------------------------
// SubAction — pause / resume / set_profile payload
// ---------------------------------------------------------------------------

#[test]
fn sub_action_pause_resume_roundtrip() {
    for (v, expect) in [(SubAction::Pause, "pause"), (SubAction::Resume, "resume")] {
        let qv = to_qv(&v);
        assert_eq!(qv.as_str(), Some(expect));
        let back: SubAction = from_qv(qv);
        assert_eq!(back, v);
    }
}

#[test]
fn sub_action_set_profile_roundtrip() {
    let v = SubAction::SetProfile("edge".to_string());
    let qv = to_qv(&v);
    assert_eq!(
        qv.get("set_profile").and_then(QueryValue::as_str),
        Some("edge")
    );
    let back: SubAction = from_qv(qv);
    assert_eq!(back, v);
}

// ---------------------------------------------------------------------------
// CreateReplicationProfileOp — streams vector + per-stream defaults
// ---------------------------------------------------------------------------

#[test]
fn create_replication_profile_op_roundtrip() {
    let op = CreateReplicationProfileOp {
        create_replication_profile: "cluster".to_string(),
        streams: vec![
            ReplStream {
                scope: ReplScope {
                    db: "app".to_string(),
                    repo: Some("main".to_string()),
                    table: Some("users".to_string()),
                },
                direction: ReplDirection::Pull,
                mode: ReplMode::ReadOnly,
            },
            // Defaults: no direction/mode → Pull / ReadOnly.
            ReplStream {
                scope: ReplScope {
                    db: "system".to_string(),
                    repo: None,
                    table: None,
                },
                direction: ReplDirection::default(),
                mode: ReplMode::default(),
            },
        ],
    };
    let back: CreateReplicationProfileOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
    assert_eq!(back.streams.len(), 2);
    assert_eq!(back.streams[1].direction, ReplDirection::Pull);
    assert_eq!(back.streams[1].mode, ReplMode::ReadOnly);
}

#[test]
fn create_replication_profile_op_from_msgpack() {
    let qv = mpack!({
        "create_replication_profile": "cluster",
        "streams": [
            {"scope": {"db": "app"}, "direction": "push", "mode": "read_write"}
        ]
    });
    let op: CreateReplicationProfileOp = from_qv(qv);
    assert_eq!(op.create_replication_profile, "cluster");
    assert_eq!(op.streams[0].scope.db, "app");
    assert_eq!(op.streams[0].direction, ReplDirection::Push);
    assert_eq!(op.streams[0].mode, ReplMode::ReadWrite);
}

// ---------------------------------------------------------------------------
// DropReplicationProfileOp
// ---------------------------------------------------------------------------

#[test]
fn drop_replication_profile_op_roundtrip() {
    let op = DropReplicationProfileOp {
        drop_replication_profile: "cluster".to_string(),
    };
    let back: DropReplicationProfileOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

// ---------------------------------------------------------------------------
// CreatePublicationOp / DropPublicationOp
// ---------------------------------------------------------------------------

#[test]
fn create_publication_op_roundtrip() {
    let op = CreatePublicationOp {
        create_publication: "pub_app".to_string(),
        scopes: vec![
            ReplScope {
                db: "app".to_string(),
                repo: Some("main".to_string()),
                table: None,
            },
            ReplScope {
                db: "system".to_string(),
                repo: Some("system".to_string()),
                table: Some("users".to_string()),
            },
        ],
    };
    let back: CreatePublicationOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

#[test]
fn drop_publication_op_roundtrip() {
    let op = DropPublicationOp {
        drop_publication: "pub_app".to_string(),
    };
    let back: DropPublicationOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

// ---------------------------------------------------------------------------
// CreateSubscriptionOp / DropSubscriptionOp
// ---------------------------------------------------------------------------

#[test]
fn create_subscription_op_roundtrip() {
    let op = CreateSubscriptionOp {
        create_subscription: "sub1".to_string(),
        upstream: "tls://leader:7432".to_string(),
        publication: "pub_app".to_string(),
        profile: "cluster".to_string(),
    };
    let back: CreateSubscriptionOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

#[test]
fn drop_subscription_op_roundtrip() {
    let op = DropSubscriptionOp {
        drop_subscription: "sub1".to_string(),
    };
    let back: DropSubscriptionOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

// ---------------------------------------------------------------------------
// AlterSubscriptionOp — all three SubAction variants
// ---------------------------------------------------------------------------

#[test]
fn alter_subscription_op_pause_roundtrip() {
    let op = AlterSubscriptionOp {
        alter_subscription: "sub1".to_string(),
        action: SubAction::Pause,
    };
    let back: AlterSubscriptionOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

#[test]
fn alter_subscription_op_resume_roundtrip() {
    let op = AlterSubscriptionOp {
        alter_subscription: "sub1".to_string(),
        action: SubAction::Resume,
    };
    let back: AlterSubscriptionOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

#[test]
fn alter_subscription_op_set_profile_roundtrip() {
    let op = AlterSubscriptionOp {
        alter_subscription: "sub1".to_string(),
        action: SubAction::SetProfile("edge".to_string()),
    };
    let back: AlterSubscriptionOp = from_qv(to_qv(&op));
    assert_eq!(back, op);
}

// ---------------------------------------------------------------------------
// Read-only introspection ops — boolean discriminator (access_tree convention)
// ---------------------------------------------------------------------------

#[test]
fn list_publications_op_roundtrip_and_default() {
    // Default-constructed (false discriminator) serializes WITHOUT the key
    // (skip_serializing_if = "is_false") — byte-identical to a no-op default.
    let op = ListPublicationsOp::default();
    assert!(!op.list_publications);
    let qv = to_qv(&op);
    assert!(
        qv.get("list_publications").is_none(),
        "false discriminator must be omitted: {qv:?}"
    );
    let back: ListPublicationsOp = from_qv(qv);
    assert_eq!(back, op);

    // From msgpack — a present discriminator (`true`) round-trips.
    let qv = mpack!({"list_publications": true});
    let op: ListPublicationsOp = from_qv(qv);
    assert!(op.list_publications);
}

#[test]
fn list_subscriptions_op_roundtrip_and_default() {
    let op = ListSubscriptionsOp::default();
    assert!(!op.list_subscriptions);
    let qv = to_qv(&op);
    assert!(qv.get("list_subscriptions").is_none());
    let back: ListSubscriptionsOp = from_qv(qv);
    assert_eq!(back, op);

    let qv = mpack!({"list_subscriptions": true});
    let op: ListSubscriptionsOp = from_qv(qv);
    assert!(op.list_subscriptions);
}

#[test]
fn replication_status_op_roundtrip_and_default() {
    let op = ReplicationStatusOp::default();
    assert!(!op.replication_status);
    let qv = to_qv(&op);
    assert!(qv.get("replication_status").is_none());
    let back: ReplicationStatusOp = from_qv(qv);
    assert_eq!(back, op);

    let qv = mpack!({"replication_status": true});
    let op: ReplicationStatusOp = from_qv(qv);
    assert!(op.replication_status);
}
