use shamir_collections::TMap;
use shamir_types::access::{Action, ResourcePath};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::{GroupRef, ResourceRef};
use crate::admin::{ReplDirection, ReplMode, SubAction};
use crate::batch::{
    BatchLimits, BatchOp, BatchRequest, BatchResponse, EdgeKind, InternerDelta, QueryEntry,
    ResultEncoding, SubBatchOp, TransactionInfo,
};
use crate::filter::FilterValue;

fn from_qv<T: serde::de::DeserializeOwned>(qv: QueryValue) -> T {
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn to_qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn roundtrip_op(qv: QueryValue) -> BatchOp {
    let op: BatchOp = from_qv(qv);
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let op2: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op, op2);
    op
}

#[test]
fn start_migration_serde() {
    let op = roundtrip_op(mpack!({
        "start_migration": "users",
        "repo": "main",
        "dst_repo": "cold",
        "dst_engine": "redb",
        "dst_path": "/data/cold",
        "hmac": "deadbeef"
    }));
    match &op {
        BatchOp::StartMigration(m) => {
            assert_eq!(m.start_migration, "users");
            assert_eq!(m.repo, "main");
            assert_eq!(m.dst_repo, "cold");
            assert_eq!(m.dst_engine, "redb");
            assert_eq!(m.dst_path.as_deref(), Some("/data/cold"));
            assert_eq!(m.hmac.as_deref(), Some("deadbeef"));
        }
        _ => panic!("expected StartMigration"),
    }
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());
}

#[test]
fn start_migration_defaults() {
    let op = roundtrip_op(mpack!({
        "start_migration": "logs",
        "dst_repo": "archive",
        "dst_engine": "fjall"
    }));
    match &op {
        BatchOp::StartMigration(m) => {
            assert_eq!(m.repo, "main");
            assert!(m.dst_path.is_none());
            assert!(m.hmac.is_none());
        }
        _ => panic!("expected StartMigration"),
    }
}

#[test]
fn commit_migration_serde() {
    let op = roundtrip_op(mpack!({"commit_migration": "mig-001", "hmac": "abcd1234"}));
    match &op {
        BatchOp::CommitMigration(m) => {
            assert_eq!(m.commit_migration, "mig-001");
            assert_eq!(m.hmac.as_deref(), Some("abcd1234"));
        }
        _ => panic!("expected CommitMigration"),
    }
    assert!(op.is_admin());
}

#[test]
fn rollback_migration_serde() {
    let op = roundtrip_op(mpack!({"rollback_migration": "mig-001", "hmac": "ff00"}));
    match &op {
        BatchOp::RollbackMigration(m) => {
            assert_eq!(m.rollback_migration, "mig-001");
            assert_eq!(m.hmac.as_deref(), Some("ff00"));
        }
        _ => panic!("expected RollbackMigration"),
    }
}

#[test]
fn migration_status_serde() {
    let op = roundtrip_op(mpack!({"migration_status": "mig-001"}));
    match &op {
        BatchOp::MigrationStatus(m) => assert_eq!(m.migration_status, "mig-001"),
        _ => panic!("expected MigrationStatus"),
    }
    assert!(op.is_admin());
}

#[test]
fn batch_request_parses_isolation_field() {
    let qv = mpack!({
        "id": 1_i64,
        "transactional": true,
        "isolation": "serializable",
        "queries": {}
    });
    let req: BatchRequest = from_qv(qv);
    assert!(req.transactional);
    assert_eq!(req.isolation, Some("serializable".to_string()));
}

#[test]
fn batch_request_isolation_defaults_to_none() {
    let qv = mpack!({
        "id": 2_i64,
        "transactional": true,
        "queries": {}
    });
    let req: BatchRequest = from_qv(qv);
    assert!(req.isolation.is_none());
}

#[test]
fn batch_request_parses_durability_field() {
    let qv = mpack!({
        "id": 3_i64,
        "durability": "synced",
        "queries": {}
    });
    let req: BatchRequest = from_qv(qv);
    assert_eq!(req.durability, Some("synced".to_string()));
}

#[test]
fn batch_request_durability_defaults_to_none() {
    let qv = mpack!({
        "id": 4_i64,
        "queries": {}
    });
    let req: BatchRequest = from_qv(qv);
    assert!(req.durability.is_none());
}

#[test]
fn batch_request_durability_not_serialized_when_none() {
    let qv = mpack!({
        "id": 5_i64,
        "queries": {}
    });
    let req: BatchRequest = from_qv(qv);
    let back = to_qv(&req);
    assert!(back.get("durability").is_none());
}

#[test]
fn transaction_info_committed_roundtrip() {
    let info = TransactionInfo::committed(42, 100, 105, true);
    assert!(info.is_committed());
    assert_eq!(info.tx_id, 42);
    assert_eq!(info.snapshot_version, Some(100));
    assert_eq!(info.commit_version, Some(105));
    assert!(info.materialized);

    let qv = to_qv(&info);
    assert_eq!(
        qv.get("status").and_then(QueryValue::as_str),
        Some("committed")
    );
    assert_eq!(
        qv.get("materialized").and_then(QueryValue::as_bool),
        Some(true)
    );
    assert!(qv.get("reason").is_none()); // skip_serializing_if
}

