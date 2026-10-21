//! F-40 (#848, P1) — FK footprint/isolation discovery must fail CLOSED.
//!
//! ## Background
//!
//! `require_footprint_if_fk_child` and `implicit_tx_isolation_for_fk_parent`
//! (`query_runner.rs`) are the two F-28 Step 5 (S3-C) hooks that make a
//! concurrent cross-transaction FK TOCTOU race observable to the SSI
//! machinery. Before F-40, both treated "I couldn't determine whether this
//! table is FK-relevant" (`resolve_repo` error or `FkReverseCache`
//! cache-build error) as "assume it isn't" — i.e. they fell back to the
//! PERMISSIVE behavior (skip the footprint requirement / return `Snapshot`
//! isolation). For a correctness-gating mechanism that is the wrong
//! direction: a discovery failure means we cannot prove the table is NOT
//! FK-relevant, so the safe assumption is that it IS.
//!
//! F-40 flips both hooks to fail CLOSED: on either discovery error,
//! `require_footprint_if_fk_child` widens the footprint UNCONDITIONALLY
//! (`tx.require_footprint_for(table_token)` anyway) and
//! `implicit_tx_isolation_for_fk_parent` returns the MORE protective
//! `Serializable` isolation.
//!
//! ## What these tests prove
//!
//! Each hook has two distinct error branches (resolve_repo failure +
//! cache-build failure), so the matrix is 2×2:
//!
//! 1. `require_footprint_if_fk_child` + resolve_repo error →
//!    `tx.footprint_tokens` DOES contain the table token (footprint widened
//!    anyway).
//! 2. `require_footprint_if_fk_child` + cache-build error → same.
//! 3. `implicit_tx_isolation_for_fk_parent` + resolve_repo error → returns
//!    `Serializable` for BOTH `FkParentOpKind::Delete` AND
//!    `FkParentOpKind::Update` (the op-kind dispatch sits strictly AFTER the
//!    cache warm, so both arms share the error paths).
//! 4. `implicit_tx_isolation_for_fk_parent` + cache-build error → same.
//!
//! ## The injection seam
//!
//! Neither `fk_race_closure_tests.rs`' `RaceInjectingResolver` (which
//! injects a concurrent WRITER at a `resolve_repo` call ordinal, not a
//! failure) nor `fk_reverse_cache_race_tests.rs`' build-closure-park
//! pattern (which exercises the cache's OWN invalidate-vs-build race, not a
//! resolver failure) is a reusable resolve_repo FAILURE injector. This file
//! adapts the counting-resolver SHAPE of `RaceInjectingResolver` (a
//! `resolve_repo` call-ordinal counter) but injects a `DbError` instead of
//! a writer — the same deterministic, no-sleeps, exact-program-point
//! handshake, applied to a different injected action.
//!
//! `FailureInjectingResolver` holds a real `DbInstance`-backed repo (so the
//! "succeed once" path returns a genuine `RepoInstance`, exercising the
//! real `FkReverseCache::get_or_build_by_parent` cold-miss path on the
//! cache-build-failure tests) and a `fail_at_or_after` ordinal:
//! - `ALWAYS_FAIL` (= 1): every `resolve_repo` call fails — exercises the
//!   hooks' direct `resolve_repo` error branch (the cache-build branch is
//!   never reached because the hooks short-circuit on the first call).
//! - `FAIL_AFTER_FIRST_OK` (= 2): the FIRST `resolve_repo` call returns
//!   `Ok(repo)` (so the hook proceeds to `get_or_build_by_parent`), and the
//!   SECOND call — the one inside `build_reverse_fk_entries`' closure, which
//!   is the first thing the cache-build path does — fails. That propagates
//!   out of `get_or_build_by_parent` as a cache-build error, exercising the
//!   hooks' second error branch.

use std::sync::atomic::{AtomicUsize, Ordering};

use shamir_tx::{IsolationLevel, TxContext, TxId};

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{
    implicit_tx_isolation_for_fk_parent, require_footprint_if_fk_child, FkParentOpKind,
    TableResolver,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};

/// `resolve_repo` always fails — every call (call #1, #2, …) returns `Err`.
/// Exercises the hooks' DIRECT `resolve_repo` error branch.
const ALWAYS_FAIL: usize = 1;

/// `resolve_repo` succeeds ONCE (call #1) then fails (call #2+). Exercises
/// the hooks' CACHE-BUILD error branch: call #1 is the hook's own direct
/// `resolve_repo`; call #2 is the one inside `build_reverse_fk_entries`'
/// closure (the first thing `get_or_build_by_parent`'s cold-miss path does),
/// so its `Err` propagates out as a cache-build failure.
const FAIL_AFTER_FIRST_OK: usize = 2;

