//! F-55 (#881, P0) — fail-closed FK reverse-cache discovery + read-error
//! propagation.
//!
//! `build_reverse_fk_entries` (`fk_reverse_cache.rs`) and
//! `discover_on_update_refs` (`fk_on_update.rs`) both ran a repo-wide FK
//! discovery scan that resolved every table in the repo. A `resolve` failure
//! on ANY single child table — a transient storage hiccup — was silently
//! swallowed (`Err(_) => continue`), so the scan "succeeded" with that table
//! absent from the FK graph. The incomplete result was then CAS-published as
//! a warm cache entry, allowing a subsequent parent UPDATE/DELETE to violate
//! referential integrity by never discovering the missed child as a
//! dependent.
//!
//! These fault-injection tests prove both discovery functions now fail the
//! WHOLE scan (return `Err`) on any per-table `resolve` failure instead of
//! continuing with a partial list, and that `FkReverseCache::
//! get_or_build_by_parent` does NOT publish any cache state when the build
//! closure returns `Err`.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_types::admin::FkAction;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::fk_on_update::discover_on_update_refs;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::build_reverse_fk_entries;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{FkReverseCache, RepoConfig, RepoInstance, ReverseFkEntry, TaggedReverseFkEntry};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};

// ── Test resolver ───────────────────────────────────────────────────────────

/// Resolver that wraps a real `DbInstance`-backed repo and fails `resolve`
/// for exactly one table name (`poison_table`). All other resolves — and all
/// `resolve_repo` calls — succeed against the backing instance, so the
/// discovery scan reaches the poisoned table naturally (it appears in
/// `list_table_names()`) and the failure fires exactly where the bug used to
/// swallow it.
struct PoisoningResolver {
    db: DbInstance,
    repo: String,
    poison_table: String,
}

#[async_trait::async_trait]
impl TableResolver for PoisoningResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        if table_ref.table == self.poison_table {
            return Err(DbError::Storage(format!(
                "F-55 injected resolve failure for table '{}'",
                self.poison_table
            )));
        }
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db
            .get_repo(&self.repo)
            .ok_or_else(|| DbError::NotFound(format!("repo '{}' not found", self.repo)))
    }
}

/// Build a `DbInstance`-backed resolver with `parent` + `child` tables where
/// `resolve("child")` always fails. `resolve_repo` and `resolve("parent")`
/// succeed against the real backing instance.
async fn setup_poison_resolver() -> PoisoningResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    PoisoningResolver {
        db,
        repo: "default".to_string(),
        poison_table: "child".to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. build_reverse_fk_entries — must return Err (not a partial Ok list) when
//    any per-table resolve fails mid-scan.
//
// Before the fix: `Err(_) => continue` silently dropped the poisoned table
// from the FK graph → returns Ok(partial_or_empty). After the fix: the `?`
// propagates the resolve error → returns Err.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn build_reverse_fk_entries_returns_err_on_resolve_failure() {
    let resolver = setup_poison_resolver().await;

    let result = build_reverse_fk_entries(&resolver, "default").await;

    assert!(
        result.is_err(),
        "build_reverse_fk_entries must fail the WHOLE scan (return Err) when any \
         per-table resolve fails — not silently continue with a partial list. \
         Got: {result:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. discover_on_update_refs — must return Err (a BatchError, not a partial
//    Ok list) when any per-table resolve fails mid-scan.
//
// Same swallow-and-continue bug as test 1, duplicated (uncached) in the
// on-update fan-out discovery path. Before the fix: the poisoned table was
// silently skipped → returns Ok(partial_or_empty). After the fix: the
// resolve error is mapped to BatchError::QueryError → returns Err.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discover_on_update_refs_returns_err_on_resolve_failure() {
    let resolver = setup_poison_resolver().await;
    let parent_ref = TableRef::with_repo("default", "parent");
    // Empty set_fields: the resolve failure happens inside the cache's build
    // closure (`build_reverse_fk_entries`), before any set-fields filtering,
    // so its content is irrelevant to this test.
    let set_fields: shamir_collections::TFxMap<String, shamir_types::types::value::QueryValue> =
        shamir_collections::TFxMap::default();

    let result = discover_on_update_refs(&resolver, &parent_ref, &set_fields).await;

    assert!(
        result.is_err(),
        "discover_on_update_refs must fail the WHOLE scan (return Err BatchError) \
         when any per-table resolve fails — not silently continue with a partial \
         list."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. get_or_build_by_parent — must return Err AND not publish any cache state
//    (the ArcSwap generation must not advance) when the build closure fails.
//
// This is the actual user-visible fail-closed guarantee: an error from
// build_reverse_fk_entries must propagate through get_or_build_by_parent's
// `build().await?` (line 345), never reaching try_publish's CAS. Verified
// behaviorally: after the failed call, a subsequent call with a SUCCEEDING
// build closure must actually run the build (proving the cache is still cold
// — if the failed build had published, the subsequent call would be a warm
// hit and skip the build entirely).
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn get_or_build_does_not_publish_on_build_error() {
    let cache = FkReverseCache::new();
    let resolver = setup_poison_resolver().await;

    // First call: the build closure calls build_reverse_fk_entries with the
    // poison resolver → resolve("child") fails → must propagate Err out of
    // get_or_build_by_parent WITHOUT publishing.
    let result = cache
        .get_or_build_by_parent("parent", || {
            let resolver = &resolver;
            async move { build_reverse_fk_entries(resolver, "default").await }
        })
        .await;

    assert!(
        result.is_err(),
        "get_or_build_by_parent must return Err when the build closure fails — \
         the resolve error must propagate via `build().await?` before any CAS \
         publish. Got: {result:?}"
    );

    // The cache must NOT have published anything from the failed build.
    // Prove it: a subsequent call with a SUCCEEDING build closure must
    // actually invoke the build (warm-hit would skip it). If the buggy code
    // had published an incomplete cache from the swallowed-error scan, this
    // counter would stay 0.
    let build_calls = Arc::new(AtomicUsize::new(0));
    let fresh_entry = TaggedReverseFkEntry {
        parent_table: "parent".to_string(),
        entry: ReverseFkEntry {
            child_table: "child".to_string(),
            child_field: "parent_id".to_string(),
            parent_ref_field: "id".to_string(),
            on_delete: FkAction::NoAction,
            on_update: FkAction::NoAction,
        },
    };
    let result2 = cache
        .get_or_build_by_parent("parent", || {
            let build_calls = Arc::clone(&build_calls);
            let fresh_entry = fresh_entry.clone();
            async move {
                build_calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(vec![fresh_entry])
            }
        })
        .await;

    assert!(
        result2.is_ok(),
        "a subsequent call with a succeeding build closure must return Ok. \
         Got: {result2:?}"
    );
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        1,
        "the cache must be COLD after a failed build (no publish) — the \
         subsequent call must run a real build, not serve a stale warm hit \
         left behind by the failed scan"
    );
}