#[test]
fn transaction_info_aborted_roundtrip() {
    let info = TransactionInfo::aborted(7, "tx_conflict");
    assert!(!info.is_committed());
    assert_eq!(info.reason, Some("tx_conflict".to_string()));

    let qv = to_qv(&info);
    assert_eq!(
        qv.get("status").and_then(QueryValue::as_str),
        Some("aborted")
    );
    assert_eq!(
        qv.get("reason").and_then(QueryValue::as_str),
        Some("tx_conflict")
    );
}

#[test]
fn transaction_info_deferred_materialization_roundtrip() {
    // A committed-but-deferred outcome must report materialized=false
    // and round-trip the flag through serde.
    let info = TransactionInfo::committed(9, 200, 201, false);
    assert!(info.is_committed());
    assert!(!info.materialized);

    let qv = to_qv(&info);
    assert_eq!(
        qv.get("status").and_then(QueryValue::as_str),
        Some("committed")
    );
    assert_eq!(
        qv.get("materialized").and_then(QueryValue::as_bool),
        Some(false)
    );

    let back: TransactionInfo = from_qv(qv);
    assert_eq!(back, info);
    assert!(!back.materialized);
}

#[test]
fn transaction_info_missing_materialized_defaults_true() {
    // Backward-compat: a payload serialized before `materialized`
    // existed (field absent) must deserialize to the fully-applied
    // common case (materialized=true), not a deferred commit.
    let qv = mpack!({
        "tx_id": 5_i64,
        "status": "committed",
        "snapshot_version": 100_i64,
        "commit_version": 105_i64
    });
    let info: TransactionInfo = from_qv(qv);
    assert!(info.is_committed());
    assert!(
        info.materialized,
        "absent `materialized` field must default to true"
    );
}

// ========================================================================
// Access-control DDL ser/de round-trip (S3)
// ========================================================================

#[test]
fn chmod_table_serde() {
    let op = roundtrip_op(mpack!({
        "chmod": {
            "table": ["mydb", "main", "users"]
        },
        "mode": 448_i64
    }));
    match &op {
        BatchOp::Chmod(c) => {
            assert_eq!(c.mode, 0o700);
            match &c.chmod {
                ResourceRef::Table { table } => {
                    assert_eq!(table, &["mydb", "main", "users"]);
                }
                _ => panic!("expected Table ResourceRef"),
            }
        }
        _ => panic!("expected Chmod"),
    }
    assert!(op.is_admin());
}

#[test]
fn chown_database_serde() {
    let op = roundtrip_op(mpack!({
        "chown": {
            "database": "testdb"
        },
        "owner": 7_i64
    }));
    match &op {
        BatchOp::Chown(c) => {
            assert_eq!(c.owner, 7);
            match &c.chown {
                ResourceRef::Database { database } => {
                    assert_eq!(database, "testdb");
                }
                _ => panic!("expected Database ResourceRef"),
            }
        }
        _ => panic!("expected Chown"),
    }
}

#[test]
fn chgrp_store_serde() {
    let op = roundtrip_op(mpack!({
        "chgrp": {
            "store": ["testdb", "main"]
        },
        "group": 3_i64
    }));
    match &op {
        BatchOp::Chgrp(c) => {
            assert_eq!(c.group, Some(3));
        }
        _ => panic!("expected Chgrp"),
    }
}

#[test]
fn chgrp_null_group_serde() {
    let op = roundtrip_op(mpack!({
        "chgrp": {
            "database": "testdb"
        },
        "group": null
    }));
    match &op {
        BatchOp::Chgrp(c) => {
            assert!(c.group.is_none());
        }
        _ => panic!("expected Chgrp"),
    }
}

#[test]
fn create_group_serde() {
    let op = roundtrip_op(mpack!({
        "create_group": "devs"
    }));
    match &op {
        BatchOp::CreateGroup(c) => {
            assert_eq!(c.create_group, "devs");
        }
        _ => panic!("expected CreateGroup"),
    }
}

#[test]
fn drop_group_by_name_serde() {
    let op = roundtrip_op(mpack!({
        "drop_group": {
            "name": "devs"
        }
    }));
    match &op {
        BatchOp::DropGroup(d) => match &d.drop_group {
            GroupRef::Name { name } => assert_eq!(name, "devs"),
            _ => panic!("expected Name GroupRef"),
        },
        _ => panic!("expected DropGroup"),
    }
}

#[test]
fn drop_group_by_id_serde() {
    let op = roundtrip_op(mpack!({
        "drop_group": {
            "id": 3_i64
        }
    }));
    match &op {
        BatchOp::DropGroup(d) => match &d.drop_group {
            GroupRef::Id { id } => assert_eq!(*id, 3),
            _ => panic!("expected Id GroupRef"),
        },
        _ => panic!("expected DropGroup"),
    }
}

#[test]
fn add_group_member_serde() {
    let op = roundtrip_op(mpack!({
        "add_group_member": {
            "name": "devs"
        },
        "user": 42_i64
    }));
    match &op {
        BatchOp::AddGroupMember(a) => {
            assert_eq!(a.user, 42);
        }
        _ => panic!("expected AddGroupMember"),
    }
}

