use shamir_collections::TMap;

use crate::admin::{GroupRef, ResourceRef};
use crate::batch::{
    BatchLimits, BatchOp, BatchRequest, BatchResponse, InternerDelta, QueryEntry, SubBatchOp,
    TransactionInfo,
};
use crate::filter::FilterValue;

fn roundtrip(json: &str) -> BatchOp {
    let op: BatchOp = serde_json::from_str(json).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    op
}

#[test]
fn start_migration_serde() {
    let op = roundtrip(
        r#"{
        "start_migration": "users",
        "repo": "main",
        "dst_repo": "cold",
        "dst_engine": "redb",
        "dst_path": "/data/cold",
        "hmac": "deadbeef"
    }"#,
    );
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
    let op = roundtrip(
        r#"{
        "start_migration": "logs",
        "dst_repo": "archive",
        "dst_engine": "fjall"
    }"#,
    );
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
    let op = roundtrip(r#"{"commit_migration": "mig-001", "hmac": "abcd1234"}"#);
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
    let op = roundtrip(r#"{"rollback_migration": "mig-001", "hmac": "ff00"}"#);
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
    let op = roundtrip(r#"{"migration_status": "mig-001"}"#);
    match &op {
        BatchOp::MigrationStatus(m) => assert_eq!(m.migration_status, "mig-001"),
        _ => panic!("expected MigrationStatus"),
    }
    assert!(op.is_admin());
}

#[test]
fn batch_request_parses_isolation_field() {
    let json = serde_json::json!({
        "id": 1,
        "transactional": true,
        "isolation": "serializable",
        "queries": {}
    });
    let req: BatchRequest = serde_json::from_value(json).unwrap();
    assert!(req.transactional);
    assert_eq!(req.isolation, Some("serializable".to_string()));
}

#[test]
fn batch_request_isolation_defaults_to_none() {
    let json = serde_json::json!({
        "id": 2,
        "transactional": true,
        "queries": {}
    });
    let req: BatchRequest = serde_json::from_value(json).unwrap();
    assert!(req.isolation.is_none());
}

#[test]
fn batch_request_parses_durability_field() {
    let json = serde_json::json!({
        "id": 3,
        "durability": "synced",
        "queries": {}
    });
    let req: BatchRequest = serde_json::from_value(json).unwrap();
    assert_eq!(req.durability, Some("synced".to_string()));
}

#[test]
fn batch_request_durability_defaults_to_none() {
    let json = serde_json::json!({
        "id": 4,
        "queries": {}
    });
    let req: BatchRequest = serde_json::from_value(json).unwrap();
    assert!(req.durability.is_none());
}

#[test]
fn batch_request_durability_not_serialized_when_none() {
    let json = serde_json::json!({
        "id": 5,
        "queries": {}
    });
    let req: BatchRequest = serde_json::from_value(json).unwrap();
    let back = serde_json::to_value(&req).unwrap();
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

    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["status"], "committed");
    assert_eq!(json["materialized"], true);
    assert!(json.get("reason").is_none()); // skip_serializing_if
}

#[test]
fn transaction_info_aborted_roundtrip() {
    let info = TransactionInfo::aborted(7, "tx_conflict");
    assert!(!info.is_committed());
    assert_eq!(info.reason, Some("tx_conflict".to_string()));

    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["status"], "aborted");
    assert_eq!(json["reason"], "tx_conflict");
}

#[test]
fn transaction_info_deferred_materialization_roundtrip() {
    // A committed-but-deferred outcome must report materialized=false
    // and round-trip the flag through serde.
    let info = TransactionInfo::committed(9, 200, 201, false);
    assert!(info.is_committed());
    assert!(!info.materialized);

    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["status"], "committed");
    assert_eq!(json["materialized"], false);

    let back: TransactionInfo = serde_json::from_value(json).unwrap();
    assert_eq!(back, info);
    assert!(!back.materialized);
}

