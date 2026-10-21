use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use smallvec::smallvec;

use super::support::{insert_row, read_field_all, FkTestResolver};
use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

// ═══════════════════════════════════════════════════════════════════════════════

/// Bind a self-referential on_update FK to a single-table hierarchy.
fn bind_self_ref_on_update(
    db: &DbInstance,
    registry: &Arc<ValidatorRegistry>,
    validator_id: u64,
    validator_name: &str,
    on_update: FkAction,
) {
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["manager_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_update("employees", "id", on_update)),
            required: false,
            nullable: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);

    let vid = RecordId::from_ts(validator_id as i64);
    registry
        .register(vid, validator_name, Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id: vid,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };

    let mut table = futures::executor::block_on(db.get_table("default", "employees")).unwrap();
    table.set_validator_registry(Arc::clone(registry));
    futures::executor::block_on(table.add_validator_binding(binding)).unwrap();
}

fn setup_self_ref_db() -> DbInstance {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("employees")],
    };
    futures::executor::block_on(DbInstance::with_repos(vec![repo_config])).unwrap()
}

/// Self-ref ON UPDATE CASCADE: re-keying a manager's `id` cascades the new
/// value to all subordinates' `manager_id`.
#[tokio::test]
async fn self_ref_on_update_cascade_rekeys_subordinates() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(
        &db,
        &registry,
        9321,
        "self_ref_cascade_upd",
        FkAction::Cascade,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // CEO(id=10), Sub1(manager_id=10), Sub2(manager_id=10).
    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "S1")
            .set("manager_id", 10_i64),
    )
    .await;
    insert_row(
        &resolver,
        "i3",
        "employees",
        doc()
            .set("id", 30_i64)
            .set("name", "S2")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key CEO id 10 → 77 → both subordinates must cascade to 77.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd",
        write::update("employees")
            .where_(filter::eq("id", 10_i64))
            .set(doc().set("id", 77_i64)),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mgr_ids = read_field_all(&resolver, "employees", "manager_id").await;
    // CEO's manager_id is Null; both subordinates should be 77.
    assert!(
        mgr_ids
            .iter()
            .filter(|v| **v == QueryValue::Int(77))
            .count()
            == 2,
        "both subordinates must cascade to 77, got: {mgr_ids:?}"
    );
    assert!(
        !mgr_ids.contains(&QueryValue::Int(10)),
        "stale reference to old id 10 must not survive, got: {mgr_ids:?}"
    );
}

/// Self-ref ON UPDATE RESTRICT: re-keying a manager's `id` is blocked when a
/// subordinate still references the old value.
#[tokio::test]
async fn self_ref_on_update_restrict_blocks_rekey() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(
        &db,
        &registry,
        9322,
        "self_ref_restrict_upd",
        FkAction::Restrict,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "Sub")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key CEO id 10 → 77 → must be REJECTED (Sub still references 10).
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd",
        write::update("employees")
            .where_(filter::eq("id", 10_i64))
            .set(doc().set("id", 77_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "self-ref on_update restrict should block, got: {msg}"
            );
        }
        Ok(_) => panic!("Expected fk_restrict on self-ref rekey with subordinate"),
    }

    // Data unchanged after restrict rollback: both CEO (id=10) and Sub (id=20)
    // must survive with their original ids.
    let ids = read_field_all(&resolver, "employees", "id").await;
    assert!(
        ids.contains(&QueryValue::Int(10)),
        "CEO id=10 should be unchanged after restrict rollback, got: {ids:?}"
    );
    assert!(
        ids.contains(&QueryValue::Int(20)),
        "Sub id=20 should be unchanged after restrict rollback, got: {ids:?}"
    );
}

/// Self-ref ON UPDATE SET NULL: re-keying a manager's `id` nulls the
/// subordinates' `manager_id`.
#[tokio::test]
async fn self_ref_on_update_set_null_nulls_subordinates() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(
        &db,
        &registry,
        9323,
        "self_ref_setnull_upd",
        FkAction::SetNull,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "Sub")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key CEO id 10 → 77 → Sub.manager_id must be nulled.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd",
        write::update("employees")
            .where_(filter::eq("id", 10_i64))
            .set(doc().set("id", 77_i64)),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mgr_ids = read_field_all(&resolver, "employees", "manager_id").await;
    assert!(
        mgr_ids.contains(&QueryValue::Null),
        "Sub.manager_id should be nulled, got: {mgr_ids:?}"
    );
    assert!(
        !mgr_ids.contains(&QueryValue::Int(10)),
        "stale reference to old id 10 must not survive, got: {mgr_ids:?}"
    );
}

