use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

use super::support::FkTestResolver;
use super::support::{
    assert_cascade_scenario_result, bind_fk_validator, build_cascade_index_scenario, count_rows,
    insert_helper, read_all_field, read_first_field,
};
use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::validator::ValidatorRegistry;

#[tokio::test]
async fn self_referential_set_null_nulls_direct_subordinates() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("employees")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // employees.manager_id → employees.id ON DELETE SET NULL (self-ref).
    bind_fk_validator(
        &db,
        &registry,
        "employees",
        "self_ref_setnull",
        "manager_id",
        "employees",
        "id",
        FkAction::SetNull,
        true, // manager_id is nullable
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Build a 3-level hierarchy: CEO(1) ← Mgr(2) ← Worker(3).
    insert_helper(
        &resolver,
        "employees",
        doc()
            .set("id", 1_i64)
            .unwrap()
            .set("name", "CEO")
            .unwrap()
            .set("manager_id", QueryValue::Null)
            .unwrap(),
    )
    .await;
    insert_helper(
        &resolver,
        "employees",
        doc()
            .set("id", 2_i64)
            .unwrap()
            .set("name", "Mgr")
            .unwrap()
            .set("manager_id", 1_i64)
            .unwrap(),
    )
    .await;
    insert_helper(
        &resolver,
        "employees",
        doc()
            .set("id", 3_i64)
            .unwrap()
            .set("name", "Worker")
            .unwrap()
            .set("manager_id", 2_i64)
            .unwrap(),
    )
    .await;

    assert_eq!(count_rows(&resolver, "employees").await, 3);

    // Delete CEO (id=1) → Mgr's manager_id must be nulled (direct subordinate).
    // Worker's manager_id must be UNCHANGED (single-level: Worker is a
    // grandchild, not a direct child of CEO).
    let mut b = Batch::new();
    b.id(4);
    b.try_delete(
        "del_ceo",
        write::delete("employees").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("del_ceo"));

    // 2 rows survive (Mgr + Worker); CEO is gone.
    assert_eq!(count_rows(&resolver, "employees").await, 2);

    // Read all manager_id values to check Mgr was nulled but Worker untouched.
    let manager_ids = read_all_field(&resolver, "employees", "manager_id").await;
    // Exactly one Null (Mgr's manager_id) and one Int(2) (Worker's manager_id).
    assert!(
        manager_ids.contains(&QueryValue::Null),
        "Mgr's manager_id should be nulled, got: {manager_ids:?}"
    );
    assert!(
        manager_ids.contains(&QueryValue::Int(2)),
        "Worker's manager_id should be untouched (single-level), got: {manager_ids:?}"
    );
    assert!(
        !manager_ids.contains(&QueryValue::Int(1)),
        "no dangling reference to deleted CEO id=1 should survive, got: {manager_ids:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2 (D1) — transactional [insert child; delete parent] under CASCADE
// must NOT leave an orphan: the cascade plan must see the child row staged
// EARLIER in the same tx (read-your-own-writes) and include it in the
// cascade, deleting it alongside the parent.
//
// Before F-28 Step 2, `plan_cascade`'s row-level discovery
// (`collect_parent_values` / the child scan in `plan_cascade_recursive`) read
// committed-only state, so a child row inserted earlier in the SAME
// transaction was invisible to the cascade plan — the parent delete would
// commit, but the newly-inserted child would silently survive as an orphan
// (still referencing a now-deleted parent id).
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_insert_child_then_delete_parent_cascades_no_orphan() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_cascade_tx",
        "parent_id",
        "parent",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Parent already exists (committed, autocommit).
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).unwrap().set("name", "P").unwrap(),
    )
    .await;
    assert_eq!(count_rows(&resolver, "parent").await, 1);
    assert_eq!(count_rows(&resolver, "child").await, 0);

    // ONE transactional batch: insert a child referencing the parent, THEN
    // delete the parent. The cascade plan (triggered by the delete) must see
    // the child inserted EARLIER in this same tx and cascade-delete it too —
    // otherwise it survives, orphaned, referencing a deleted parent id.
    let mut b = Batch::new();
    b.id(2);
    b.transactional();
    b.insert(
        "ins_child",
        write::insert("child").row(
            doc()
                .set("cid", 10_i64)
                .unwrap()
                .set("parent_id", 1_i64)
                .unwrap()
                .set("label", "c1")
                .unwrap(),
        ),
    );
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp.transaction.expect("transaction info present");
    assert!(
        info.is_committed(),
        "transactional [insert child; delete parent] under CASCADE must commit, got: {info:?}"
    );

    // Parent gone.
    assert_eq!(count_rows(&resolver, "parent").await, 0);
    // Child must ALSO be gone — cascaded, not left as an orphan.
    assert_eq!(
        count_rows(&resolver, "child").await,
        0,
        "child inserted earlier in the SAME tx must be visible to the cascade \
         plan and cascade-deleted alongside the parent — no orphan should survive"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2 (D1) — a child row staged as an UPDATE (re-keying its FK value
// to point elsewhere) EARLIER in the same tx must be reflected by the cascade
// plan using the UPDATED reference, not the stale committed one.
//
// Setup: two parents (1, 2), one child initially referencing parent 1. In one
// transactional batch: (a) re-key the child's FK to point at parent 2
// instead, then (b) delete parent 1. Since the plan must see the STAGED
// update (not the stale committed `parent_id = 1`), the child must NOT be
// cascade-deleted by parent 1's delete (it no longer references parent 1 by
// the time the plan runs) — it must survive, still pointing at parent 2.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_update_then_delete_parent_cascade_uses_updated_reference() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_cascade_restage",
        "parent_id",
        "parent",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Two parents; the child initially references parent 1.
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).unwrap().set("name", "P1").unwrap(),
    )
    .await;
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 2_i64).unwrap().set("name", "P2").unwrap(),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .unwrap()
            .set("parent_id", 1_i64)
            .unwrap()
            .set("label", "c1")
            .unwrap(),
    )
    .await;

    // ONE transactional batch: re-key the child to reference parent 2, THEN
    // delete parent 1. The cascade plan for the parent-1 delete must use the
    // UPDATED (staged) reference — child now points at parent 2 — and so must
    // NOT cascade-delete the child.
    let mut b = Batch::new();
    b.id(3);
    b.transactional();
    b.try_update(
        "update_child_fk",
        write::update("child")
            .where_(filter::eq("parent_id", 1_i64))
            .set(doc().set("parent_id", 2_i64).unwrap()),
    )
    .unwrap();
    b.try_delete(
        "del_parent_1",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp.transaction.expect("transaction info present");
    assert!(
        info.is_committed(),
        "transactional [re-key child; delete parent 1] must commit, got: {info:?}"
    );

    // Parent 1 gone, parent 2 survives.
    assert_eq!(count_rows(&resolver, "parent").await, 1);
    // Child survives — it no longer referenced parent 1 by the time the
    // cascade plan ran (it was re-keyed to parent 2 earlier in the same tx).
    assert_eq!(
        count_rows(&resolver, "child").await,
        1,
        "child re-keyed to parent 2 earlier in the SAME tx must survive \
         parent 1's cascade-delete — the plan must reflect the UPDATED \
         reference, not the stale committed one"
    );
    let val = read_first_field(&resolver, "child", "parent_id").await;
    assert_eq!(
        val,
        Some(QueryValue::Int(2)),
        "child's parent_id must reflect the staged update (now 2), not the \
         stale committed value (1)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2, point 4 — staged-overlay fix for the RESTRICT index fast-path.
//
// A child row INSERTED (staged, not yet committed) referencing the parent,
// with an index covering the child FK field: `lookup_by_index` returns empty
// (indexing happens only at commit), so the RESTRICT gate's index fast-path
// must fall back to probing `tx.write_set` directly (mirroring
// `ValidatorDb::exists_in_table`'s staged-overlay pattern) rather than
// concluding "no reference" from the empty index alone.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_insert_child_with_index_then_delete_parent_restrict_sees_staged_insert() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_restrict_idx",
        "parent_id",
        "parent",
        "id",
        FkAction::Restrict,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Parent exists (committed).
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).unwrap().set("name", "P").unwrap(),
    )
    .await;

    // Build an index covering the child's FK field (`parent_id`) so the
    // RESTRICT gate's `child_has_reference` takes the index fast-path.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_parent_id", &["parent_id"])
        .await
        .expect("index creation should succeed");

    // ONE transactional batch: insert a child referencing the parent (staged,
    // not yet committed — never in the index), THEN try to delete the
    // parent. The RESTRICT gate must see the STAGED child via the
    // write_set overlay probe (the index alone returns empty) and reject.
    let mut b = Batch::new();
    b.id(2);
    b.transactional();
    b.insert(
        "ins_child",
        write::insert("child").row(
            doc()
                .set("parent_id", 1_i64)
                .unwrap()
                .set("label", "x")
                .unwrap(),
        ),
    );
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    // A transactional batch's mid-batch rejection surfaces as `Ok(response)`
    // with an ABORTED `TransactionInfo` (not an `Err`) — the batch executor
    // wraps every op error inside the tx into the abort reason. Mirrors how
    // `execute_batch_transactional_si_happy_path` reads `response.transaction`.
    match resp {
        Ok(r) => {
            let info = r
                .transaction
                .as_ref()
                .expect("transaction info present for a transactional batch");
            assert!(
                !info.is_committed(),
                "expected the tx to abort with fk_restrict (staged child insert \
                 should still block the parent delete via the index fast-path's \
                 staged-overlay fallback), but it committed: {info:?}"
            );
            let reason = info.reason.as_deref().unwrap_or("");
            assert!(
                reason.contains("fk_restrict"),
                "expected abort reason to contain 'fk_restrict', got: {reason}"
            );
        }
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "expected fk_restrict — staged (uncommitted, unindexed) child \
                 insert must still be visible to the RESTRICT gate via the \
                 write_set overlay probe, got: {msg}"
            );
        }
    }
}

