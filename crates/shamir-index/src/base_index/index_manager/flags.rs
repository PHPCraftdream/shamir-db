use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::types::{RecordKey, Store};
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::types::record_id::RecordId;

use crate::base_index::index_record_key::IndexRecordKey;
use crate::base_index::write_barrier_flags::WriteBarrierFlags;
use crate::base_index::write_barrier_flags::{REGULAR_INDEX_EXISTS, UNIQUE_INDEX_EXISTS};

use super::inner::{HashRenameTombstone, IndexManager};

impl IndexManager {
    /// Синхронизирует атомарные флаги с реальным состоянием индексов.
    pub(super) fn sync_flags(&self) {
        // #1102: sync both REGULAR_INDEX_EXISTS and UNIQUE_INDEX_EXISTS bits
        // from their respective IndexInfo state to the packed word.
        self.write_barrier_flags
            .set_to(REGULAR_INDEX_EXISTS, self.indexes.is_enabled());
        self.write_barrier_flags
            .set_to(UNIQUE_INDEX_EXISTS, self.indexes_unique.is_enabled());
    }

    // ========================================================================
    // P0-3 (#959) — DROP INDEX durable tombstone + crash recovery
    // ========================================================================
    //
    // Sub-bug 3c: a crash between the posting sweep (step 3 of `drop_index`)
    // and the IndexInfo persist (step 4) resurrects a fully-broken "Ready
    // but no postings" index after restart. The fix: persist a tombstone
    // BEFORE the sweep, and clear it AFTER the persist. On restart,
    // `recover_in_progress_drops` resumes any tombstoned-but-incomplete drop.
    //
    // Sub-bug 3b: a name in the tombstone set is rejected by CREATE INDEX
    // until the tombstone clears, preventing ghost postings from namespace
    // reuse.
    //
    // Tombstone shape: `Vec<(u64, Option<String>)>` (id, op_id — #1051)
    // serialized via bincode under `system:idx_drop` (regular) /
    // `system:uidx_drop` (unique). A separate key was chosen over extending
    // `IndexInfo`'s serialized shape to avoid touching `IndexInfo::decode_bytes`'s delicate
    // bincode forward-compat fallback chain (current-shape → pre-`state`
    // legacy shape). An absent key or empty vec means "no in-progress drops".
    //
    // **CRITICAL**: the key names `"idx_drop"` and `"uidx_drop"` are short by
    // design — `RecordId::system(name)` truncates `name` to 12 bytes, so the
    // original candidates `"indexes_dropping"` / `"indexes_unique_dropping"`
    // collided with `"indexes_unique"` (both truncate to `"indexes_uniq"`).

