//! F-71 (#898) — manager-level tests for the three AsOf epoch-initialization
//! vectors identified as an F-67 (#893) regression:
//!
//! 1. **Restart** — `SortedIndexManager::load()` must seed
//!    `last_mutation_version` from each definition's durable
//!    `ready_at_version`, not leave it at the empty-map default of `0`.
//! 2. **CREATE INDEX** (the manager-level half — `mark_ready_at`) — a
//!    freshly backfilled index's epoch must land at the table's watermark at
//!    backfill-completion time, never `0`.
//! 3. **RENAME INDEX** (`rename_definition`) — the in-memory epoch entry
//!    must travel from the old `name_interned` to the new one, not reset to
//!    `0`.
//!
//! These are unit tests against `SortedIndexManager` directly (no
//! `TableManager`/MVCC harness) — the engine-level integration tests in
//! `crates/shamir-engine/src/table/tests/f71_asof_epoch_init_tests.rs` prove
//! the same three vectors through the actual `read_as_of` gate.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use crate::base_index::sorted_index_manager::{SortedIndexDefinition, SortedIndexManager};

use super::helpers::{enc_i64, record_with_int};

// ─────────────────────────────────────────────────────────────────────────────
// Vector 1 — restart.
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-fix: a fresh `SortedIndexManager::load()` never touched
/// `last_mutation_version`, so any restarted index read epoch `0` no matter
/// what `ready_at_version` its persisted definition carried. Post-fix,
/// `load()` seeds the map from `ready_at_version`, so the epoch survives a
/// restart intact.
#[tokio::test]
async fn restart_restores_epoch_from_persisted_ready_at_version() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.register(SortedIndexDefinition::new(101, vec![201]))
            .await
            .unwrap();
        // Simulate "CREATE INDEX completed at table version 500" —
        // mark_ready_at is the exact call the engine's backfill makes.
        mgr.mark_ready_at(101, 500).await.unwrap();
        assert_eq!(mgr.last_mutation_version(101), 500);
    }

    // "Restart": a brand-new manager instance over the SAME info_store.
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert_eq!(
        mgr2.last_mutation_version(101),
        500,
        "epoch must survive restart via the persisted ready_at_version floor, \
         not silently reset to 0"
    );
}

/// A definition persisted by a genuinely pre-F-71 build (no `ready_at_version`
/// field on disk) decodes with the `#[serde(default)]` value `0` — the OLD,
/// merely-permissive default. Confirms backward-compat decode does not panic
/// and the fallback floor is exactly `0` (not something worse).
#[tokio::test]
async fn restart_with_legacy_v1_defs_floors_epoch_at_zero() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Persist using the OLD V1 on-disk shape directly (no `included_fields`,
    // no `ready_at_version` — mirrors data written before both fields
    // existed).
    #[derive(serde::Serialize)]
    struct V1 {
        name_interned: u64,
        field_path: Vec<u64>,
    }
    let v1s = vec![V1 {
        name_interned: 202,
        field_path: vec![9],
    }];
    let bytes = bincode::serialize(&v1s).unwrap();
    let sys_id = RecordId::system("sorted_indexes");
    info_store
        .set(sys_id.to_bytes().into(), bytes::Bytes::from(bytes))
        .await
        .unwrap();

    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr.find_by_name_interned(202).is_some());
    assert_eq!(
        mgr.last_mutation_version(202),
        0,
        "legacy V1 defs (no ready_at_version) floor at 0, matching pre-fix behavior \
         (not a regression — the gate is still safe, just not improved for old data)"
    );
}

/// Empty table: an index registered but never backfilled/mutated stays at
/// epoch 0 across a restart — no false floor is invented for an index that
/// was never marked ready.
#[tokio::test]
async fn restart_with_never_mutated_index_stays_zero() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.register(SortedIndexDefinition::new(303, vec![401]))
            .await
            .unwrap();
        // No mark_ready_at call — mirrors an index whose CREATE never
        // completed the backfill in this session, or one on the legacy
        // pre-F-71 code path.
    }
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert_eq!(mgr2.last_mutation_version(303), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Vector 2 — CREATE INDEX (`mark_ready_at`).
// ─────────────────────────────────────────────────────────────────────────────

/// `mark_ready_at` must set the epoch to the table's version at
/// backfill-completion time — NOT `0` — even when the backfill itself only
/// ever called `on_record_created(.., 0)` (the version-`0` placeholder every
/// backfill row uses, since backfill isn't a real MVCC write).
#[tokio::test]
async fn create_index_epoch_floors_at_table_version_not_zero() {
    let (_info, mgr) = super::helpers::fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();

    // Backfill 5 rows all at the version-0 placeholder — mirrors
    // `create_sorted_index_with_include`'s loop exactly.
    for score in 0..5i64 {
        let id = RecordId::new();
        mgr.on_record_created(&id, &record_with_int(201, score), 0)
            .await
            .unwrap();
    }
    // Pre-mark: the placeholder-0 writes above left the epoch at 0.
    assert_eq!(mgr.last_mutation_version(101), 0);

    // The engine samples the table's real watermark AFTER backfill drains
    // and calls this — say the table was already at version 100 when the
    // CREATE INDEX ran (prior mutation history the new index now mirrors).
    mgr.mark_ready_at(101, 100).await.unwrap();

    assert_eq!(
        mgr.last_mutation_version(101),
        100,
        "epoch must reflect the table's watermark at backfill-completion, not the \
         placeholder-0 writes the backfill loop made"
    );
}

/// An index created on a genuinely EMPTY table (no backfill rows at all)
/// still floors at the table's current version, not `0` — "ready as of now",
/// not "ready as of the dawn of time".
#[tokio::test]
async fn create_index_on_empty_table_still_floors_at_table_version() {
    let (_info, mgr) = super::helpers::fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(202, vec![301]))
        .await
        .unwrap();
    // No backfill rows — table was empty.
    mgr.mark_ready_at(202, 77).await.unwrap();
    assert_eq!(mgr.last_mutation_version(202), 77);
}