#[test]
fn transaction_info_missing_materialized_defaults_true() {
    // Backward-compat: a payload serialized before `materialized`
    // existed (field absent) must deserialize to the fully-applied
    // common case (materialized=true), not a deferred commit.
    let json = serde_json::json!({
        "tx_id": 5,
        "status": "committed",
        "snapshot_version": 100,
        "commit_version": 105
    });
    let info: TransactionInfo = serde_json::from_value(json).unwrap();
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
    let op = roundtrip(
        r#"{
        "chmod": {
            "table": ["mydb", "main", "users"]
        },
        "mode": 448
    }"#,
    );
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
    let op = roundtrip(
        r#"{
        "chown": {
            "database": "testdb"
        },
        "owner": 7
    }"#,
    );
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
    let op = roundtrip(
        r#"{
        "chgrp": {
            "store": ["testdb", "main"]
        },
        "group": 3
    }"#,
    );
    match &op {
        BatchOp::Chgrp(c) => {
            assert_eq!(c.group, Some(3));
        }
        _ => panic!("expected Chgrp"),
    }
}

#[test]
fn chgrp_null_group_serde() {
    let op = roundtrip(
        r#"{
        "chgrp": {
            "database": "testdb"
        },
        "group": null
    }"#,
    );
    match &op {
        BatchOp::Chgrp(c) => {
            assert!(c.group.is_none());
        }
        _ => panic!("expected Chgrp"),
    }
}

#[test]
fn create_group_serde() {
    let op = roundtrip(
        r#"{
        "create_group": "devs"
    }"#,
    );
    match &op {
        BatchOp::CreateGroup(c) => {
            assert_eq!(c.create_group, "devs");
        }
        _ => panic!("expected CreateGroup"),
    }
}

#[test]
fn drop_group_by_name_serde() {
    let op = roundtrip(
        r#"{
        "drop_group": {
            "name": "devs"
        }
    }"#,
    );
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
    let op = roundtrip(
        r#"{
        "drop_group": {
            "id": 3
        }
    }"#,
    );
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
    let op = roundtrip(
        r#"{
        "add_group_member": {
            "name": "devs"
        },
        "user": 42
    }"#,
    );
    match &op {
        BatchOp::AddGroupMember(a) => {
            assert_eq!(a.user, 42);
        }
        _ => panic!("expected AddGroupMember"),
    }
}

#[test]
fn remove_group_member_serde() {
    let op = roundtrip(
        r#"{
        "remove_group_member": {
            "id": 1
        },
        "user": 42
    }"#,
    );
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
    let json = serde_json::json!({
        "from": "orders",
        "return_result": true,
        "after": ["create_tbl"]
    });
    let entry: QueryEntry = serde_json::from_value(json).unwrap();
    assert_eq!(entry.after, vec!["create_tbl".to_string()]);

    let back = serde_json::to_value(&entry).unwrap();
    assert_eq!(
        back.get("after").and_then(|v| v.as_array()).unwrap(),
        &[serde_json::json!("create_tbl")]
    );

    let entry2: QueryEntry = serde_json::from_value(back).unwrap();
    assert_eq!(entry, entry2);
}

#[test]
fn query_entry_empty_after_omitted_from_json() {
    let json = serde_json::json!({
        "from": "orders",
        "return_result": true
    });
    let entry: QueryEntry = serde_json::from_value(json).unwrap();
    assert!(entry.after.is_empty());

    let back = serde_json::to_value(&entry).unwrap();
    assert!(
        back.get("after").is_none(),
        "empty `after` must NOT appear in serialized JSON"
    );
}

#[test]
fn chmod_function_namespace_serde() {
    let op = roundtrip(
        r#"{
        "chmod": {
            "function_namespace": true
        },
        "mode": 493
    }"#,
    );
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
    let op = roundtrip(
        r#"{
        "access_tree": true,
        "depth": 2
    }"#,
    );
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
    let op = roundtrip(r#"{"access_tree": true}"#);
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
        id: serde_json::json!(99),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
    };
    let mut bind = TMap::default();
    bind.insert("uid".to_string(), FilterValue::String("u1".into()));
    let sub = SubBatchOp { batch: inner, bind };
    let op = BatchOp::Batch(sub);

    let json = serde_json::to_string(&op).unwrap();
    assert!(
        json.contains("\"batch\""),
        "serialized JSON must have 'batch' key"
    );
    assert!(
        json.contains("\"bind\""),
        "serialized JSON must have 'bind' key"
    );

    let back: BatchOp = serde_json::from_str(&json).unwrap();
    assert_eq!(op, back);
}