#[test]
fn remove_group_member_serde() {
    let op = roundtrip_op(mpack!({
        "remove_group_member": {
            "id": 1_i64
        },
        "user": 42_i64
    }));
    match &op {
        BatchOp::RemoveGroupMember(r) => {
            assert_eq!(r.user, 42);
        }
        _ => panic!("expected RemoveGroupMember"),
    }
}

// ========================================================================
// QueryEntry `after` field ser/de
// ========================================================================

#[test]
fn query_entry_after_nonempty_roundtrip() {
    let qv = mpack!({
        "from": "orders",
        "return_result": true,
        "after": ["create_tbl"]
    });
    let entry: QueryEntry = from_qv(qv);
    assert_eq!(entry.after, vec!["create_tbl".to_string()]);

    let back = to_qv(&entry);
    let after_list = back.get("after").expect("after key present");
    if let QueryValue::List(items) = after_list {
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].as_str(), Some("create_tbl"));
    } else {
        panic!("expected List for 'after', got {after_list:?}");
    }

    let bytes = rmp_serde::to_vec_named(&entry).unwrap();
    let entry2: QueryEntry = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(entry, entry2);
}

#[test]
fn query_entry_empty_after_omitted_from_wire() {
    let qv = mpack!({
        "from": "orders",
        "return_result": true
    });
    let entry: QueryEntry = from_qv(qv);
    assert!(entry.after.is_empty());

    let back = to_qv(&entry);
    assert!(
        back.get("after").is_none(),
        "empty `after` must NOT appear in serialized output"
    );
}

#[test]
fn chmod_function_namespace_serde() {
    let op = roundtrip_op(mpack!({
        "chmod": {
            "function_namespace": true
        },
        "mode": 493_i64
    }));
    match &op {
        BatchOp::Chmod(c) => {
            assert_eq!(c.mode, 0o755);
            match &c.chmod {
                ResourceRef::FunctionNamespace { .. } => {}
                _ => panic!("expected FunctionNamespace ResourceRef"),
            }
        }
        _ => panic!("expected Chmod"),
    }
}

#[test]
fn access_tree_serde() {
    let op = roundtrip_op(mpack!({
        "access_tree": true,
        "depth": 2_i64
    }));
    match &op {
        BatchOp::AccessTree(a) => {
            assert!(a.access_tree);
            assert_eq!(a.depth, Some(2));
            assert!(a.db.is_none());
        }
        _ => panic!("expected AccessTree"),
    }
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());
}

#[test]
fn access_tree_defaults_serde() {
    let op = roundtrip_op(mpack!({"access_tree": true}));
    match &op {
        BatchOp::AccessTree(a) => {
            assert!(a.access_tree);
            assert!(a.depth.is_none());
            assert!(a.db.is_none());
        }
        _ => panic!("expected AccessTree"),
    }
}

// ========================================================================
// Nested batch types + wire serde (P1)
// ========================================================================

#[test]
fn nested_batch_serde_roundtrip() {
    let inner = BatchRequest {
        id: QueryValue::Int(99),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    };
    let mut bind = TMap::default();
    bind.insert("uid".to_string(), FilterValue::String("u1".into()));
    let sub = SubBatchOp { batch: inner, bind };
    let op = BatchOp::Batch(sub);

    // Verify the serialized output has the expected keys.
    let qv = to_qv(&op);
    assert!(
        qv.get("batch").is_some(),
        "serialized output must have 'batch' key"
    );
    assert!(
        qv.get("bind").is_some(),
        "serialized output must have 'bind' key"
    );

    // Round-trip through msgpack — the wire codec.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let back: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op, back);
}

#[test]
fn nested_batch_empty_bind_omitted() {
    let inner = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    };
    let op = BatchOp::Batch(SubBatchOp {
        batch: inner,
        bind: TMap::default(),
    });

    let qv = to_qv(&op);
    assert!(
        qv.get("bind").is_none(),
        "empty bind must NOT appear in serialized output"
    );

    // Verify msgpack round-trip also works for the empty-bind case.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let back: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op, back);
}

#[test]
fn nested_batch_dispatch_by_batch_key() {
    let op: BatchOp = from_qv(mpack!({"batch": {"id": 1_i64, "queries": {}}}));
    assert!(matches!(op, BatchOp::Batch(_)));
}

#[test]
fn nested_batch_is_admin() {
    let inner = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    };
    let op = BatchOp::Batch(SubBatchOp {
        batch: inner,
        bind: TMap::default(),
    });
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());
}

#[test]
fn filter_value_param_serde() {
    let v = FilterValue::Param { name: "uid".into() };
    let bytes = rmp_serde::to_vec_named(&v).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.get("$param").and_then(QueryValue::as_str), Some("uid"));

    let back: FilterValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(v, back);
}

#[test]
fn batch_limits_default_nesting_depth() {
    assert_eq!(BatchLimits::default().max_nesting_depth, 4);
}