/// `mark_ready_at` is a floor (max), not an overwrite: calling it again with
/// a LOWER version (e.g. a doctor repair re-run against a stale watermark)
/// must never move the epoch backward.
#[tokio::test]
async fn mark_ready_at_never_moves_epoch_backward() {
    let (_info, mgr) = super::helpers::fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    mgr.mark_ready_at(101, 500).await.unwrap();
    mgr.mark_ready_at(101, 10).await.unwrap();
    assert_eq!(
        mgr.last_mutation_version(101),
        500,
        "a lower re-mark must not move the epoch backward"
    );
}

/// `mark_ready_at` persists `ready_at_version` durably — combined with the
/// restart tests above, this is the CREATE-then-restart path: create,
/// mark ready, restart, and confirm the epoch is still the CREATE-time
/// watermark (not `0`, not stuck at the pre-mark placeholder).
#[tokio::test]
async fn create_index_epoch_survives_restart() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.register(SortedIndexDefinition::new(101, vec![201]))
            .await
            .unwrap();
        for score in 0..3i64 {
            let id = RecordId::new();
            mgr.on_record_created(&id, &record_with_int(201, score), 0)
                .await
                .unwrap();
        }
        mgr.mark_ready_at(101, 250).await.unwrap();
    }
    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert_eq!(mgr2.last_mutation_version(101), 250);
    // Sanity: the entries backfilled before the restart are still there too.
    let r = mgr2
        .lookup_range(101, Some(&enc_i64(0)), Some(&enc_i64(2)))
        .await
        .unwrap();
    assert_eq!(r.len(), 3);
}

// ─────────────────────────────────────────────────────────────────────────────
// Vector 3 — RENAME INDEX (`rename_definition`).
// ─────────────────────────────────────────────────────────────────────────────

/// `rename_definition` must carry the in-memory mutation-epoch entry from the
/// old `name_interned` to the new one — a rename must not reset the AsOf gate
/// to `0`.
#[tokio::test]
async fn rename_definition_carries_epoch_to_new_name() {
    let (_info, mgr) = super::helpers::fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    mgr.mark_ready_at(101, 300).await.unwrap();
    // A later plain mutation bumps it further, to confirm the CURRENT value
    // (not just the ready_at_version floor) is what travels.
    mgr.note_mutation_at_version(101, 305);
    assert_eq!(mgr.last_mutation_version(101), 305);

    mgr.rename_definition(101, 999).await.unwrap();

    assert_eq!(
        mgr.last_mutation_version(999),
        305,
        "epoch must survive the rename under the NEW name"
    );
    assert_eq!(
        mgr.last_mutation_version(101),
        0,
        "the OLD name's epoch entry must be gone (reads as the never-mutated default)"
    );
}

/// The renamed epoch is ALSO durable: the definition's `ready_at_version`
/// travels for free (rename mutates `name_interned` in place on the same
/// struct), so a restart after a rename still restores the correct epoch
/// under the NEW name.
#[tokio::test]
async fn rename_definition_epoch_survives_restart() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.register(SortedIndexDefinition::new(101, vec![201]))
            .await
            .unwrap();
        mgr.mark_ready_at(101, 400).await.unwrap();
        mgr.rename_definition(101, 999).await.unwrap();
        assert_eq!(mgr.last_mutation_version(999), 400);
    }

    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr2.find_by_name_interned(999).is_some());
    assert!(mgr2.find_by_name_interned(101).is_none());
    assert_eq!(
        mgr2.last_mutation_version(999),
        400,
        "renamed index's epoch must be restored under the NEW name after restart"
    );
}

/// Renaming an index that was never mutated (epoch 0 under the old name)
/// correctly leaves the new name at epoch 0 too — no entry to carry, and
/// `rename_definition` must not error or fabricate one.
#[tokio::test]
async fn rename_definition_of_never_mutated_index_stays_zero() {
    let (_info, mgr) = super::helpers::fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    mgr.rename_definition(101, 999).await.unwrap();
    assert_eq!(mgr.last_mutation_version(999), 0);
}
