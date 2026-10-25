use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

use super::support::{
    bind_fk_validator, count_rows, insert_row, read_field_all, read_first_field, FkTestResolver,
};
use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use crate::validator::ValidatorRegistry;

// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_and_direct_update_on_same_child_row_both_survive() {
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
        9325,
        "child_fk_cascade_overlap",
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

    // parent(id=5); child(cid=50, parent_id=5, label="original").
    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).unwrap().set("name", "P5").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc()
            .set("cid", 50)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "original")
            .unwrap(),
    )
    .await;

    // Single TRANSACTIONAL batch — both ops share one TxContext so the cascade
    // fan-out (op a) stages into the SAME write_set that op (b)'s
    // `execute_update_tx` must merge over.
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    // (a) Re-key parent 5→7 → cascade child.parent_id 5→7 (staged in tx).
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    // (b) Direct UPDATE on the SAME child row, setting a DIFFERENT field.
    //    WHERE matches on `cid` (unchanged by the cascade) so the match is
    //    unambiguous regardless of read-your-own-writes.
    b.try_update(
        "upd_child",
        write::update("child")
            .where_(filter::eq("cid", 50))
            .set(doc().set("label", "direct").unwrap()),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));
    assert!(resp.results.contains_key("upd_child"));

    // BOTH mutations must survive on the child row:
    //  - parent_id == 7  (the cascade fan-out was NOT overwritten)
    //  - label    == "direct"  (the direct UPDATE was applied on top)
    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(7)),
        "cascade fan-out (parent_id 5→7) must survive the direct UPDATE"
    );
    assert_eq!(
        read_first_field(&resolver, "child", "label").await,
        Some(QueryValue::Str("direct".to_string())),
        "direct UPDATE (label→\"direct\") must be applied on top of the cascade"
    );
    assert_eq!(count_rows(&resolver, "child").await, 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// REMOVED-ROW SEMANTICS: DELETE then UPDATE the same row in one transactional
// batch — the row must stay deleted (UPDATE is a no-op on a tx-staged-removed
// row, never resurrecting it).
//
// Same bug class as above: `execute_update_tx` scanned the committed store
// (where the row still exists) and, without the staging probe, would merge +
// stage a Set over the staged Remove — silently resurrecting the deleted row.
// The fix probes staging, sees the staged Remove, and treats the row as
// NOT MATCHED (skip entirely).
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn delete_then_update_same_row_in_one_tx_row_stays_deleted() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("items")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // items(id=5, name="original").
    insert_row(
        &resolver,
        "ii",
        "items",
        doc().set("id", 5).unwrap().set("name", "original").unwrap(),
    )
    .await;
    assert_eq!(count_rows(&resolver, "items").await, 1);

    // Single TRANSACTIONAL batch: DELETE then UPDATE the same row.
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.try_delete("del", write::delete("items").where_(filter::eq("id", 5)))
        .unwrap();
    b.try_update(
        "upd",
        write::update("items")
            .where_(filter::eq("id", 5))
            .set(doc().set("name", "resurrected").unwrap()),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("del"));
    assert!(resp.results.contains_key("upd"));

    // The row must stay deleted — the UPDATE must NOT resurrect it.
    assert_eq!(
        count_rows(&resolver, "items").await,
        0,
        "DELETE then UPDATE same row in one tx: row must stay deleted"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-53c — index fast-path vs scan-fallback equivalence for ON UPDATE
// CASCADE / SET NULL.
//
// `fk_on_update.rs::index_candidate_ids` (the F-53c index fast-path) and the
// `list_stream_tx` scan fallback MUST produce byte-for-byte identical results.
// These tests run the SAME scenario WITH a supporting index on the child FK
// column (so the F-53c fast-path engages — the fresh autocommit tx has no
// staged child writes) and assert the fan-out matches the no-index scan path
// exercised by the existing tests above (`on_update_cascade_rekeys_multiple_*`
// / `on_update_set_null_nulls_child_fk`).
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_cascade_with_index_fast_path_rekeys_same_children() {
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
        9330,
        "child_fk_cascade_upd_idx",
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

    // Two parents (id=5, id=6), three children (two referencing 5, one 6) —
    // same shape as `on_update_cascade_rekeys_multiple_children_and_rows`.
    insert_row(
        &resolver,
        "ip1",
        "parent",
        doc().set("id", 5).unwrap().set("name", "P5").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ip2",
        "parent",
        doc().set("id", 6).unwrap().set("name", "P6").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic1",
        "child",
        doc()
            .set("cid", 1)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "a")
            .unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic2",
        "child",
        doc()
            .set("cid", 2)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "b")
            .unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic3",
        "child",
        doc()
            .set("cid", 3)
            .unwrap()
            .set("parent_id", 6)
            .unwrap()
            .set("label", "c")
            .unwrap(),
    )
    .await;

    // Supporting index on the FK column → F-53c fast-path engages.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id", &["parent_id"])
        .await
        .unwrap();

    // Re-key BOTH parents (5,6 → 99) via a where that matches both.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parents",
        write::update("parent")
            .where_(filter::in_("id", [5, 6]))
            .set(doc().set("id", 99).unwrap()),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // IDENTICAL to the no-index scan path: every child re-keyed to 99.
    let parent_ids = read_field_all(&resolver, "child", "parent_id").await;
    assert_eq!(parent_ids.len(), 3);
    assert!(
        parent_ids.iter().all(|v| *v == QueryValue::Int(99)),
        "expected all children re-keyed to 99 via the index fast-path, got: {parent_ids:?}"
    );
}