/// Resolver that wraps a real `DbInstance`-backed repo and fails
/// `resolve_repo` for the Nth-or-later call (1-based), where N is
/// `fail_at_or_after`. Calls BEFORE N return the genuine `RepoInstance`
/// (so the cache-build-failure tests actually reach
/// `FkReverseCache::get_or_build_by_parent`'s cold-miss path before the
/// injected failure fires).
///
/// `resolve` is never reached by either hook's error branches (both
/// short-circuit on a `resolve_repo` failure before any per-table resolve),
/// but is implemented honestly against the backing `DbInstance` so the
/// resolver remains a faithful stand-in if a future test exercises a
/// different code path.
struct FailureInjectingResolver {
    db: DbInstance,
    repo: String,
    resolve_repo_calls: AtomicUsize,
    fail_at_or_after: usize,
}

#[async_trait::async_trait]
impl TableResolver for FailureInjectingResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        let n = self.resolve_repo_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= self.fail_at_or_after {
            return Err(DbError::NotFound(format!(
                "F-40 test: injected resolve_repo failure on call #{n} (fail_at_or_after={})",
                self.fail_at_or_after
            )));
        }
        self.db.get_repo(&self.repo).ok_or_else(|| {
            DbError::NotFound(format!(
                "F-40 test: backing repo '{}' not found on call #{n}",
                self.repo
            ))
        })
    }
}

/// Build a single-table `DbInstance`-backed resolver that fails
/// `resolve_repo` per `fail_at_or_after`. The table name (`"t"`) and repo
/// name (`"default"`) are fixed; tests reach the hooks via the returned
/// `TableRef` + a chosen `table_token`.
async fn setup_resolver(fail_at_or_after: usize) -> (FailureInjectingResolver, TableRef) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("t")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let resolver = FailureInjectingResolver {
        db,
        repo: "default".to_string(),
        resolve_repo_calls: AtomicUsize::new(0),
        fail_at_or_after,
    };
    let table_ref = TableRef::with_repo("default", "t");
    (resolver, table_ref)
}

/// A minimal `TxContext` for observing `require_footprint_for`'s side
/// effect. `Snapshot` isolation (the common case for an autocommit writer)
/// is the meaningful one to test: it is the case where the footprint token
/// is the ONLY thing that makes this tx's commit visible to a concurrent
/// Serializable FK-parent-delete's Phase 2-bis check.
fn fresh_snapshot_tx() -> TxContext {
    TxContext::new(
        TxId::new(1),
        /* repo_id */ 0,
        /* snapshot_version */ 0,
        IsolationLevel::Snapshot,
    )
}

const TABLE_TOKEN: u64 = 4242;

// ============================================================================
// 1. require_footprint_if_fk_child — resolve_repo error → fail CLOSED.
//
// The resolver's resolve_repo always fails (ALWAYS_FAIL), so the hook's
// DIRECT resolve_repo error branch fires on its very first line. The old
// (pre-F-40) behavior was `return` without touching the tx — leaving
// `footprint_tokens` empty. F-40 requires `tx.require_footprint_for` be
// called anyway. Asserted by observing `tx.footprint_tokens` post-call.
// ============================================================================

#[tokio::test]
async fn require_footprint_if_fk_child_fail_closes_on_resolve_repo_error() {
    let (resolver, table_ref) = setup_resolver(ALWAYS_FAIL).await;
    let mut tx = fresh_snapshot_tx();

    require_footprint_if_fk_child(&resolver, &table_ref, TABLE_TOKEN, &mut tx).await;

    assert!(
        tx.footprint_tokens.contains(&TABLE_TOKEN),
        "F-40 fail-closed: require_footprint_if_fk_child MUST call \
         require_footprint_for({TABLE_TOKEN}) on a resolve_repo error (widening the footprint \
         unconditionally), instead of the old permissive `return` that left footprint_tokens empty. \
         Actual footprint_tokens: {:?}",
        tx.footprint_tokens
    );
}

// ============================================================================
// 2. require_footprint_if_fk_child — cache-build error → fail CLOSED.
//
// The resolver's FIRST resolve_repo call returns Ok(repo) (so the hook
// proceeds to FkReverseCache::get_or_build_by_parent), and the SECOND call
// — inside build_reverse_fk_entries's closure — fails, propagating out of
// get_or_build_by_parent as a cache-build error. Same assertion as test 1.
// ============================================================================

