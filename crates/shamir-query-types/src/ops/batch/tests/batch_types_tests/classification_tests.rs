use shamir_collections::TMap;
use shamir_types::access::{Action, ResourcePath};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::admin::{ReplDirection, ReplMode, SubAction};
use crate::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding, SubBatchOp,
};
use crate::filter::FilterValue;

use super::batch_types_tests::roundtrip_op;

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
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
            when: None,
        },
    );
    queries.insert(
        "i".to_string(),
        QueryEntry {
            op: inner_insert,
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let batch = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
// ForEach pessimistic authorization (Epic04/D, #655 gap 3; ADR Decision 5)
// ========================================================================
//
// "One layer up" from `for_each_op_tests.rs`'s `for_each_is_write_reflects_
// body` unit test: mirrors `is_write_subbatch_with_read_and_insert_is_true`
// / `is_write_nested_subbatch_depth_2_is_true` above, pinning the same
// pessimistic (maximal-branch) classification for `ForEach` -- a write body
// makes the WHOLE node a write, and (critically) `over` resolving to ZERO
// elements at runtime plays NO role in this plan-time classification.

#[test]
fn is_write_for_each_with_only_reads_is_false() {
    let inner_read = roundtrip_op(mpack!({"from": "users"}));
    let fe = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: single_query_batch("r", inner_read),
    });
    assert!(!fe.is_write(), "for_each of pure reads must not be a write");
}

#[test]
fn is_write_for_each_with_write_body_and_zero_iterations_is_true() {
    // `over` is a literal empty array -- ZERO iterations at runtime -- yet
    // the body contains an Insert, so classification must still be a
    // write. This is the exact edge case ADR Decision 5 calls out: the
    // pessimistic (maximal-branch) model classifies over the full static
    // body, independent of runtime iteration count (including 0).
    let inner_insert = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    let fe = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: single_query_batch("i", inner_insert),
    });
    assert!(
        fe.is_write(),
        "for_each with a write body must be classified is_write()==true \
         even when `over` resolves to zero iterations at runtime"
    );
}

#[test]
fn is_write_nested_for_each_depth_2_is_true() {
    // Depth-2 nesting: outer ForEach wraps a plain sub-batch that contains
    // an inner ForEach whose body has an Insert. The recursion must reach
    // down two levels, mirroring `is_write_nested_subbatch_depth_2_is_true`.
    let inner_insert = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    let inner_for_each = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: single_query_batch("i", inner_insert),
    });
    let outer = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: single_query_batch("nested", inner_for_each),
    });
    assert!(
        outer.is_write(),
        "nested for_each (depth 2) with a write at the leaf must be a write"
    );
}

#[test]
fn required_access_none_for_for_each() {
    // ForEach carries no table_ref() either -- mirrors
    // `required_access_none_for_batch_and_subscribe` -- its body's ops are
    // authorized recursively at execution time, once per iteration.
    let inner_insert = roundtrip_op(mpack!({"insert_into": "users", "values": []}));
    let fe = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: single_query_batch("i", inner_insert),
    });
    assert!(fe.table_ref().is_none());
    assert!(fe.required_access("mydb").is_none());
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