#[test]
fn chown_function_serde() {
    let op = roundtrip_op(mpack!({
        "chown": {
            "function": "my_fn"
        },
        "owner": 10_i64
    }));
    match &op {
        BatchOp::Chown(c) => match &c.chown {
            ResourceRef::Function { function } => {
                assert_eq!(function, "my_fn");
            }
            _ => panic!("expected Function ResourceRef"),
        },
        _ => panic!("expected Chown"),
    }
}

// ========================================================================
// Ambient interner epoch-delta fields (Stage 5-wire Part A)
// ========================================================================

#[test]
fn batch_request_interner_epochs_omitted_when_empty() {
    let req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    };
    let qv = to_qv(&req);
    assert!(
        qv.get("interner_epochs").is_none(),
        "empty interner_epochs must be omitted: {qv:?}"
    );
    let back: BatchRequest = from_qv(qv);
    assert_eq!(req, back);
}

#[test]
fn batch_request_interner_epochs_roundtrip() {
    let mut epochs = TMap::default();
    epochs.insert("repo_a".to_string(), 5u64);
    epochs.insert("repo_b".to_string(), 42u64);
    let req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: epochs,
        result_encoding: ResultEncoding::default(),
    };
    let qv = to_qv(&req);
    assert!(
        qv.get("interner_epochs").is_some(),
        "non-empty interner_epochs must appear: {qv:?}"
    );
    let back: BatchRequest = from_qv(qv);
    assert_eq!(req, back);
    assert_eq!(back.interner_epochs.get("repo_a"), Some(&5u64));
    assert_eq!(back.interner_epochs.get("repo_b"), Some(&42u64));
}

#[test]
fn batch_request_backward_compat_old_peer_no_field() {
    // An old client that doesn't know interner_epochs sends a payload without it.
    let qv = mpack!({"id": 1_i64, "queries": {}});
    let req: BatchRequest = from_qv(qv);
    assert!(req.interner_epochs.is_empty());
}

#[test]
fn batch_response_interner_delta_omitted_when_empty() {
    let resp = BatchResponse {
        id: QueryValue::Int(1),
        results: TMap::default(),
        execution_plan: vec![],
        edge_provenance: TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: TMap::default(),
    };
    let qv = to_qv(&resp);
    assert!(
        qv.get("interner_delta").is_none(),
        "empty interner_delta must be omitted: {qv:?}"
    );
    let back: BatchResponse = from_qv(qv);
    assert_eq!(resp, back);
}

#[test]
fn batch_response_interner_delta_roundtrip() {
    let mut delta = TMap::default();
    delta.insert(
        "main".to_string(),
        InternerDelta {
            epoch: 10,
            entries: vec![(7, "alpha".to_string()), (8, "beta".to_string())],
        },
    );
    let resp = BatchResponse {
        id: QueryValue::Int(1),
        results: TMap::default(),
        execution_plan: vec![],
        edge_provenance: TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: delta,
    };
    let qv = to_qv(&resp);
    assert!(
        qv.get("interner_delta").is_some(),
        "non-empty interner_delta must appear: {qv:?}"
    );
    let back: BatchResponse = from_qv(qv);
    assert_eq!(resp, back);
    let d = back.interner_delta.get("main").expect("main delta");
    assert_eq!(d.epoch, 10);
    assert_eq!(d.entries.len(), 2);
    assert_eq!(d.entries[0], (7, "alpha".to_string()));
}

#[test]
fn batch_response_backward_compat_old_peer_no_field() {
    // An old server that doesn't know interner_delta sends a payload without it.
    let qv = mpack!({
        "id": 1_i64,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0_i64
    });
    let resp: BatchResponse = from_qv(qv);
    assert!(resp.interner_delta.is_empty());
}

// ========================================================================
// edge_provenance field (task #630 — Epic01/C gap #4)
// ========================================================================
//
// The internal `BatchPlan.edge_provenance` is unit-tested extensively
// (planner_tests.rs, both crates). What was NOT covered anywhere: that
// `BatchResponse.edge_provenance` — the wire-level DTO field a real client
// actually deserializes off the socket — round-trips through msgpack with
// its `EdgeKind` values intact, and is omitted when empty (mirroring the
// `interner_delta` wire-compat tests directly above). This is the
// serialization contract a server-round-trip integration test would rely
// on; pinning it here at the DTO level is the precise, fast unit
// counterpart.

#[test]
fn batch_response_edge_provenance_omitted_when_empty() {
    let resp = BatchResponse {
        id: QueryValue::Int(1),
        results: TMap::default(),
        execution_plan: vec![],
        edge_provenance: TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: TMap::default(),
    };
    let qv = to_qv(&resp);
    assert!(
        qv.get("edge_provenance").is_none(),
        "empty edge_provenance must be omitted: {qv:?}"
    );
    let back: BatchResponse = from_qv(qv);
    assert_eq!(resp, back);
}