#[tokio::test]
async fn require_footprint_if_fk_child_fail_closes_on_cache_build_error() {
    let (resolver, table_ref) = setup_resolver(FAIL_AFTER_FIRST_OK).await;
    let mut tx = fresh_snapshot_tx();

    require_footprint_if_fk_child(&resolver, &table_ref, TABLE_TOKEN, &mut tx).await;

    assert!(
        tx.footprint_tokens.contains(&TABLE_TOKEN),
        "F-40 fail-closed: require_footprint_if_fk_child MUST call \
         require_footprint_for({TABLE_TOKEN}) on a FkReverseCache cache-build error (widening the \
         footprint unconditionally), instead of the old permissive `return`. The resolver's first \
         resolve_repo succeeded (so the cache-build path was reached) and the second failed inside \
         build_reverse_fk_entries. Actual footprint_tokens: {:?}",
        tx.footprint_tokens
    );
    // Sanity: the resolver really did exercise the cache-build path (not the
    // direct-resolve_repo path) — at least 2 resolve_repo calls means the
    // first Ok'd and the build closure ran. Without this, the test would
    // pass trivially even if the cache-build branch were unreachable.
    assert!(
        resolver.resolve_repo_calls.load(Ordering::SeqCst) >= 2,
        "cache-build-failure test must have actually reached the build closure: expected >= 2 \
         resolve_repo calls (first Ok, second Err inside build_reverse_fk_entries), got {}",
        resolver.resolve_repo_calls.load(Ordering::SeqCst)
    );
}

// ============================================================================
// 3. implicit_tx_isolation_for_fk_parent — resolve_repo error → fail CLOSED,
//    for BOTH FkParentOpKind::Delete AND FkParentOpKind::Update.
//
// The op-kind dispatch (is_fk_parent_with_delete_action vs.
// is_fk_parent_with_update_action) sits strictly AFTER the cache warm, so
// both arms share the resolve_repo and cache-build error paths — both must
// return Serializable. A fresh resolver per op-kind keeps the call-count
// assertion independent.
// ============================================================================

#[tokio::test]
async fn implicit_tx_isolation_for_fk_parent_fail_closes_on_resolve_repo_error() {
    // Delete op-kind.
    {
        let (resolver, table_ref) = setup_resolver(ALWAYS_FAIL).await;
        let iso =
            implicit_tx_isolation_for_fk_parent(&resolver, &table_ref, FkParentOpKind::Delete)
                .await;
        assert_eq!(
            iso,
            IsolationLevel::Serializable,
            "F-40 fail-closed: implicit_tx_isolation_for_fk_parent MUST return Serializable (not \
             the old permissive Snapshot) on a resolve_repo error for FkParentOpKind::Delete"
        );
    }
    // Update op-kind — independent resolver so the call-count is fresh.
    {
        let (resolver, table_ref) = setup_resolver(ALWAYS_FAIL).await;
        let iso =
            implicit_tx_isolation_for_fk_parent(&resolver, &table_ref, FkParentOpKind::Update)
                .await;
        assert_eq!(
            iso,
            IsolationLevel::Serializable,
            "F-40 fail-closed: implicit_tx_isolation_for_fk_parent MUST return Serializable (not \
             the old permissive Snapshot) on a resolve_repo error for FkParentOpKind::Update"
        );
    }
}

// ============================================================================
// 4. implicit_tx_isolation_for_fk_parent — cache-build error → fail CLOSED,
//    for BOTH op kinds.
// ============================================================================

#[tokio::test]
async fn implicit_tx_isolation_for_fk_parent_fail_closes_on_cache_build_error() {
    // Delete op-kind.
    {
        let (resolver, table_ref) = setup_resolver(FAIL_AFTER_FIRST_OK).await;
        let iso =
            implicit_tx_isolation_for_fk_parent(&resolver, &table_ref, FkParentOpKind::Delete)
                .await;
        assert_eq!(
            iso,
            IsolationLevel::Serializable,
            "F-40 fail-closed: implicit_tx_isolation_for_fk_parent MUST return Serializable (not \
             the old permissive Snapshot) on a FkReverseCache cache-build error for \
             FkParentOpKind::Delete"
        );
        assert!(
            resolver.resolve_repo_calls.load(Ordering::SeqCst) >= 2,
            "cache-build-failure test must have actually reached the build closure: expected >= 2 \
             resolve_repo calls, got {}",
            resolver.resolve_repo_calls.load(Ordering::SeqCst)
        );
    }
    // Update op-kind — independent resolver.
    {
        let (resolver, table_ref) = setup_resolver(FAIL_AFTER_FIRST_OK).await;
        let iso =
            implicit_tx_isolation_for_fk_parent(&resolver, &table_ref, FkParentOpKind::Update)
                .await;
        assert_eq!(
            iso,
            IsolationLevel::Serializable,
            "F-40 fail-closed: implicit_tx_isolation_for_fk_parent MUST return Serializable (not \
             the old permissive Snapshot) on a FkReverseCache cache-build error for \
             FkParentOpKind::Update"
        );
        assert!(
            resolver.resolve_repo_calls.load(Ordering::SeqCst) >= 2,
            "cache-build-failure test must have actually reached the build closure: expected >= 2 \
             resolve_repo calls, got {}",
            resolver.resolve_repo_calls.load(Ordering::SeqCst)
        );
    }
}