#[tokio::test]
async fn on_update_set_null_with_index_fast_path_nulls_same_children() {
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
        9331,
        "child_fk_setnull_upd_idx",
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

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).unwrap().set("name", "P5").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic1",
        "child",
        doc()
            .set("cid", 1)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "a")
            .unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic2",
        "child",
        doc()
            .set("cid", 2)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "b")
            .unwrap(),
    )
    .await;

    // Supporting index on the FK column → F-53c fast-path engages.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id", &["parent_id"])
        .await
        .unwrap();

    // Re-key parent 5 → 7: both children's parent_id must be SET to Null.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let parent_ids = read_field_all(&resolver, "child", "parent_id").await;
    assert_eq!(parent_ids.len(), 2);
    assert!(
        parent_ids.iter().all(|v| *v == QueryValue::Null),
        "expected both children nulled via the index fast-path, got: {parent_ids:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Defect 1 (fan-out collapse) — ON UPDATE RESTRICT's per-child-field scan
// must still catch a match on ANY of several distinct old parent values, not
// just the first/last one iterated. The RESTRICT branch of
// `plan_fk_on_update` used to call `child_has_reference` once PER
// (old-value, child_field) pair collected from `collect_parent_values`
// (itself un-deduped, pushing one entry per matched parent row); the fix
// groups by child_field and dedupes old values into a `TFxSet` before
// calling `any_child_references` once per field. This test updates THREE
// parent rows in one batch where only the MIDDLE one (by insertion order)
// still has a referencing child.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_restrict_blocks_on_middle_value_among_several_deduped_parents() {
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
        9340,
        "child_fk_restrict_update_dedup",
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

    // Three parent rows, none sharing a ref_field value.
    for id in [10_i64, 20, 30] {
        insert_row(
            &resolver,
            "ip",
            "parent",
            doc()
                .set("id", id)
                .unwrap()
                .set("name", format!("P{id}"))
                .unwrap(),
        )
        .await;
    }

    // TWO children both reference the MIDDLE parent value (20) — proves
    // multiple child rows referencing the same deduped old value are also
    // correctly detected. Parents 10 and 30 have no referencing children.
    insert_row(
        &resolver,
        "ic1",
        "child",
        doc()
            .set("cid", 200)
            .unwrap()
            .set("parent_id", 20)
            .unwrap()
            .set("label", "c200a")
            .unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic2",
        "child",
        doc()
            .set("cid", 201)
            .unwrap()
            .set("parent_id", 20)
            .unwrap()
            .set("label", "c200b")
            .unwrap(),
    )
    .await;

    // Update all three parents' "id" in ONE batch op (three distinct old
    // ref_field values, one probed field) → must be rejected because of the
    // still-referenced middle value (20).
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parents",
        write::update("parent")
            .where_(filter::in_("id", [10_i64, 20, 30]))
            .set(doc().set("id", 999_i64).unwrap()),
    )
    .unwrap();
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "expected fk_restrict error for the still-referenced middle \
                 value, got: {msg}"
            );
        }
        Ok(_) => panic!(
            "expected fk_restrict rejection (parent 20 still has 2 \
             referencing children) — got success, which means the deduped \
             per-child-field scan silently missed a non-first/last old value"
        ),
    }

    // All three parents unchanged (rollback) — a partial-application bug
    // would leave 10/30 re-keyed to 999 while 20 stays put; none of the
    // three should have moved.
    let mut ids = read_field_all(&resolver, "parent", "id").await;
    ids.sort_by_key(|v| match v {
        QueryValue::Int(i) => *i,
        _ => panic!("expected Int id, got {v:?}"),
    });
    assert_eq!(
        ids,
        vec![
            QueryValue::Int(10),
            QueryValue::Int(20),
            QueryValue::Int(30)
        ],
        "no partial application: all three parents must retain their \
         original ids after the rejected update"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// #1181 — ON UPDATE FK discovery routed through `fk_reverse_cache` behind the
// intersection gate.
//
// `discover_on_update_refs` used to do `repo.list_table_names()` +
// `resolver.resolve(..)` + `collect_fk_refs()` per table on EVERY UPDATE,
// BEFORE the set-fields ∩ ref-fields no-op gate — an update touching no
// FK-referenced field still resolved every child table in the repo. The fix
// routes discovery through the repo's cached reverse-FK map
// (`RepoInstance::fk_reverse_cache`, the SAME cache the DELETE path's
// `discover_action_refs`/`discover_restrict_refs` already use) and applies
// the set-fields membership check as the FIRST predicate, before the
// `on_update` check or any `OnUpdateRef` allocation.
//
// `ResolveCountingResolver` proves this behaviorally: it counts calls to
// `resolve()` for a specific table name, independent of what the production
// code does internally — a spy at the `TableResolver` trait boundary, not an
// assertion on private implementation details.
// ═══════════════════════════════════════════════════════════════════════════════

/// Wraps [`FkTestResolver`], counting `resolve()` calls for one watched table
/// name. Used to prove the ON UPDATE no-op gate (#1181) does not re-resolve
/// (and hence does not re-walk the schema of) a child table once the
/// reverse-FK cache is warm — the old O(tables) `discover_on_update_refs`
/// resolved every child table on EVERY update, no-op or not.
struct ResolveCountingResolver {
    inner: FkTestResolver,
    watched_table: String,
    resolve_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TableResolver for ResolveCountingResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        if table_ref.table == self.watched_table {
            self.resolve_count.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.resolve(table_ref).await
    }

    async fn resolve_repo(
        &self,
        repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        self.inner.resolve_repo(repo_name).await
    }
}

/// Proves the fast/no-op path: once the reverse-FK cache is warm, a
/// subsequent UPDATE whose `op.set` touches NO FK-referenced field never
/// resolves the child table again. Under the pre-#1181 code,
/// `discover_on_update_refs` unconditionally resolved every table returned by
/// `repo.list_table_names()` — including `child` — on every single UPDATE,
/// regardless of the later set-fields ∩ ref-fields gate; this test's
/// `resolve_count` would keep climbing on every no-op iteration under that
/// code. Under the fixed code, `discover_on_update_refs` reads the cached
/// `Arc<[ReverseFkEntry]>` (a warm-cache pointer clone, no table resolution at
/// all) and the set-fields membership check rejects every entry before the
/// `on_update` check, so `child` is never touched again once the cache is
/// warm.
#[tokio::test]
async fn on_update_noop_after_cache_warm_skips_child_table_resolve() {
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
        9341,
        "child_fk_cascade_resolve_spy",
        "parent_id",
        "parent",
        "id",
        FkAction::Cascade,
        true,
    );

    let inner = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &inner,
        "ip",
        "parent",
        doc().set("id", 5).unwrap().set("name", "P5").unwrap(),
    )
    .await;
    insert_row(
        &inner,
        "ic",
        "child",
        doc()
            .set("cid", 50)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "c50")
            .unwrap(),
    )
    .await;

    let resolve_count = Arc::new(AtomicUsize::new(0));
    let resolver = ResolveCountingResolver {
        inner,
        watched_table: "child".to_string(),
        resolve_count: Arc::clone(&resolve_count),
    };

    // Warm the reverse-FK cache: an UPDATE that DOES touch the ref_field
    // triggers real discovery (a cold-cache build) and the actual CASCADE
    // fan-out, which resolves `child` at least once.
    let mut warm = Batch::new();
    warm.id(1);
    warm.try_update(
        "warm",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 6).unwrap()),
    )
    .unwrap();
    execute_batch(&warm.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let count_after_warm = resolve_count.load(Ordering::SeqCst);
    assert!(
        count_after_warm > 0,
        "the warming UPDATE (which touches the ref_field and must cascade) \
         should have resolved 'child' at least once"
    );

    // Several NO-OP updates in a row (touching `name`, never `id`) — with the
    // cache warm, NONE of these may resolve `child` again.
    for i in 0..5u32 {
        let mut b = Batch::new();
        b.id(2 + i);
        b.try_update(
            "noop",
            write::update("parent")
                .where_(filter::eq("id", 6))
                .set(doc().set("name", format!("renamed-{i}")).unwrap()),
        )
        .unwrap();
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    }

    assert_eq!(
        resolve_count.load(Ordering::SeqCst),
        count_after_warm,
        "#1181: a no-op UPDATE (touching no FK-referenced field) must not \
         resolve the child table at all once the reverse-FK cache is warm — \
         the pre-fix O(tables) `discover_on_update_refs` schema walk resolved \
         every child table on EVERY update regardless of the no-op gate"
    );
}