#[test]
fn batch_response_edge_provenance_roundtrip() {
    let mut orders_provenance: TMap<String, EdgeKind> = TMap::default();
    orders_provenance.insert("users".to_string(), EdgeKind::DataFlow);
    orders_provenance.insert("marker".to_string(), EdgeKind::Explicit);

    let mut edge_provenance: TMap<String, TMap<String, EdgeKind>> = TMap::default();
    edge_provenance.insert("orders".to_string(), orders_provenance);

    let resp = BatchResponse {
        id: QueryValue::Int(1),
        results: TMap::default(),
        execution_plan: vec![
            vec!["users".to_string(), "marker".to_string()],
            vec!["orders".to_string()],
        ],
        edge_provenance,
        execution_time_us: 42,
        transaction: None,
        interner_delta: TMap::default(),
    };

    let qv = to_qv(&resp);
    assert!(
        qv.get("edge_provenance").is_some(),
        "non-empty edge_provenance must appear on the wire: {qv:?}"
    );
    let back: BatchResponse = from_qv(qv);
    assert_eq!(resp, back);

    let orders = back
        .edge_provenance
        .get("orders")
        .expect("orders provenance entry");
    assert_eq!(orders.get("users"), Some(&EdgeKind::DataFlow));
    assert_eq!(orders.get("marker"), Some(&EdgeKind::Explicit));
}

#[test]
fn batch_response_edge_provenance_backward_compat_old_peer_no_field() {
    // An old server that predates edge_provenance sends a payload without it.
    let qv = mpack!({
        "id": 1_i64,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0_i64
    });
    let resp: BatchResponse = from_qv(qv);
    assert!(resp.edge_provenance.is_empty());
}

// ========================================================================
// result_encoding field (S-read pass-through, inert DTO)
// ========================================================================

/// An old client payload that has no `result_encoding` field must deserialize
/// with `ResultEncoding::Name` as the default — backward-compat preserved.
#[test]
fn batch_request_result_encoding_defaults_to_name() {
    let qv = mpack!({"id": 1_i64, "queries": {}});
    let req: BatchRequest = from_qv(qv);
    assert_eq!(
        req.result_encoding,
        ResultEncoding::Name,
        "absent result_encoding must default to Name"
    );
}

/// A payload with `result_encoding = "id"` must round-trip via msgpack.
#[test]
fn batch_request_result_encoding_id_roundtrip_via_msgpack() {
    let req = BatchRequest {
        id: QueryValue::Int(42),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::Id,
    };

    let bytes = rmp_serde::to_vec_named(&req).unwrap();
    let back: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        back.result_encoding,
        ResultEncoding::Id,
        "result_encoding must survive a msgpack round-trip"
    );
}

// ========================================================================
// BatchOp::is_write() — follower read-only gate classification (PR1)
// ========================================================================
//
// is_write() is the symmetric counterpart of is_admin(): it classifies
// whether an op mutates data or persistent server state. See the doc on
// `BatchOp::is_write` for the full rationale. These tests pin the
// classification of representative variants so a future re-classification
// is a conscious decision, not a silent regression.