    /// P0-3 (#959 / #1051): persist the current in-memory `dropping_regular` map to
    /// `info_store` under `system:idx_drop`. Serializes the map as a
    /// `Vec<(u64, Option<String>)>` (deterministic order via `BTreeMap`'s sorted iter).
    /// An empty map writes an empty vec (NOT a key deletion) so the load path handles
    /// both `NotFound` and empty-vec uniformly.
    pub(super) async fn save_dropping_regular(&self) -> DbResult<()> {
        let snapshot: Vec<(u64, Option<String>)> = {
            let map = self.dropping_regular.lock().unwrap();
            map.iter().map(|(&k, v)| (k, v.clone())).collect()
        };
        let key = RecordId::system("idx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-3 (#959 / #1051): persist the current in-memory `dropping_unique` map.
    /// Mirror of `save_dropping_regular` for the unique family.
    pub(super) async fn save_dropping_unique(&self) -> DbResult<()> {
        let snapshot: Vec<(u64, Option<String>)> = {
            let map = self.dropping_unique.lock().unwrap();
            map.iter().map(|(&k, v)| (k, v.clone())).collect()
        };
        let key = RecordId::system("uidx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-3 (#959 / #1051): load a persisted dropping map from `info_store`. Returns
    /// an empty `Vec` if the key is absent (`NotFound`) or contains an empty vec —
    /// both mean "no in-progress drops".
    ///
    /// #1051 backward compatibility: tries to deserialize as `Vec<(u64, Option<String>)>`
    /// first (new format with op_id). If that fails, falls back to `Vec<u64>` (old format
    /// without op_id), treating all entries as having `op_id = None`.
    pub(in crate::base_index) async fn load_dropping_set(
        &self,
        is_unique: bool,
    ) -> DbResult<Vec<(u64, Option<String>)>> {
        Self::load_dropping_set_standalone(is_unique, &self.info_store).await
    }

    /// #1048: standalone version of `load_dropping_set` for use by TableManager
    /// BEFORE IndexManager::new clears the tombstones. This allows TableManager
    /// to write SucceededViaCrashRecovery status for recovered hash DROP operations.
    /// Returns an empty `Vec` if the key is absent or contains an empty vec.
    ///
    /// #1051 backward compatibility: see `load_dropping_set`.
    pub async fn load_dropping_set_standalone(
        is_unique: bool,
        info_store: &Arc<dyn Store>,
    ) -> DbResult<Vec<(u64, Option<String>)>> {
        let key_str = if is_unique { "uidx_drop" } else { "idx_drop" };
        let key = RecordId::system(key_str).to_bytes();
        match info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                // Try new format first: Vec<(u64, Option<String>)>
                match bincode::deserialize::<Vec<(u64, Option<String>)>>(&bytes) {
                    Ok(new_format) => Ok(new_format),
                    Err(_) => {
                        // Fall back to old format: Vec<u64> (pre-#1051)
                        let old_format: Vec<u64> = bincode::deserialize(&bytes).map_err(|e| {
                            shamir_storage::error::DbError::Codec(format!(
                                "system:{key_str} decode failed: {e}"
                            ))
                        })?;
                        // Convert to new format with op_id = None
                        Ok(old_format.into_iter().map(|id| (id, None)).collect())
                    }
                }
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// P0-3 (#959 / #1051): add `name_interned` to the in-memory dropping map, then
    /// persist the updated map durably. MUST be called BEFORE the sweep in
    /// `drop_index` / `drop_unique_index` so a crash at any subsequent point
    /// is recoverable.
    ///
    /// In-memory update happens FIRST (so a concurrent CREATE INDEX
    /// immediately sees the name as guarded), then the persist follows. If
    /// the persist fails, the in-memory update is rolled back and the caller
    /// receives `Err` — the drop MUST NOT proceed without a durable
    /// tombstone. A crash between the in-memory update and the persist is
    /// safe: nothing has been swept or persisted yet, so the on-disk state
    /// is unchanged (old IndexInfo, postings intact).
    ///
    /// #1051: accepts an optional `op_id` minted at dispatch time. `None` means
    /// the caller does not have an op_id (e.g., a non-DDL path), and the tombstone
    /// entry will have `op_id = None` (backward compatible with pre-#1051 format).
    pub(in crate::base_index) async fn add_to_dropping(
        &self,
        is_unique: bool,
        name_interned: u64,
        op_id: Option<String>,
    ) -> DbResult<()> {
        let dropping = if is_unique {
            &self.dropping_unique
        } else {
            &self.dropping_regular
        };
        {
            let mut map = dropping.lock().unwrap();
            map.insert(name_interned, op_id);
        }
        let result = if is_unique {
            self.save_dropping_unique().await
        } else {
            self.save_dropping_regular().await
        };
        if result.is_err() {
            // Roll back the in-memory map so the guard stays consistent.
            let mut map = dropping.lock().unwrap();
            map.remove(&name_interned);
        }
        result
    }

    /// P0-3 (#959 / #1051): clear `name_interned` from the persisted tombstone, then
    /// from the in-memory map. Persist-first ordering ensures the on-disk
    /// state is always at least as advanced as the in-memory state: a crash
    /// between persist and in-memory update leaves a stale in-memory entry
    /// that dies with the process, while the on-disk tombstone is already
    /// correct.
    ///
    /// MUST be called AFTER `save_index_info` / `save_index_info_unique`
    /// (the reduced IndexInfo must be durable first). If the process crashes
    /// between the IndexInfo persist and this clear, recovery sees the
    /// tombstone but the def is already gone from IndexInfo — it just clears
    /// the tombstone (a no-op sweep).
    pub(in crate::base_index) async fn clear_from_dropping(
        &self,
        is_unique: bool,
        name_interned: u64,
    ) -> DbResult<()> {
        let dropping = if is_unique {
            &self.dropping_unique
        } else {
            &self.dropping_regular
        };
        // Compute the snapshot without the entry (do NOT modify in-memory yet).
        let snapshot: Vec<(u64, Option<String>)> = {
            let map = dropping.lock().unwrap();
            map.iter()
                .filter(|(&k, _)| k != name_interned)
                .map(|(&k, v)| (k, v.clone()))
                .collect()
        };
        // Persist first.
        let key_str = if is_unique { "uidx_drop" } else { "idx_drop" };
        let key = RecordId::system(key_str).to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        // Only now update in-memory.
        {
            let mut map = dropping.lock().unwrap();
            map.remove(&name_interned);
        }
        Ok(())
    }

    // ========================================================================
    // #1058: In-flight online build registry (regular family) + dirty-set
    // capture. Lifecycle owned by #1059; this slice provides data structures
    // and API only.
    // ========================================================================
    //
    // The registry answers: "is `name_interned` currently in an in-flight online
    // build?" (Phase B/C/D — barrier-free, dirty-set-capturing). Presence in
    // the `in_flight_builds` HashMap means "yes". The dirty-set stores
    // `RecordId`s touched by writes to that building index, for Phase C's
    // catch-up replay.
    //
    // Storage is IN-MEMORY ONLY — per RFC v2 §4.2, a crash loses the dirty-set,
    // but slice 1's conservative crash-recovery policy is restart-from-scratch
    // (a crash also restarts Phase A from scratch per #1060's design), so
    // nothing needs the set to survive a crash in slice 1. The dirty-set is
    // never persisted; resumable Phase-C recovery is explicitly deferred to
    // slice 2+.

    /// #1058: mark an index as having an in-flight online build (Phase B/C/D).
    /// Called by #1059 at Phase B start. If the index is already in the registry,
    /// this is a no-op (safe idempotency — #1059 should call this exactly once per build).
    pub fn mark_build_in_flight(&self, name_interned: u64) {
        self.in_flight_builds
            .entry_sync(name_interned)
            .or_insert(());
        // The dirty-set entry is created lazily in `get_or_create_dirty_set`
        // to avoid allocating a BTreeSet for builds that complete before any
        // writes touch the indexed fields.
    }

    /// #1058: check if an index has an in-flight online build. Used by the
    /// planning methods (`plan_record_created`, `plan_record_updated`,
    /// `plan_record_deleted`) to decide whether to capture to dirty-set vs.
    /// produce direct posting ops.
    ///
    /// Uses `scc::HashMap::get_sync` for lock-free access — critical for the
    /// hot-path call site inside the planning methods.
    pub fn is_build_in_flight(&self, name_interned: u64) -> bool {
        self.in_flight_builds.get_sync(&name_interned).is_some()
    }

    /// #1058: remove an index from the in-flight build registry. Called by
    /// #1059 at Phase D completion or abort. Also removes the dirty-set entry
    /// (Phase C should have drained it before this is called, but we clean up
    /// regardless for safety). If the index is not in the registry, this is a
    /// no-op.
    pub fn clear_build_in_flight(&self, name_interned: u64) {
        let _ = self.in_flight_builds.remove_sync(&name_interned);
        let mut dirty = self.dirty_sets.lock().unwrap();
        dirty.remove(&name_interned);
    }

    /// #1059: drain the dirty-set for `name_interned`, returning every RecordId
    /// captured since the last drain (or since the build started, for the first
    /// drain). Returns an empty Vec if nothing was captured or the index isn't
    /// in-flight. Draining clears the set — a RecordId captured AFTER this call
    /// returns is not lost, it accumulates for the NEXT drain.
    pub fn drain_dirty_set(&self, name_interned: u64) -> Vec<RecordId> {
        if !self.is_build_in_flight(name_interned) {
            return Vec::new();
        }
        let dirty = self.dirty_sets.lock().unwrap();
        match dirty.get(&name_interned) {
            Some(set) => {
                let mut inner = set.lock().unwrap();
                std::mem::take(&mut *inner).into_iter().collect()
            }
            None => Vec::new(),
        }
    }

    /// #1058: get the dirty-set for an in-flight build (internal helper for
    /// the planning methods). Returns `None` if the index is not in the registry.
    pub(super) fn get_or_create_dirty_set(
        &self,
        name_interned: u64,
    ) -> Option<Arc<Mutex<BTreeSet<RecordId>>>> {
        if !self.is_build_in_flight(name_interned) {
            return None;
        }

        let mut dirty = self.dirty_sets.lock().unwrap();
        dirty
            .entry(name_interned)
            .or_insert_with(|| Arc::new(Mutex::new(BTreeSet::new())))
            .clone()
            .into()
    }

    // ========================================================================
    // #997 — RENAME INDEX (regular/unique hash) durable tombstone + crash
    // recovery. Mirrors #959's `idx_drop`/`uidx_drop` (same `IndexManager`
    // owner) and #962's `sidx_ren` (same operation, sorted family).
    // ========================================================================
    //
    // The regular/unique hash RENAME is a drop+rebuild (the hash physical
    // key mixes `name_interned` into h1/h2, so postings cannot be rekeyed —
    // see `rename_index`'s doc). The tombstone therefore carries enough to
    // rebuild from nothing: `HashRenameTombstone { old_name, new_name,
    // paths }`. `TableManager::rename_index` writes the tombstone BEFORE
    // the first mutating step and clears it AFTER the last; the engine-side
    // `TableManager::recover_hash_renames` finishes any interrupted rename
    // on restart (it needs the record stream + interner for a backfill, so
    // it cannot live on `IndexManager`).
    //
    // Tombstone shape: `Vec<HashRenameTombstone>` serialized via bincode
    // under `system:idx_ren` (regular) / `system:uidx_ren` (unique). An
    // absent key or empty vec means "no in-progress renames".
    //
    // **CRITICAL** (the hard-won #959 collision lesson): `RecordId::system(name)`
    // truncates `name` to 12 bytes (see `shamir_types::types::record_id::system`).
    // `"idx_ren"` (7 bytes) is verified collision-free against every other
    // base_index system key:
    //   `"idx_drop"` → [0,0,0,0, 69,64,78,5f,64,72,6f,70, 00,00,00,00]  (8 bytes)
    //   `"idx_ren"`  → [0,0,0,0, 69,64,78,5f,72,65,6e,00, 00,00,00,00]  (7 bytes)
    // They share the first 4 bytes (`idx_`) but diverge at byte 8 (within
    // the name): `0x64` (`d`) vs `0x72` (`r`) — NO collision.
    //   `"indexes"`  → [0,0,0,0, 69,6e,64,65,78,65,73,00, 00,00,00,00]
    //   `"idx_ren"`  → [0,0,0,0, 69,64,78,5f,72,65,6e,00, 00,00,00,00]
    // Diverge at byte 5 (`n`=`0x6e` vs `d`=`0x64`) — NO collision.
    //
    // `"uidx_ren"` (8 bytes) is verified collision-free:
    //   `"uidx_drop"`    → [0,0,0,0, 75,69,64,78,5f,64,72,6f,70,00,00,00]
    //   `"uidx_ren"`     → [0,0,0,0, 75,69,64,78,5f,72,65,6e,00,00,00,00]
    // Diverge at byte 9 (`d`=`0x64` vs `r`=`0x72`) — NO collision.
    //   `"indexes_uniq"` → [0,0,0,0, 69,6e,64,65,78,65,73,5f,75,6e,69,71]  (truncated 12)
    //   `"uidx_ren"`     → [0,0,0,0, 75,69,64,78,5f,72,65,6e,00,00,00,00]
    // Diverge at byte 4 (`i`=`0x69` vs `u`=`0x75`) — NO collision.

    /// #997: persist the in-memory `renaming_regular` map to `info_store`
    /// under `system:idx_ren`. Serializes as a `Vec<HashRenameTombstone>`
    /// (deterministic order via `BTreeMap`). An empty map writes an empty
    /// `Vec` (NOT a key deletion) so the load path handles `NotFound` and
    /// empty-vec uniformly. Mirrors #959's `save_dropping_regular`.
    pub async fn save_renaming_regular(&self) -> DbResult<()> {
        let snapshot: Vec<HashRenameTombstone> = {
            let map = self.renaming_regular.lock().unwrap();
            map.values().cloned().collect()
        };
        let key = RecordId::system("idx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// #997: persist the in-memory `renaming_unique` map. Mirror of
    /// `save_renaming_regular` for the unique family.
    pub async fn save_renaming_unique(&self) -> DbResult<()> {
        let snapshot: Vec<HashRenameTombstone> = {
            let map = self.renaming_unique.lock().unwrap();
            map.values().cloned().collect()
        };
        let key = RecordId::system("uidx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// #997: load a persisted rename tombstone list from `info_store`.
    /// Returns an empty `Vec` if the key is absent (`NotFound`) or contains
    /// an empty vec — both mean "no in-progress renames". Mirrors #959's
    /// `load_dropping_set` (adapted for the richer payload type).
    pub async fn load_renaming_list(&self, is_unique: bool) -> DbResult<Vec<HashRenameTombstone>> {
        let key_str = if is_unique { "uidx_ren" } else { "idx_ren" };
        let key = RecordId::system(key_str).to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<HashRenameTombstone>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:{key_str} decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// #997: record a rename in the in-memory map, then persist durably.
    /// MUST be called BEFORE the first mutating step in the rename
    /// (`create_index` for regular, `drop_unique_index` for unique) so a
    /// crash at any subsequent point is recoverable by
    /// `TableManager::recover_hash_renames`.
    ///
    /// In-memory update happens FIRST, then the persist follows. If the
    /// persist fails, the in-memory update is rolled back and the caller
    /// receives `Err` — the rename MUST NOT proceed without a durable
    /// tombstone. Mirrors #959's `add_to_dropping`.
    pub async fn add_to_renaming(
        &self,
        is_unique: bool,
        old_name_interned: u64,
        entry: HashRenameTombstone,
    ) -> DbResult<()> {
        let renaming = if is_unique {
            &self.renaming_unique
        } else {
            &self.renaming_regular
        };
        {
            let mut map = renaming.lock().unwrap();
            map.insert(old_name_interned, entry);
        }
        let result = if is_unique {
            self.save_renaming_unique().await
        } else {
            self.save_renaming_regular().await
        };
        if result.is_err() {
            let mut map = renaming.lock().unwrap();
            map.remove(&old_name_interned);
        }
        result
    }

    /// #997: clear `old_name_interned` from the persisted tombstone, then
    /// from the in-memory map. Persist-first ordering ensures the on-disk
    /// state is always at least as advanced as the in-memory state.
    ///
    /// MUST be called AFTER the last mutating step succeeds (the new index
    /// is registered + backfilled for regular; the new unique index is
    /// registered + backfilled for unique). If the process crashes between
    /// the last step and this clear, recovery sees the tombstone and
    /// reconciles — see `TableManager::recover_hash_renames`'s crash-state
    /// matrix. Mirrors #959's `clear_from_dropping`.
    pub async fn clear_from_renaming(
        &self,
        is_unique: bool,
        old_name_interned: u64,
    ) -> DbResult<()> {
        let renaming = if is_unique {
            &self.renaming_unique
        } else {
            &self.renaming_regular
        };
        let snapshot: Vec<HashRenameTombstone> = {
            let map = renaming.lock().unwrap();
            map.iter()
                .filter(|(&k, _)| k != old_name_interned)
                .map(|(_, v)| v.clone())
                .collect()
        };
        let key_str = if is_unique { "uidx_ren" } else { "idx_ren" };
        let key = RecordId::system(key_str).to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        {
            let mut map = renaming.lock().unwrap();
            map.remove(&old_name_interned);
        }
        Ok(())
    }

    /// #997 (post-review fix): clear the ENTIRE persisted rename tombstone
    /// list for one family by writing an explicit empty `Vec`, bypassing the
    /// in-memory `renaming_regular`/`renaming_unique` map entirely.
    ///
    /// `TableManager::recover_hash_renames` MUST use this once per family
    /// after its whole reconciliation loop finishes, NOT `clear_from_renaming`
    /// per entry. At open time `renaming_regular`/`renaming_unique` start
    /// empty (`IndexManager::new`) and `load_renaming_list` never rehydrates
    /// them — it only returns a local `Vec`. So a per-entry
    /// `clear_from_renaming` call during recovery would derive its snapshot
    /// from that empty map and persist `[]` after the FIRST entry, silently
    /// discarding the durable tombstone for every NOT-YET-recovered entry. If
    /// recovery then failed or the process crashed again before finishing the
    /// remaining entries, their stranded state would become permanently
    /// invisible on the next restart. Mirrors #959's `recover_in_progress_drops`,
    /// which writes an explicit empty `Vec<u64>` once after its loop instead
    /// of calling `clear_from_dropping` per entry.
    pub async fn clear_all_renaming(&self, is_unique: bool) -> DbResult<()> {
        let renaming = if is_unique {
            &self.renaming_unique
        } else {
            &self.renaming_regular
        };
        let key_str = if is_unique { "uidx_ren" } else { "idx_ren" };
        let key = RecordId::system(key_str).to_bytes();
        let empty = bincode::serialize(&Vec::<HashRenameTombstone>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(empty)).await?;
        {
            let mut map = renaming.lock().unwrap();
            map.clear();
        }
        Ok(())
    }

    /// P0-3 (#959): sweep all posting entries for the given index prefix.
    /// Extracted from `drop_index` / `drop_unique_index` so the recovery
    /// path can re-run it idempotently. Removing already-removed keys is a
    /// no-op on every `Store` backend.
    pub(in crate::base_index) async fn sweep_index_postings(
        &self,
        is_unique: bool,
        name_interned: u64,
    ) -> DbResult<()> {
        let prefix = IndexRecordKey::new(is_unique, name_interned).to_prefix_bytes();
        use futures::StreamExt;
        let mut to_remove: Vec<RecordKey> = Vec::new();
        let mut stream = self
            .info_store
            .scan_prefix_stream(prefix.clone(), FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            for (key, _) in batch_result? {
                to_remove.push(key);
            }
        }
        if !to_remove.is_empty() {
            let _ = self.info_store.remove_many(to_remove).await?;
        }
        self.posting_cache.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    /// P0-3 (#959): open-time recovery for DROP INDEX operations interrupted
    /// by a crash. Called from `IndexManager::new` AFTER definitions are
    /// loaded but BEFORE the manager is returned to the caller.
    ///
    /// # Crash-state matrix
    ///
    /// | crash point                    | IndexInfo has def? | tombstone? | recovery action            |
    /// |--------------------------------|--------------------|------------|----------------------------|
    /// | after tombstone-write, pre-sweep | yes (Ready)        | yes        | remove def, sweep, persist |
    /// | after sweep, pre-persist        | yes (Ready)        | yes        | remove def, sweep (no-op), persist |
    /// | after persist, pre-clear        | no                 | yes        | sweep (no-op), clear tombstone |
    ///
    /// In every case the recovery leaves the manager in a consistent state:
    /// the def is gone from IndexInfo, its postings are swept, and the
    /// tombstone is cleared. The sweep is idempotent (removing already-removed
    /// keys is a no-op), so calling recovery twice (two restart attempts) is
    /// a clean no-op on the second call.
    pub(super) async fn recover_in_progress_drops(&self) -> DbResult<()> {
        let dropping_regular = self.load_dropping_set(false).await?;
        let dropping_unique = self.load_dropping_set(true).await?;

        if dropping_regular.is_empty() && dropping_unique.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-3 (#959): recovering {} regular + {} unique in-progress DROP(s)",
            dropping_regular.len(),
            dropping_unique.len()
        );

        // ── Regular family ───────────────────────────────────────────────
        let mut regular_changed = false;
        for &(name_interned, ref _op_id) in &dropping_regular {
            if self.indexes.contains(name_interned) {
                // Crash between tombstone-write and IndexInfo-persist:
                // def is still in IndexInfo at Ready. Resume the drop.
                self.indexes.remove_index(name_interned);
                self.bump_generation();
                regular_changed = true;
            }
            // Always run the sweep (idempotent). Covers both the
            // "sweep never ran" and "sweep ran but persist failed" cases.
            self.sweep_index_postings(false, name_interned).await?;
        }
        if regular_changed {
            // NOTE: This is the ONLY site that bumps generation BEFORE setting
            // REGULAR_INDEX_EXISTS (reverse of the CREATE order). This is safe because
            // it runs at open-time during IndexManager::new, single-threaded, before
            // the manager is published/shared — no concurrent reader can race this.
            // #1102: update REGULAR_INDEX_EXISTS to match the new IndexInfo state
            // (may be true if other indexes remain, false if this was the last one).
            self.write_barrier_flags
                .set_to(REGULAR_INDEX_EXISTS, self.indexes.is_enabled());
            self.save_index_info().await?;
        }

        // ── Unique family ────────────────────────────────────────────────
        let mut unique_changed = false;
        for &(name_interned, ref _op_id) in &dropping_unique {
            if self.indexes_unique.contains(name_interned) {
                self.indexes_unique.remove_index(name_interned);
                self.bump_generation();
                unique_changed = true;
            }
            self.sweep_index_postings(true, name_interned).await?;
        }
        if unique_changed {
            self.write_barrier_flags
                .set_to(UNIQUE_INDEX_EXISTS, self.indexes_unique.is_enabled());
            self.save_index_info_unique().await?;
        }

        // ── Clear both tombstones ────────────────────────────────────────
        // Write empty Vec<(u64, Option<String>)> for both keys (cheaper than a remove, and
        // the load path treats empty-vec and NotFound identically).
        let empty = bincode::serialize(&Vec::<(u64, Option<String>)>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let key_regular = RecordId::system("idx_drop").to_bytes();
        self.info_store
            .set(key_regular.into(), Bytes::from(empty.clone()))
            .await?;
        let key_unique = RecordId::system("uidx_drop").to_bytes();
        self.info_store
            .set(key_unique.into(), Bytes::from(empty))
            .await?;

        // Sync in-memory flags one more time after recovery.
        self.sync_flags();

        log::info!(
            "P0-3 (#959 / #1051): recovery complete — {} regular + {} unique DROP(s) finalized",
            dropping_regular.len(),
            dropping_unique.len()
        );

        Ok(())
    }

    /// Проверяет, есть ли хоть один обычный индекс.
    ///
    /// #1102: reads the shared [`REGULAR_INDEX_EXISTS`] bit of the packed
    /// [`write_barrier_flags`] word (`SeqCst`), closing the ordering gap
    /// that let a row commit with no posting under a concurrently-created
    /// regular index (the former standalone `has_indexes: Arc<AtomicBool>`
    /// was written `Release` but read `Relaxed` — no happens-before edge).
    /// This mirrors `has_unique_indexes()`'s shape; both are now `SeqCst`
    /// on the same packed word, guaranteeing consistent ordering semantics.
    pub fn has_indexes(&self) -> bool {
        self.write_barrier_flags.is_set(REGULAR_INDEX_EXISTS)
    }

    /// Проверяет, есть ли хоть один уникальный индекс.
    ///
    /// F-69 (#896): reads the shared [`WriteBarrierFlags`] word (`SeqCst`) —
    /// previously a standalone `Relaxed` `AtomicBool` load, which is the
    /// exact ordering gap that let a duplicate slip past a unique
    /// constraint racing a concurrent index-create (see
    /// `write_barrier_flags.rs`'s module doc). `SeqCst` here costs nothing
    /// extra in practice (x86 loads are already total-order-respecting;
    /// the fence cost of `SeqCst` is paid on the RMW/store side, not this
    /// load) and is required to keep this bit inside the SAME total order
    /// as `TableManager::needs_write_barrier()`'s single load of this word.
    pub fn has_unique_indexes(&self) -> bool {
        self.write_barrier_flags.is_set(UNIQUE_INDEX_EXISTS)
    }

    /// P0-2 (#958): current base_index `IndexManager` generation (regular +
    /// unique combined). Bumped (monotonic) whenever the set of definitions
    /// changes. The zero-overhead gate value for commit-time base_index
    /// ops-plan re-derivation: a tx captures this at stage time and, at
    /// commit, skips re-derivation entirely unless it has advanced.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// P0-2 (#958): advance the generation counter after a definition
    /// change (create or drop of a regular or unique index). Called by
    /// every create / drop path in this manager and in
    /// `index_manager_unique.rs` (same `impl IndexManager` block).
    /// Mirrors `SortedIndexManager`'s `generation.fetch_add` and
    /// `IndexRegistry`'s equivalent.
    pub(in crate::base_index) fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// F-69 (#896, P0): expose the shared packed write-barrier word so
    /// `TableManager` (`shamir-engine`) can fold its own five DDL-intent
    /// bits into the SAME `Arc<AtomicU16>` this manager uses for
    /// [`UNIQUE_INDEX_EXISTS`] — the shape that makes
    /// `needs_write_barrier()` a single atomic load across both crates'
    /// halves of the predicate. See `write_barrier_flags.rs`'s module doc
    /// for the full ownership rationale.
    pub fn write_barrier_flags(&self) -> WriteBarrierFlags {
        self.write_barrier_flags.clone()
    }
}