#[test]
fn nested_batch_empty_bind_omitted() {
    let inner = BatchRequest {
        id: serde_json::json!(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
    };
    let op = BatchOp::Batch(SubBatchOp {
        batch: inner,
        bind: TMap::default(),
    });

    let json = serde_json::to_string(&op).unwrap();
    assert!(
        !json.contains("\"bind\""),
        "empty bind must NOT appear in serialized JSON"
    );
}

#[test]
fn nested_batch_dispatch_by_batch_key() {
    let json = r#"{"batch": {"id": 1, "queries": {}}}"#;
    let op: BatchOp = serde_json::from_str(json).unwrap();
    assert!(matches!(op, BatchOp::Batch(_)));
}

#[test]
fn nested_batch_is_admin() {
    let inner = BatchRequest {
        id: serde_json::json!(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
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
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, r#"{"$param":"uid"}"#);

    let back: FilterValue = serde_json::from_str(&json).unwrap();
    assert_eq!(v, back);
}

#[test]
fn batch_limits_default_nesting_depth() {
    assert_eq!(BatchLimits::default().max_nesting_depth, 4);
}

#[test]
fn chown_function_serde() {
    let op = roundtrip(
        r#"{
        "chown": {
            "function": "my_fn"
        },
        "owner": 10
    }"#,
    );
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
        id: serde_json::json!(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(
        !json.contains("interner_epochs"),
        "empty interner_epochs must be omitted: {json}"
    );
    let back: BatchRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(req, back);
}

#[test]
fn batch_request_interner_epochs_roundtrip() {
    let mut epochs = TMap::default();
    epochs.insert("repo_a".to_string(), 5u64);
    epochs.insert("repo_b".to_string(), 42u64);
    let req = BatchRequest {
        id: serde_json::json!(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: epochs,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(
        json.contains("interner_epochs"),
        "non-empty interner_epochs must appear: {json}"
    );
    let back: BatchRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(req, back);
    assert_eq!(back.interner_epochs.get("repo_a"), Some(&5u64));
    assert_eq!(back.interner_epochs.get("repo_b"), Some(&42u64));
}

#[test]
fn batch_request_backward_compat_old_peer_no_field() {
    // An old client that doesn't know interner_epochs sends JSON without it.
    let json = r#"{"id":1,"queries":{}}"#;
    let req: BatchRequest = serde_json::from_str(json).unwrap();
    assert!(req.interner_epochs.is_empty());
}

#[test]
fn batch_response_interner_delta_omitted_when_empty() {
    let resp = BatchResponse {
        id: serde_json::json!(1),
        results: TMap::default(),
        execution_plan: vec![],
        execution_time_us: 0,
        transaction: None,
        interner_delta: TMap::default(),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(
        !json.contains("interner_delta"),
        "empty interner_delta must be omitted: {json}"
    );
    let back: BatchResponse = serde_json::from_str(&json).unwrap();
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
        id: serde_json::json!(1),
        results: TMap::default(),
        execution_plan: vec![],
        execution_time_us: 0,
        transaction: None,
        interner_delta: delta,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(
        json.contains("interner_delta"),
        "non-empty interner_delta must appear: {json}"
    );
    let back: BatchResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(resp, back);
    let d = back.interner_delta.get("main").expect("main delta");
    assert_eq!(d.epoch, 10);
    assert_eq!(d.entries.len(), 2);
    assert_eq!(d.entries[0], (7, "alpha".to_string()));
}

#[test]
fn batch_response_backward_compat_old_peer_no_field() {
    // An old server that doesn't know interner_delta sends JSON without it.
    let json = r#"{"id":1,"results":{},"execution_plan":[],"execution_time_us":0}"#;
    let resp: BatchResponse = serde_json::from_str(json).unwrap();
    assert!(resp.interner_delta.is_empty());
}