/// Build a minimal `BatchRequest` carrying a single `QueryEntry` whose `op`
/// is `inner`. Used by the sub-batch recursion tests.
fn single_query_batch(alias: &str, inner: BatchOp) -> BatchRequest {
    let mut queries: TMap<String, QueryEntry> = TMap::default();
    queries.insert(
        alias.to_string(),
        QueryEntry {
            op: inner,
            return_result: true,
            after: Vec::new(),
        },
    );
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

#[test]
fn is_write_read_is_false() {
    let op = roundtrip_op(mpack!({"from": "users"}));
    assert!(matches!(op, BatchOp::Read(_)));
    assert!(!op.is_write());
}

#[test]
fn is_write_insert_is_true() {
    let op = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    assert!(matches!(op, BatchOp::Insert(_)));
    assert!(op.is_write());
}

#[test]
fn is_write_set_is_true() {
    let op = roundtrip_op(mpack!({"set": "users", "key": {"id": 1_i64}, "value": {"name": "x"}}));
    assert!(matches!(op, BatchOp::Set(_)));
    assert!(op.is_write());
}

#[test]
fn is_write_delete_is_true() {
    let op = roundtrip_op(mpack!({
        "delete_from": "users",
        "where": {"op": "eq", "field": "id", "value": 1_i64}
    }));
    assert!(matches!(op, BatchOp::Delete(_)));
    assert!(op.is_write());
}

// ============================================================================
// #546: `required_access` — regression against the pre-fix duplicated match
// ============================================================================
//
// `BatchOp::required_access` replaces a byte-for-byte-duplicated inline
// match that used to live separately in `shamir-db`'s `execute_as` and
// `tx_execute_as`. These tests pin the exact `(Action, ResourcePath)` the
// old duplicated code produced for a representative sample of ops, so a
// future edit to `required_access` cannot silently drift from that
// established behavior. `is_write`'s own exhaustive-match convention
// covers "does this compile for every variant" — the property these
// tests check is behavioral: same inputs, same `(Action, ResourcePath)`
// as before the refactor.

#[test]
fn required_access_read_is_read_action() {
    let op = roundtrip_op(mpack!({"from": "users"}));
    assert!(matches!(op, BatchOp::Read(_)));
    let (action, path) = op.required_access("mydb").expect("Read has a table_ref");
    assert_eq!(action, Action::Read);
    assert_eq!(path, ResourcePath::table("mydb", "main", "users"));
}

#[test]
fn required_access_insert_is_create_action() {
    let op = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    assert!(matches!(op, BatchOp::Insert(_)));
    let (action, path) = op.required_access("mydb").expect("Insert has a table_ref");
    assert_eq!(action, Action::Create);
    assert_eq!(path, ResourcePath::table("mydb", "main", "users"));
}

#[test]
fn required_access_set_is_write_action() {
    let op = roundtrip_op(mpack!({"set": "users", "key": {"id": 1_i64}, "value": {"name": "x"}}));
    assert!(matches!(op, BatchOp::Set(_)));
    let (action, path) = op.required_access("mydb").expect("Set has a table_ref");
    assert_eq!(action, Action::Write);
    assert_eq!(path, ResourcePath::table("mydb", "main", "users"));
}

#[test]
fn required_access_update_is_write_action() {
    let op = roundtrip_op(mpack!({
        "update": "users",
        "where": {"op": "eq", "field": "id", "value": 1_i64},
        "set": {"name": "y"}
    }));
    assert!(matches!(op, BatchOp::Update(_)));
    let (action, path) = op.required_access("mydb").expect("Update has a table_ref");
    assert_eq!(action, Action::Write);
    assert_eq!(path, ResourcePath::table("mydb", "main", "users"));
}

#[test]
fn required_access_delete_is_delete_action() {
    let op = roundtrip_op(mpack!({
        "delete_from": "users",
        "where": {"op": "eq", "field": "id", "value": 1_i64}
    }));
    assert!(matches!(op, BatchOp::Delete(_)));
    let (action, path) = op.required_access("mydb").expect("Delete has a table_ref");
    assert_eq!(action, Action::Delete);
    assert_eq!(path, ResourcePath::table("mydb", "main", "users"));
}

#[test]
fn required_access_honors_explicit_repo() {
    // TableRef's ["repo", "table"] wire form — the resulting
    // ResourcePath::Table must carry the explicit repo/store, not the
    // "main" default repo used by the bare string shorthand in the
    // other tests above.
    let op = roundtrip_op(mpack!({"from": ["sales", "orders"]}));
    let (action, path) = op.required_access("mydb").expect("Read has a table_ref");
    assert_eq!(action, Action::Read);
    assert_eq!(path, ResourcePath::table("mydb", "sales", "orders"));
}

#[test]
fn required_access_none_for_admin_ops() {
    // Admin/DDL ops carry no table_ref() and are authorized separately in
    // execute_admin — required_access must return None for them, matching
    // table_ref()'s own None.
    let op = roundtrip_op(mpack!({"create_db": "newdb"}));
    assert!(op.table_ref().is_none());
    assert!(op.required_access("mydb").is_none());
}

#[test]
fn required_access_none_for_batch_and_subscribe() {
    // Batch/Subscribe/Unsubscribe have no table_ref() either, despite
    // carrying table-shaped data internally (recursion / grant markers
    // handle their own authorization elsewhere).
    let inner_insert = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    let sub_batch = SubBatchOp {
        batch: single_query_batch("inner", inner_insert),
        bind: TMap::default(),
    };
    let op = BatchOp::Batch(sub_batch);
    assert!(op.table_ref().is_none());
    assert!(op.required_access("mydb").is_none());
}

#[test]
fn is_write_create_user_is_true() {
    let op = roundtrip_op(mpack!({"create_user": "alice", "password": "s3cr3t"}));
    assert!(matches!(op, BatchOp::CreateUser(_)));
    assert!(op.is_write());
}

#[test]
fn is_write_describe_table_is_false() {
    let op = roundtrip_op(mpack!({"describe_table": "users", "repo": "main"}));
    assert!(matches!(op, BatchOp::DescribeTable(_)));
    assert!(!op.is_write());
}

#[test]
fn is_write_call_is_true() {
    // WASM stored-procedure invocation is conservatively a write: a host
    // function may mutate state.
    let op = roundtrip_op(mpack!({"call": "my_proc", "repo": "main"}));
    assert!(matches!(op, BatchOp::Call(_)));
    assert!(op.is_write());
}

#[test]
fn is_write_read_only_admin_introspection_is_false() {
    // Spot-check the read-only admin/introspection variants.
    let list = roundtrip_op(mpack!({"list": "databases"}));
    assert!(matches!(list, BatchOp::List(_)));
    assert!(!list.is_write());

    let get_buf = roundtrip_op(mpack!({"get_buffer_config": "users", "repo": "main"}));
    assert!(matches!(get_buf, BatchOp::GetBufferConfig(_)));
    assert!(!get_buf.is_write());

    let mig_status = roundtrip_op(mpack!({"migration_status": "m1"}));
    assert!(matches!(mig_status, BatchOp::MigrationStatus(_)));
    assert!(!mig_status.is_write());
}

#[test]
fn is_write_subbatch_with_only_reads_is_false() {
    let inner_read = roundtrip_op(mpack!({"from": "users"}));
    let sub = SubBatchOp {
        batch: single_query_batch("r", inner_read),
        bind: TMap::default(),
    };
    let op = BatchOp::Batch(sub);
    assert!(
        !op.is_write(),
        "sub-batch of pure reads must not be a write"
    );
}

#[test]
fn is_write_subbatch_with_read_and_insert_is_true() {
    let inner_read = roundtrip_op(mpack!({"from": "users"}));
    let inner_insert = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    let mut queries: TMap<String, QueryEntry> = TMap::default();
    queries.insert(
        "r".to_string(),
        QueryEntry {
            op: inner_read,
            return_result: true,
            after: Vec::new(),
        },
    );
    queries.insert(
        "i".to_string(),
        QueryEntry {
            op: inner_insert,
            return_result: true,
            after: Vec::new(),
        },
    );
    let batch = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    };
    let op = BatchOp::Batch(SubBatchOp {
        batch,
        bind: TMap::default(),
    });
    assert!(
        op.is_write(),
        "sub-batch containing a write op must be a write"
    );
}

