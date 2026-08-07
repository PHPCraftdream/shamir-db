//! F-28 Step 4 (#831) — cached per-repo reverse-FK map tests.
//!
//! `discover_restrict_refs` (`fk_restrict.rs`) and `discover_action_refs`
//! (`fk_actions.rs`) used to run an O(tables) `collect_fk_refs()` scan on
//! EVERY delete/cascade-recursion-level. Both now read through
//! `RepoInstance::fk_reverse_cache()` (cache-aside: rebuild once on a miss,
//! then serve every subsequent lookup from the cached map until the next
//! invalidation).
//!
//! These tests cover:
//! 1. A NEW `foreign_key` schema rule is discovered/enforced immediately
//!    (the cache doesn't serve a stale "no such reference" from before the
//!    DDL that added it).
//! 2. REMOVING a schema rule that declared an FK stops it being
//!    discovered/enforced (the cache doesn't keep enforcing a dropped rule).
//! 3. Repeated deletes on the same parent table, with no intervening schema
//!    mutation, do NOT re-run the O(tables) discovery scan each time
//!    (observed via a call-counting resolver wrapper).
//!
//! Test 3 from the brief (rollback correctness) lives in
//! `shamir-db`'s `schema_rollback_tests.rs`, which already has the
//! `compile_table_schema` activation-failure fixtures this needs (F-24/F-27b)
//! — this crate has no direct access to `ShamirDb::compile_table_schema`
//! (it is `pub(crate)` to `shamir-db`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

// ── Test resolver (mirrors fk_restrict_tests / fk_actions_tests) ────────────

struct FkTestResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
    /// Counts every `resolve()` call — used as a proxy for "how many times
    /// did the O(tables) reverse-FK scan run". Each rebuild resolves every
    /// table in the repo exactly once via `build_reverse_fk_entries`.
    resolve_calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TableResolver for FkTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        let mut table = self.db.get_table(&self.repo, &table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }
}

/// Build parent + child tables with NO foreign_key bound yet — same shape as
/// `fk_restrict_tests::setup_fk_test`, minus the FK binding, so tests can
/// exercise DDL-driven schema mutation (add/remove) against a warm cache.
async fn setup_no_fk() -> FkTestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());
    FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
        resolve_calls: Arc::new(AtomicUsize::new(0)),
    }
}

/// Bind (or replace, via a fresh validator_id) a SchemaValidator with a
/// single `foreign_key` field on the child table.
fn bind_restrict_fk(resolver: &FkTestResolver, validator_id: RecordId) {
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["parent_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete(
                "parent",
                "id",
                FkAction::Restrict,
            )),
            required: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);

    resolver
        .registry
        .register(validator_id, "child_fk_schema", Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };

    let mut child_table =
        futures::executor::block_on(resolver.db.get_table("default", "child")).unwrap();
    child_table.set_validator_registry(Arc::clone(&resolver.registry));
    futures::executor::block_on(child_table.add_validator_binding(binding)).unwrap();
}

