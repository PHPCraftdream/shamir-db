use shamir_collections::TMap;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::{GroupRef, ResourceRef};
use crate::batch::{
    BatchLimits, BatchOp, BatchRequest, BatchResponse, InternerDelta, QueryEntry, ResultEncoding,
    SubBatchOp, TransactionInfo,
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