#[test]
fn is_write_nested_subbatch_depth_2_is_true() {
    // Depth-2 nesting: outer sub-batch wraps an inner sub-batch that
    // contains an Insert. The recursion must reach down two levels.
    let inner_insert = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    let inner_batch = BatchOp::Batch(SubBatchOp {
        batch: single_query_batch("i", inner_insert),
        bind: TMap::default(),
    });
    let outer = BatchOp::Batch(SubBatchOp {
        batch: single_query_batch("nested", inner_batch),
        bind: TMap::default(),
    });
    assert!(
        outer.is_write(),
        "nested sub-batch (depth 2) with a write at the leaf must be a write"
    );
}

// ========================================================================
// Replication DDL ops — BatchOp dispatch / serde / classification
// (REPLICATION.md §5.5, REPLICATION-CLIENT-SURFACE.md §2-3)
// ========================================================================
//
// These tests pin the wire discriminator of each repl-DDL op, the serde
// round-trip of the nested enums (ReplDirection / ReplMode / SubAction),
// and the is_admin / is_write classification of all 10 new BatchOp
// variants. list_publications / list_subscriptions / replication_status
// are read-only introspection (is_write == false); the create/drop/alter
// ops are write-classified.

#[test]
fn create_replication_profile_serde_and_classify() {
    let op = roundtrip_op(mpack!({
        "create_replication_profile": "cluster",
        "streams": [
            {
                "scope": {"db": "app", "repo": "main", "table": "users"},
                "direction": "pull",
                "mode": "read_only"
            },
            {
                // db-only scope — repo/table omitted.
                "scope": {"db": "system"},
                "direction": "both",
                "mode": "read_write"
            }
        ]
    }));
    match &op {
        BatchOp::CreateReplicationProfile(c) => {
            assert_eq!(c.create_replication_profile, "cluster");
            assert_eq!(c.streams.len(), 2);
            // First stream — full scope + Pull + ReadOnly.
            let s0 = &c.streams[0];
            assert_eq!(s0.scope.db, "app");
            assert_eq!(s0.scope.repo.as_deref(), Some("main"));
            assert_eq!(s0.scope.table.as_deref(), Some("users"));
            assert_eq!(s0.direction, ReplDirection::Pull);
            assert_eq!(s0.mode, ReplMode::ReadOnly);
            // Second stream — db-only scope + Both + ReadWrite.
            let s1 = &c.streams[1];
            assert_eq!(s1.scope.db, "system");
            assert!(s1.scope.repo.is_none());
            assert!(s1.scope.table.is_none());
            assert_eq!(s1.direction, ReplDirection::Both);
            assert_eq!(s1.mode, ReplMode::ReadWrite);
        }
        _ => panic!("expected CreateReplicationProfile"),
    }
    assert!(op.is_admin());
    assert!(op.is_write(), "create_replication_profile mutates catalog");
    assert!(op.table_ref().is_none());
}