/// Proves correctness is preserved through the cache path even when the
/// cache was warmed by a PRECEDING no-op update (not just a cold-cache first
/// call) — the set-fields filter is applied fresh per call against the
/// cached (unfiltered) `Arc<[ReverseFkEntry]>`, so an earlier no-op call
/// (whose `set_fields` happened not to intersect any ref_field) cannot poison
/// the cache into wrongly suppressing a LATER genuine CASCADE.
#[tokio::test]
async fn on_update_cascade_fires_correctly_after_cache_warmed_by_prior_noop() {
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
        9342,
        "child_fk_cascade_warm_then_real",
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

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).unwrap().set("name", "P5").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc()
            .set("cid", 50)
            .unwrap()
            .set("parent_id", 5)
            .unwrap()
            .set("label", "c50")
            .unwrap(),
    )
    .await;

    // Warm the cache with a NO-OP update first (touches `name`, not the
    // ref_field `id`) — this populates `fk_reverse_cache` via
    // `discover_on_update_refs`'s cache-routed build but must not fan out.
    let mut warm = Batch::new();
    warm.id(1);
    warm.try_update(
        "warm",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("name", "still-P5").unwrap()),
    )
    .unwrap();
    execute_batch(&warm.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(5)),
        "the warming no-op update must not have re-keyed the child"
    );

    // NOW the genuine CASCADE update: id 5 → 7. Must still fire correctly
    // through the now-warm cache.
    let mut b = Batch::new();
    b.id(2);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(
        read_first_field(&resolver, "parent", "id").await,
        Some(QueryValue::Int(7))
    );
    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(7)),
        "CASCADE must still fire correctly through the cache path even when \
         the cache was warmed by a preceding no-op update"
    );
}