async fn insert_parent(resolver: &FkTestResolver, id: i64) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("parent").row(doc().set("id", id).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

async fn insert_child(resolver: &FkTestResolver, parent_id: i64) {
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins",
        write::insert("child").row(doc().set("parent_id", parent_id).set("label", "x")),
    );
    let req = b.build();
    execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

async fn try_delete_parent(resolver: &FkTestResolver, id: i64) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(3);
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", id)),
    )
    .unwrap();
    let req = b.build();
    match execute_batch(&req, resolver, None, None, Actor::System, "test").await {
        Ok(resp) => {
            if resp.results.contains_key("del_parent") {
                Ok(())
            } else {
                Err("del_parent missing from results".to_string())
            }
        }
        Err(e) => Err(format!("{e:?}")),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Cache correctness after ADDING a foreign_key schema rule: a subsequent
//    delete on the parent must discover and enforce the NEW reference, not
//    serve a stale "no such reference" cached from before the DDL.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cache_reflects_newly_added_fk_reference() {
    let resolver = setup_no_fk().await;

    insert_parent(&resolver, 1).await;

    // Before any FK is declared, deleting the parent must succeed (no
    // reverse-FK reference exists yet) — this ALSO warms the cache with an
    // "empty" reverse-FK map for this repo.
    let pre = try_delete_parent(&resolver, 1).await;
    assert!(
        pre.is_ok(),
        "delete must succeed before any FK is declared: {pre:?}"
    );

    // Re-insert the parent (it was just deleted) and add a child referencing
    // it, THEN declare the RESTRICT foreign_key — simulating a DDL schema
    // mutation happening after the cache was already warmed empty.
    insert_parent(&resolver, 2).await;
    insert_child(&resolver, 2).await;
    bind_restrict_fk(&resolver, RecordId::from_ts(9101));

    // The cache is NOT invalidated by directly mutating validator bindings
    // in this test harness (only `ShamirDb::compile_table_schema` calls
    // `invalidate()` in production) — so this exercises the DDL path's
    // actual production behavior: `add_table`/`remove_table` and
    // `compile_table_schema` are the invalidation hooks. To prove the cache
    // genuinely reflects live state rather than a hardcoded stale snapshot,
    // invalidate explicitly here (this is what `compile_table_schema` does
    // internally after a successful bind) and confirm the delete is now
    // rejected.
    resolver
        .db
        .get_repo(&resolver.repo)
        .unwrap()
        .fk_reverse_cache()
        .invalidate();

    let post = try_delete_parent(&resolver, 2).await;
    assert!(
        post.is_err(),
        "delete must be rejected once the FK reference is live: {post:?}"
    );
    assert!(
        post.unwrap_err().contains("fk_restrict"),
        "rejection must be fk_restrict"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Cache correctness after REMOVING a schema rule that declared an FK: the
//    reference must no longer be discovered/enforced once the binding is
//    gone and the cache invalidated.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cache_reflects_removed_fk_reference() {
    let resolver = setup_no_fk().await;
    let validator_id = RecordId::from_ts(9102);
    bind_restrict_fk(&resolver, validator_id);

    insert_parent(&resolver, 1).await;
    insert_child(&resolver, 1).await;

    // Warm the cache while the FK is still live: delete must be rejected.
    let with_fk = try_delete_parent(&resolver, 1).await;
    assert!(
        with_fk.is_err(),
        "delete must be rejected while FK is live: {with_fk:?}"
    );

    // Remove the binding (simulating an ALTER that drops the foreign_key
    // rule) and invalidate the cache — mirrors what
    // `ShamirDb::compile_table_schema` does on a successful ALTER.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .remove_validator_binding(&validator_id)
        .await
        .unwrap();
    resolver
        .db
        .get_repo(&resolver.repo)
        .unwrap()
        .fk_reverse_cache()
        .invalidate();

    // Delete the (still-referencing) child row too so this only tests the
    // FK-discovery gate, not RESTRICT's actual row-level match — the FK
    // binding is gone so RESTRICT should not even query further.
    let post = try_delete_parent(&resolver, 1).await;
    assert!(
        post.is_ok(),
        "delete must succeed once the FK rule is removed and cache invalidated: {post:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Cached-vs-uncached distinction: repeated deletes on the SAME parent
//    table, with NO intervening schema mutation, must NOT re-run the
//    O(tables) discovery scan each time. Proxy: the PER-DELETE DELTA in
//    `resolve()` call count. Every delete pays a fixed per-delete resolve
//    cost independent of caching (`check_fk_restrict` resolves the matched
//    ref's child table once to run the row-level probe) — so the signal is
//    not "zero resolves on a hit" but "strictly fewer resolves on a hit than
//    on the miss that preceded it", by exactly the discovery scan's
//    per-table resolve count that a hit skips.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn repeated_deletes_do_not_rerun_discovery_scan() {
    let resolver = setup_no_fk().await;
    bind_restrict_fk(&resolver, RecordId::from_ts(9103));

    // Seed 3 parent rows, each with a referencing child, so 3 independent
    // deletes can each hit the RESTRICT gate without running out of rows.
    for i in 1..=3 {
        insert_parent(&resolver, i).await;
        insert_child(&resolver, i).await;
    }

    // Baseline resolve-call count includes the setup inserts above (they
    // resolve "parent"/"child" via the write path, not the FK-discovery
    // scan) — snapshot it AFTER setup so only discovery-driven resolves are
    // measured from here on.
    resolver.resolve_calls.store(0, Ordering::SeqCst);

    // F-28 Step 5 (#832): the seed inserts above ALREADY warmed the cache —
    // `require_footprint_if_fk_child` (query_runner.rs's Insert `None` arm)
    // calls `FkReverseCache::get_or_build_by_parent` on every insert to
    // decide whether the inserted table is an FK child, so by the time the
    // very first `insert_child` ran, the cache was built. Force an explicit
    // invalidation here so delete #1 below still observes a genuine cache
    // MISS — otherwise all three deletes would see an already-warm cache and
    // this test's cache-miss-vs-cache-hit distinction would have nothing
    // left to measure. Mirrors the same explicit-`invalidate()` pattern
    // `cache_reflects_newly_added_fk_reference` (test 1, above) already uses
    // to force a miss on demand.
    resolver
        .db
        .get_repo(&resolver.repo)
        .unwrap()
        .fk_reverse_cache()
        .invalidate();

    // First delete: cache miss (forced above) → discovery's rebuild calls
    // `resolver.resolve()` once per table in the repo (2 tables: parent,
    // child) IN ADDITION to `check_fk_restrict`'s own per-matched-ref
    // child-table resolve (a fixed per-delete cost that also happens on a
    // cache HIT — NOT part of the discovery scan itself). The first delete's
    // resolve-count delta is therefore strictly larger than every subsequent
    // (cache-hit) delete's delta by exactly that discovery scan's table
    // count. Note: F-28 Step 5's `implicit_tx_isolation_for_fk_parent` (also
    // called from the Delete arm, before `check_fk_restrict`) pays this same
    // miss cost — whichever of the two call sites reaches the cache first
    // absorbs the O(tables) scan; the other sees an already-warm cache. The
    // total resolve count for the miss delete is unaffected either way.
    let before_first = resolver.resolve_calls.load(Ordering::SeqCst);
    let r1 = try_delete_parent(&resolver, 1).await;
    assert!(r1.is_err(), "first delete must be rejected: {r1:?}");
    let delta_first = resolver.resolve_calls.load(Ordering::SeqCst) - before_first;
    assert!(
        delta_first > 0,
        "first delete (cache miss) must resolve at least one table during discovery"
    );

    // Second delete: the cache is now warm (no invalidation happened in
    // between) — `discover_restrict_refs`/`discover_action_refs` must hit
    // the cache and skip the O(tables) scan, so this delete's resolve delta
    // must be STRICTLY SMALLER than the first (miss) delete's — the
    // difference is exactly the discovery scan's per-table resolve count
    // that a hit skips entirely.
    let before_second = resolver.resolve_calls.load(Ordering::SeqCst);
    let r2 = try_delete_parent(&resolver, 2).await;
    assert!(r2.is_err(), "second delete must be rejected: {r2:?}");
    let delta_second = resolver.resolve_calls.load(Ordering::SeqCst) - before_second;
    assert!(
        delta_second < delta_first,
        "second delete (cache hit) must resolve FEWER tables than the first \
         (cache miss) delete — delta_first={delta_first}, delta_second={delta_second}"
    );

    // Third delete: also a cache hit (still no intervening schema mutation)
    // — its delta must match the second delete's EXACTLY, proving the cache
    // serves a STABLE hit count rather than degrading/rebuilding piecemeal.
    let before_third = resolver.resolve_calls.load(Ordering::SeqCst);
    let r3 = try_delete_parent(&resolver, 3).await;
    assert!(r3.is_err(), "third delete must be rejected: {r3:?}");
    let delta_third = resolver.resolve_calls.load(Ordering::SeqCst) - before_third;
    assert_eq!(
        delta_third, delta_second,
        "third delete (cache hit) must resolve the SAME number of tables as \
         the second (cache hit) delete — no re-run of the O(tables) scan"
    );
}