#[test]
fn drop_replication_profile_serde_and_classify() {
    let op = roundtrip_op(mpack!({"drop_replication_profile": "cluster"}));
    match &op {
        BatchOp::DropReplicationProfile(d) => {
            assert_eq!(d.drop_replication_profile, "cluster");
        }
        _ => panic!("expected DropReplicationProfile"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn create_publication_serde_and_classify() {
    let op = roundtrip_op(mpack!({
        "create_publication": "pub_app",
        "scopes": [
            {"db": "app", "repo": "main"},
            {"db": "system", "repo": "system", "table": "users"}
        ]
    }));
    match &op {
        BatchOp::CreatePublication(c) => {
            assert_eq!(c.create_publication, "pub_app");
            assert_eq!(c.scopes.len(), 2);
            assert_eq!(c.scopes[0].db, "app");
            assert_eq!(c.scopes[0].repo.as_deref(), Some("main"));
            assert!(c.scopes[0].table.is_none());
            assert_eq!(c.scopes[1].db, "system");
            assert_eq!(c.scopes[1].table.as_deref(), Some("users"));
        }
        _ => panic!("expected CreatePublication"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn drop_publication_serde_and_classify() {
    let op = roundtrip_op(mpack!({"drop_publication": "pub_app"}));
    match &op {
        BatchOp::DropPublication(d) => assert_eq!(d.drop_publication, "pub_app"),
        _ => panic!("expected DropPublication"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn create_subscription_serde_and_classify() {
    let op = roundtrip_op(mpack!({
        "create_subscription": "sub1",
        "upstream": "tls://leader.cluster:7432",
        "publication": "pub_app",
        "profile": "cluster"
    }));
    match &op {
        BatchOp::CreateSubscription(c) => {
            assert_eq!(c.create_subscription, "sub1");
            assert_eq!(c.upstream, "tls://leader.cluster:7432");
            assert_eq!(c.publication, "pub_app");
            assert_eq!(c.profile, "cluster");
        }
        _ => panic!("expected CreateSubscription"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn drop_subscription_serde_and_classify() {
    let op = roundtrip_op(mpack!({"drop_subscription": "sub1"}));
    match &op {
        BatchOp::DropSubscription(d) => assert_eq!(d.drop_subscription, "sub1"),
        _ => panic!("expected DropSubscription"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn alter_subscription_pause_serde_and_classify() {
    let op = roundtrip_op(mpack!({
        "alter_subscription": "sub1",
        "action": "pause"
    }));
    match &op {
        BatchOp::AlterSubscription(a) => {
            assert_eq!(a.alter_subscription, "sub1");
            assert_eq!(a.action, SubAction::Pause);
        }
        _ => panic!("expected AlterSubscription"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn alter_subscription_set_profile_roundtrips_payload() {
    let op = roundtrip_op(mpack!({
        "alter_subscription": "sub2",
        "action": {"set_profile": "edge"}
    }));
    match &op {
        BatchOp::AlterSubscription(a) => {
            assert_eq!(a.alter_subscription, "sub2");
            assert_eq!(a.action, SubAction::SetProfile("edge".to_string()));
        }
        _ => panic!("expected AlterSubscription"),
    }
    assert!(op.is_admin());
    assert!(op.is_write());
}

#[test]
fn alter_subscription_resume_roundtrips_payload() {
    let op = roundtrip_op(mpack!({
        "alter_subscription": "sub3",
        "action": "resume"
    }));
    match &op {
        BatchOp::AlterSubscription(a) => assert_eq!(a.action, SubAction::Resume),
        _ => panic!("expected AlterSubscription"),
    }
}

#[test]
fn list_publications_serde_and_classify() {
    let op = roundtrip_op(mpack!({"list_publications": true}));
    assert!(matches!(op, BatchOp::ListPublications(_)));
    assert!(op.is_admin());
    assert!(
        !op.is_write(),
        "list_publications is read-only introspection"
    );
}

#[test]
fn list_subscriptions_serde_and_classify() {
    let op = roundtrip_op(mpack!({"list_subscriptions": true}));
    assert!(matches!(op, BatchOp::ListSubscriptions(_)));
    assert!(op.is_admin());
    assert!(!op.is_write());
}

#[test]
fn replication_status_serde_and_classify() {
    let op = roundtrip_op(mpack!({"replication_status": true}));
    assert!(matches!(op, BatchOp::ReplicationStatus(_)));
    assert!(op.is_admin());
    assert!(
        !op.is_write(),
        "replication_status is read-only introspection"
    );
}

/// Pin the snake_case wire shape of the nested enums so a future rename is
/// a conscious decision, not a silent break.
#[test]
fn repl_nested_enums_wire_shape() {
    // ReplDirection
    let bytes = rmp_serde::to_vec_named(&ReplDirection::Pull).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("pull"));
    let bytes = rmp_serde::to_vec_named(&ReplDirection::Push).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("push"));
    let bytes = rmp_serde::to_vec_named(&ReplDirection::Both).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("both"));

    // ReplMode
    let bytes = rmp_serde::to_vec_named(&ReplMode::ReadOnly).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("read_only"));
    let bytes = rmp_serde::to_vec_named(&ReplMode::ReadWrite).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("read_write"));

    // SubAction — unit variants render as bare strings; the SetProfile
    // payload renders as a single-key map.
    let bytes = rmp_serde::to_vec_named(&SubAction::Pause).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("pause"));
    let bytes = rmp_serde::to_vec_named(&SubAction::Resume).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(qv.as_str(), Some("resume"));
    let bytes = rmp_serde::to_vec_named(&SubAction::SetProfile("p".to_string())).unwrap();
    let qv: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(
        qv.get("set_profile").and_then(QueryValue::as_str),
        Some("p")
    );
}

/// `ReplDirection` / `ReplMode` default to the R1 values (Pull / ReadOnly).
/// A stream declared without explicit direction/mode must deserialize to
/// those defaults.
#[test]
fn repl_stream_defaults_to_pull_readonly() {
    let op = roundtrip_op(mpack!({
        "create_replication_profile": "defaults",
        "streams": [{"scope": {"db": "app"}}]
    }));
    match &op {
        BatchOp::CreateReplicationProfile(c) => {
            let s = &c.streams[0];
            assert_eq!(s.direction, ReplDirection::Pull);
            assert_eq!(s.mode, ReplMode::ReadOnly);
        }
        _ => panic!("expected CreateReplicationProfile"),
    }
}