#[tokio::test]
async fn cascade_no_index_scan_path_deletes_referencing_children() {
    // Scan-fallback path (no supporting index).
    let resolver = build_cascade_index_scenario(false).await;
    assert_eq!(count_rows(&resolver, "child").await, 6);

    let mut b = Batch::new();
    b.id(10);
    b.try_delete(
        "del",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_cascade_scenario_result(&resolver).await;
}

#[tokio::test]
async fn cascade_with_index_fast_path_deletes_same_children() {
    // F-53c index fast-path (supporting index, fresh autocommit tx → no
    // staged child writes → fast-path engages). Must produce the IDENTICAL
    // result to the no-index scan path above.
    let resolver = build_cascade_index_scenario(true).await;
    assert_eq!(count_rows(&resolver, "child").await, 6);

    let mut b = Batch::new();
    b.id(10);
    b.try_delete(
        "del",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_cascade_scenario_result(&resolver).await;
}

#[tokio::test]
async fn cascade_with_index_fast_path_falls_back_to_scan_for_staged_child_writes() {
    // With a supporting index BUT a staged child insert in the SAME tx, the
    // F-53c "no staged writes" gate MUST route to the tx-aware scan fallback
    // — otherwise the staged (unindexed) child would be orphaned by the
    // cascade (the index reflects committed state only). This is the CASCADE
    // analog of
    // `transactional_insert_child_with_index_then_delete_parent_restrict_sees_staged_insert`.
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_cascade_staged",
        "parent_id",
        "parent",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).unwrap().set("name", "P").unwrap(),
    )
    .await;
    // One COMMITTED child referencing parent 1.
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .unwrap()
            .set("parent_id", 1_i64)
            .unwrap(),
    )
    .await;

    // Supporting index → the fast-path WOULD engage, except the same-tx
    // staged insert below forces the scan fallback.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id", &["parent_id"])
        .await
        .unwrap();

    // ONE transactional batch: insert a SECOND child referencing parent 1
    // (staged, never in the index until commit), THEN delete parent 1. The
    // cascade must see BOTH children (the committed one + the staged one via
    // the scan overlay) and cascade-delete both.
    let mut b = Batch::new();
    b.id(20);
    b.transactional();
    b.insert(
        "ins_child2",
        write::insert("child").row(
            doc()
                .set("cid", 11_i64)
                .unwrap()
                .set("parent_id", 1_i64)
                .unwrap(),
        ),
    );
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp
        .transaction
        .as_ref()
        .expect("transaction info present for a transactional batch");
    assert!(
        info.is_committed(),
        "cascade tx should commit (Cascade, not Restrict); got: {info:?}"
    );

    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(
        count_rows(&resolver, "child").await,
        0,
        "the staged (unindexed) child insert must be cascaded too — the \
         F-53c 'no staged writes' gate must route to the tx-aware scan \
         fallback rather than trusting the committed-only index"
    );
}

#[tokio::test]
async fn set_null_with_index_fast_path_nulls_same_children() {
    // SET NULL via the F-53c index fast-path must match the existing
    // scan-path test `set_null_nulls_child_field_when_parent_deleted`
    // (which runs with no index). Two children reference parent 1; after
    // the delete both must survive with parent_id == Null.
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_setnull_idx",
        "parent_id",
        "parent",
        "id",
        FkAction::SetNull,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).unwrap().set("name", "P").unwrap(),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .unwrap()
            .set("parent_id", 1_i64)
            .unwrap()
            .set("label", "c1")
            .unwrap(),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 11_i64)
            .unwrap()
            .set("parent_id", 1_i64)
            .unwrap()
            .set("label", "c2")
            .unwrap(),
    )
    .await;

    // Supporting index on the FK column → F-53c fast-path engages.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id", &["parent_id"])
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(30);
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Parent gone; both children survive with parent_id == Null (same as the
    // scan-path SET NULL test).
    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(count_rows(&resolver, "child").await, 2);
    let nulled = read_all_field(&resolver, "child", "parent_id").await;
    assert!(
        nulled.iter().all(|v| *v == QueryValue::Null),
        "both children's parent_id must be Null after SET NULL, got: {nulled:?}"
    );
}
