//! Shared repo-level DDL-admission helpers for admin wire handlers.
//!
//! Every admin handler that mutates index registries, table info/validator
//! stores, or repo stores DIRECTLY (bypassing the already-guarded
//! `RepoInstance` wrappers) must hold the repo-level `ddl_admission` mutex
//! (`RepoInstance::ddl_admission_guard`) for the entire mutation — exactly
//! once per path (the `tokio::sync::Mutex` is NOT reentrant). The follower
//! receipt apply pipeline takes the same mutex, so guarded DDL can never
//! interleave with a receipt apply.
//!
//! Deadlock discipline: a handler that holds the guard returned here must
//! only call guard-free inner variants (`ShamirDb::drop_table_inner`,
//! `drop_table_cleaning_validators_inner`) — never the public guarded
//! facade methods.

use crate::engine::table::TableManager;
use crate::query::batch::BatchError;
use crate::shamir_db::ShamirDb;

/// Resolve `(db, repo)` and acquire the repo-level `ddl_admission` guard
/// (`RepoInstance::ddl_admission_guard`).
///
/// Returns a `BatchError::QueryError` when the database or repository does
/// not exist (same shape as the inline guard blocks this helper replaces in
/// `admin_table_index.rs`).
pub(in crate::shamir_db::execute) async fn repo_ddl_guard(
    shamir: &ShamirDb,
    db_name: &str,
    repo_name: &str,
) -> Result<tokio::sync::OwnedMutexGuard<()>, BatchError> {
    let err = |msg: String| BatchError::QueryError {
        alias: String::new(),
        message: msg,
        code: None,
    };
    let repo = shamir
        .get_db(db_name)
        .ok_or_else(|| err(format!("Database '{}' not found", db_name)))?
        .get_repo(repo_name)
        .ok_or_else(|| err(format!("Repository '{}' not found", repo_name)))?;
    Ok(repo.ddl_admission_guard().await)
}

/// Begin the schema-activation write barrier for `table`: acquire the
/// repo-level `ddl_admission` guard FIRST (S5c round 3 — schema DDL rewrites
/// table info/validator stores directly and must not interleave with a
/// follower receipt apply), then resolve the table handle and acquire the
/// per-table barrier via
/// [`TableManager::begin_write_barrier`](crate::engine::table::TableManager::begin_write_barrier)
/// (`SCHEMA_ACTIVATION` bit) — raise the intent bit, drain in-flight
/// fast-path writers, THEN take `unique_write_lock`, in that order (F-70,
/// #897). Returns the table handle plus all three RAII guards; the caller
/// keeps them all alive for the ENTIRE count→persist→activate sequence
/// (drop order: barrier clears first, then the lock, then the repo-level
/// admission guard — reverse declaration order).
///
/// Call this immediately BEFORE the `stamp_keyset_safe` count-proof read so
/// the subsequent count→persist→activate sequence is a genuine snapshot no
/// concurrent writer can invalidate.
pub(in crate::shamir_db::execute) async fn begin_schema_activation_barrier(
    shamir: &ShamirDb,
    db: &str,
    repo: &str,
    table: &str,
) -> Result<
    (
        TableManager,
        crate::engine::table::WriteBarrierGuard,
        tokio::sync::OwnedMutexGuard<()>,
        tokio::sync::OwnedMutexGuard<()>,
    ),
    BatchError,
> {
    let _ddl = repo_ddl_guard(shamir, db, repo).await?;
    let handle = shamir
        .get_table(db, repo, table)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: e.to_string(),
            code: Some("internal_error".to_string()),
        })?;
    let (barrier, uwl_guard) = handle
        .begin_write_barrier(crate::engine::index::write_barrier_flags::SCHEMA_ACTIVATION)
        .await;
    Ok((handle, barrier, uwl_guard, _ddl))
}

/// DROP TABLE CASCADE index cleanup: drop every index in all four families
/// (base_index regular, base_index unique, sorted, index2) on `table` via
/// the raw manager primitives.
///
/// F-3 (#1030): these direct registry mutations (`index_manager_ref()
/// .drop_index`/`drop_unique_index`, `sorted_indexes().drop_index`,
/// `index2_registry().remove_by_id`) bypass the barrier `TableManager`'s own
/// `drop_index`/`drop_unique_index`/`drop_sorted_index`/`drop_index2`
/// wrappers take — violating the precondition `IndexRegistry::insert`'s doc
/// comment states every registry-mutating DDL op must uphold (see
/// `crates/shamir-index/src/registry.rs`, "# Precondition"): a concurrent
/// `CREATE INDEX` could race this cascade's direct drops to the same next
/// generation-counter value, un-serialized. One `begin_write_barrier`
/// acquisition covering the entire block (not four separate ones, which
/// would only serialize against itself) restores that guarantee. Bit choice:
/// `REGULAR_INDEX_CREATE` — the same single per-table `ddl_admission` mutex
/// backs every bit (`begin_write_barrier`'s Step 0), so any one existing bit
/// is sufficient for the admission serialization this fix needs; no new bit
/// is minted since the table is about to be removed entirely regardless of
/// which writers see the slow path via this specific bit. The raw manager
/// primitives called below do NOT themselves acquire
/// `ddl_admission`/`unique_write_lock` (confirmed by reading
/// `TableManager::drop_index`/`drop_unique_index`/`drop_sorted_index`/
/// `drop_index2`, which call these exact primitives AFTER already taking the
/// barrier) — so acquiring it here once cannot re-enter the non-reentrant
/// mutex.
///
/// The caller MUST already hold the repo-level `ddl_admission` guard
/// (`repo_ddl_guard`) — this helper's per-table barrier mutex is a DIFFERENT
/// mutex (`TableManager::ddl_admission`, fresh per table), so the two nest
/// safely; but the cascade as a whole must be serialized against the
/// receipt pipeline at the repo level.
pub(in crate::shamir_db::execute) async fn drop_table_cascade_indexes(table: &TableManager) {
    // base_index regular indexes.
    let regular_ids: Vec<u64> = table
        .index_manager_ref()
        .iter_indexes()
        .map(|d| d.name_interned)
        .collect();
    for id in regular_ids {
        let _ = table.index_manager_ref().drop_index(id, None, None).await;
    }
    // base_index unique indexes.
    let unique_ids: Vec<u64> = table
        .index_manager_ref()
        .iter_unique_indexes()
        .map(|d| d.name_interned)
        .collect();
    for id in unique_ids {
        let _ = table
            .index_manager_ref()
            .drop_unique_index(id, None, None)
            .await;
    }
    // Sorted indexes.
    let sorted_ids: Vec<u64> = table
        .sorted_indexes()
        .iter_indexes()
        .iter()
        .map(|d| d.name_interned)
        .collect();
    for id in sorted_ids {
        let _ = table.sorted_indexes().drop_index(id, None, None).await;
    }
    // index2 registry — remove all backends.
    let backends = table.index2_registry().all_backends().await;
    for b in &backends {
        let _ = table
            .index2_registry()
            .remove_by_id(b.descriptor().id)
            .await;
    }
}