/// Self-loop edge case: co-updated parent rows must NOT be incorrectly
/// re-matched as their own children.  Two roots are updated in one operation;
/// neither references the other's old value, so neither should be cascaded —
/// only the genuine child (which references one of the old values) is affected.
///
/// Hierarchy:
///   R1(id=10, manager_id=null)  — root, co-updated
///   R2(id=20, manager_id=null)  — root, co-updated (does NOT ref R1)
///   R3(id=30, manager_id=10)    — genuine child of R1, NOT co-updated
///
/// UPDATE employees SET id=99 WHERE id IN (10, 20):
///   • old values = {10, 20}
///   • R1.manager_id=null → NOT matched (co-updated parent, no ref) ✓
///   • R2.manager_id=null → NOT matched (co-updated parent, no ref) ✓
///   • R3.manager_id=10 → matched (genuine child of R1) → cascade to 99 ✓
#[tokio::test]
async fn self_ref_on_update_self_loop_overlapping_parent_and_child() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(&db, &registry, 9324, "self_ref_loop_upd", FkAction::Cascade);

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "R1")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "R2")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i3",
        "employees",
        doc()
            .set("id", 30_i64)
            .set("name", "R3")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key BOTH R1 (10→99) and R2 (20→99) in one operation.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd",
        write::update("employees")
            .where_(filter::in_("id", [10_i64, 20_i64]))
            .set(doc().set("id", 99_i64)),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let ids = read_field_all(&resolver, "employees", "id").await;
    let mgr_ids = read_field_all(&resolver, "employees", "manager_id").await;

    // 3 rows survive.
    assert_eq!(ids.len(), 3);

    // Exactly TWO rows now have id=99 (R1 and R2 were re-keyed).
    assert_eq!(
        ids.iter().filter(|v| **v == QueryValue::Int(99)).count(),
        2,
        "R1 and R2 should be re-keyed to 99, got ids: {ids:?}"
    );
    // R3 keeps id=30.
    assert!(
        ids.contains(&QueryValue::Int(30)),
        "R3 (not matched by WHERE) keeps id=30, got ids: {ids:?}"
    );

    // Only R3 (the genuine child of R1) should have manager_id=99. R1 and R2
    // (co-updated parents with manager_id=null) must NOT have been incorrectly
    // re-matched as children — their manager_id stays Null.
    assert_eq!(
        mgr_ids
            .iter()
            .filter(|v| **v == QueryValue::Int(99))
            .count(),
        1,
        "only R3 (genuine child) should be cascaded to 99, got: {mgr_ids:?}"
    );
    assert_eq!(
        mgr_ids.iter().filter(|v| **v == QueryValue::Null).count(),
        2,
        "R1 and R2 (co-updated parents) must keep manager_id=Null, got: {mgr_ids:?}"
    );
    assert!(
        !mgr_ids.contains(&QueryValue::Int(10)),
        "no stale ref to old id 10, got: {mgr_ids:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// LOST-UPDATE REGRESSION: cascade fan-out + direct UPDATE on the SAME child row
// in one transactional batch — both mutations must survive.
//
// Bug class: `execute_update_tx` scanned the COMMITTED store for matched rows
// (blind to this tx's own `write_set`) and merged its SET on top of that stale
// snapshot, then staged unconditionally via `update_tx_bytes` →
// `StagingStore::set` (a blind replace). A prior op in the SAME tx (the FK
// cascade fan-out) that already staged a DIFFERENT value for that exact row was
// silently overwritten — one of the two mutations vanished with no error.
//
// This test reproduces the EXACT overlap scenario: op (a) re-keys the parent,
// cascading child.parent_id 5→7; op (b) directly UPDATEs the SAME child row's
// `label`. Without the fix, op (b) merges on the committed snapshot
// (parent_id=5) and overwrites the cascade — child.parent_id reverts to 5.
// With the fix, op (b) merges on the tx-staged cascade result (parent_id=7) and
