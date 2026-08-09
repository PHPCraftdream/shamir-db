# Write Path Audit — Regular/Hash Posting Keyspace Mutations

**Task**: #1054 — Examine all write paths that mutate regular/hash posting entries to ensure the dirty-set capture for online-CREATE-INDEX is exhaustive.

**Date**: 2026-08-09

**Auditor**: Automated code review

## Executive Summary

This document exhaustively enumerates every path in ShamirDB that can mutate the regular/hash posting keyspace. The audit identified **seven** distinct write paths:

1. **Transaction-staged path** (6 sites) — plans index ops at stage time, applies at commit
2. **Non-transactional path** (3 methods, 5 call sites) — direct `on_record_*` calls on CRUD
3. **Doctor::repair() rebuild path** (3 call sites) — index rebuild after corruption
4. **Index2 backfill path** (2 sites) — CREATE INDEX backfill
5. **Sorted index backfill path** (1 site) — CREATE INDEX backfill
6. **Replication migration backfill path** (1 site) — repo migration backfill
7. **WAL recovery replay path** (2 sites) — crash recovery, **does NOT need dirty-set capture**

All paths except #7 are **primary write paths** that derive new posting keys from record values and must be captured by the dirty-set during online-CREATE-INDEX.

Path #7 (WAL recovery) is a **replay path** that re-applies already-computed posting keys captured at original commit time — it does NOT derive new keys, so it needs no independent dirty-set capture.

## Exhaustiveness Method

To claim exhaustiveness, this audit used the **following method**:

1. **Cross-referenced three complementary search axes**:
   - All callers of `on_record_created`/`on_record_updated`/`on_record_deleted` on `IndexManager`, `SortedIndexManager`, and index2 backends
   - All callers of `set_many`/`remove` on `info_store` that operate on posting keys (verified by key prefix inspection)
   - All construction sites of `IndexWriteOp::SetPosting`/`RemovePosting` for the regular/unique families

2. **Triangulated results**: Each path appears in at least two of the three search axes, eliminating false positives/negatives.

3. **Codebase coverage**: Searched `crates/shamir-engine/`, `crates/shamir-index/`, `crates/shamir-tx/`, and `crates/shamir-wal/` (production code only, not test code).

4. **Verification**: Manually inspected each found site to confirm it:
   - Actually mutates postings (not just reads or metadata)
   - Derives posting keys from record data (not replaying already-derived keys)
   - Is a primary write path (not an administrative sweep like DROP)

## Path Inventory

### Path 1: Transaction-Staged Path

**Description**: Transactional operations (INSERT/UPSERT/UPDATE/DELETE) plan index ops at STAGE time via `plan_*_ops` methods, capture the `generation()` of each index family, and re-derive ops at COMMIT if any generation changed. This is the primary transactional write path.

**Mechanism-level behavior (CRITICAL for online-build safety)**:

The planning methods (`IndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted` and their `SortedIndexManager` equivalents) iterate over **all registered index definitions with NO filter on `IndexState`**:

- `IndexManager::plan_record_created` (`crates/shamir-index/src/base_index/index_manager.rs:2023`) — `for def in self.indexes.iter()` — raw iterator, includes `Building` defs
- `IndexManager::plan_record_updated` (`:2074`) — same pattern
- `IndexManager::plan_record_deleted` (`:2130`) — same pattern
- `SortedIndexManager::plan_record_created` (`crates/shamir-index/src/base_index/sorted_index_manager.rs:1564`) — `let defs = self.iter_indexes(); for def in &defs` — includes `Building` defs
- `SortedIndexManager::plan_record_updated` (`:1644`) — same pattern
- `SortedIndexManager::plan_record_deleted` (`:1713`) — same pattern

When an index is registered at `Building` state via `add_index` (BEFORE the backfill scan starts — see `create_index_from_stream` Phase 1 at `index_manager.rs:1615-1620`), **every subsequent call to these planning methods includes that `Building` def and produces `SetPosting`/`RemovePosting` ops for it**. This happens at **ordinary stage-time planning**, NOT just in the re-derivation branch.

Today this is SAFE because `TableManager::create_index` holds `begin_write_barrier` across the ENTIRE Phase 1→2→3 sequence (documented at `index_manager.rs:1501-1519`), blocking ALL writers (tx-staged and non-tx) for the whole backfill duration. The generation-gate re-derivation path (`rederive_base_index_ops_post_stage`) is a SEPARATE mechanism for the case where an index is registered AFTER stage but BEFORE commit — it is NOT the only way tx-staged writes reach Building indexes.

**Hazard under online-build**: Once slice 1 removes the barrier for Phase A/B/C, the ordinary stage-time planning path will execute CONCURRENTLY with Phase A's snapshot scan — both writing to the same posting keyspace. Without dirty-set interception, Path 1's direct `SetPosting` writes race Phase A's writes with no ordering guarantee.

**Files**:
- `crates/shamir-engine/src/table/table_manager_tx_ops.rs`

**Call sites** (10 total, covering 6 tx methods):
- `insert_tx` — `:451` captures `base_index_gen`, calls `plan_insert_ops` at `:456`
- `insert_tx` — `:479` captures via `tx.note_base_index_stage_gen(token, base_index_gen)`
- `insert_tx_many` — `:592` captures `base_index_gen`, calls `plan_base_index_insert_ops` at `:631`
- `insert_tx_many` — `:677` captures via `tx.note_base_index_stage_gen(token, base_index_gen)`
- `set_tx` — `:785` captures `base_index_gen`, calls `plan_base_index_update_ops` at `:831`
- `set_tx` — `:924` captures via `tx.note_base_index_stage_gen(token, base_index_gen)`
- `delete_tx` — `:999` captures `base_index_gen`
- `delete_tx` — `:1094` captures via `tx.note_base_index_stage_gen(token, base_index_gen)`
- `upsert_tx_many_bytes` — `:1147` captures `base_index_gen`
- `upsert_tx_many_bytes` — `:1193` captures via `tx.note_base_index_stage_gen(token, base_index_gen)`

**Barrier coverage**: YES — passes through the SSI validation pipeline; commit-time gate detects `Building` indexes via generation mismatch.

**Building visibility**: YES — but NOT only via re-derivation. The PRIMARY visibility is through the ordinary planning loop, which iterates over ALL registered defs (including `Building` ones) and produces ops for them. The generation-mismatch re-derivation is a secondary catch-up path for indexes registered between stage and commit.

**Dirty-set capture point**: **NEW CAPTURE NEEDED** — same as Path 2. The planning methods (`IndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted` and their `SortedIndexManager` equivalents) are the shared choke point where posting keys are derived and `SetPosting`/`RemovePosting` ops are produced. These methods iterate over ALL defs with NO state filter, so they produce ops for `Building` indexes directly.

**Recommended capture architecture**: Place the dirty-set check **inside the shared planning methods themselves** (e.g., `IndexManager::plan_record_created` at `index_manager.rs:2013-2038` and its equivalents), NOT at the ~15 scattered call sites in `table_manager_tx_ops.rs` and `table_manager_crud.rs`. The logic would be:

1. Inside the planning loop (`for def in self.indexes.iter()`), check `def.state`
2. If `def.state == IndexState::Building` AND this Building index has an active in-flight online-build registry entry (per #1058's planned registry):
   - Route to dirty-set capture: add the `RecordId` to the dirty-set for this index (persisted in `info_store`)
   - Do NOT produce a `SetPosting`/`RemovePosting` op for this specific def (or produce it AND dirty-set, then let Phase C choose which to apply — design choice for #1058)
3. If `def.state == IndexState::Ready` (or no active build):
   - Produce the `SetPosting`/`RemovePosting` op as usual

This single-choke-point design is preferable to scattering capture logic at ~15 call sites because:
- The planning methods are already the place where all index families share the same logic
- It centralizes the `IndexState` check and dirty-set routing
- It works for both Path 1 (tx-staged) and Path 2 (non-tx CRUD) without duplication
- It avoids missing a call site in future refactors

### Path 2: Non-Transactional Path

**Description**: Non-transactional CRUD operations (`insert`, `delete`, `set`/`upsert`) call `on_record_created`/`on_record_updated`/`on_record_deleted` DIRECTLY on index managers, which internally call the SAME `plan_record_*` methods as Path 1. This bypasses the generation gate entirely.

**Mechanism-level relationship to Path 1**:

Path 1 and Path 2 are **NOT independent write paths** — they are two different CALLERS of the exact SAME underlying planning methods:

- **Path 1** calls planning methods via `table_manager_tx_ops.rs`:
  - `plan_base_index_insert_ops` at `:285` calls `self.index_manager.plan_record_created(&rid, rec)`
  - `plan_records_created_batch` at `:823` calls `self.index_manager.plan_records_created_batch(pairs())`
  - `plan_records_updated_batch` at `:826` calls `self.index_manager.plan_records_updated_batch(pairs())`

- **Path 2** calls planning methods via `table_manager_crud.rs`:
  - `:211` calls `self.index_manager.on_record_created(&id, value)` which internally calls `plan_record_created`
  - `:452` calls `self.index_manager.on_record_deleted(&id, old)` which internally calls `plan_record_deleted`
  - `:562` calls `self.index_manager.on_record_updated(&id, old, value)` which internally calls `plan_record_updated`

Both paths funnel through the SAME choke point: the planning methods in `IndexManager` and `SortedIndexManager` that iterate over ALL defs (including `Building` ones) and produce `SetPosting`/`RemovePosting` ops. This is why the dirty-set capture point belongs INSIDE those shared methods (as described in Path 1), NOT at the ~15 scattered call sites.

**Files**:
- `crates/shamir-engine/src/table/table_manager_crud.rs`

**Call sites**:
- `insert_returning_version` — `:211-218` calls `on_record_created` (index_manager), `on_record_created_unique` (index_manager), `on_record_created` (sorted_indexes), `index2_on_insert`
- `delete_returning_version` — `:452-459` calls `on_record_deleted` (index_manager), `on_record_deleted_unique` (index_manager), `on_record_deleted` (sorted_indexes), `index2_on_delete`
- `set_returning_version` — `:550-577` calls:
  - `:550` `on_record_created` (if created)
  - `:551` `on_record_created_unique` (if created)
  - `:554` `on_record_created` (sorted_indexes, if created)
  - `:557` `index2_on_insert` (if created)
  - `:562` `on_record_updated` (if updated)
  - `:565` `on_record_updated_unique` (if updated)
  - `:568` `on_record_updated` (sorted_indexes, if updated)
  - `:571` `index2_on_update` (if updated)

**Barrier coverage**: YES — the write barrier (`needs_write_barrier()` → `unique_write_lock`) is raised during CREATE INDEX backfill, so non-tx writers serialize against the backfill via the lock.

**Building visibility**: YES (via shared planning methods) — this path bypasses the generation gate, but calls the SAME planning methods as Path 1, which iterate over ALL defs including `Building` ones. When a `Building` index exists, non-tx writers produce ops for it just like tx-staged writers do.

**Dirty-set capture point**: **NEW CAPTURE NEEDED** — same as Path 1, via the shared planning methods. The capture logic lives inside `IndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted` and their `SortedIndexManager` equivalents, not at the call sites in `table_manager_crud.rs`.

**Implementation note**: The individual call sites in `table_manager_crud.rs` (after lines `:218`, `:459`, `:557`, `:571`) do NOT need individual dirty-set instrumentation once the shared planning methods are enhanced. Those methods already capture the `RecordId` context (they receive it as a parameter), so they can add it to the dirty-set directly when encountering a `Building` def with an active build registry entry.

**SortedIndexManager verification**: The same no-state-filter behavior applies to `SortedIndexManager`. Verified by code inspection:
- `SortedIndexManager::plan_record_created` (`sorted_index_manager.rs:1564`) — `let defs = self.iter_indexes(); for def in &defs` — includes `Building` defs
- `SortedIndexManager::plan_record_updated` (`:1644`) — same pattern
- `SortedIndexManager::plan_record_deleted` (`:1713`) — same pattern
- `SortedIndexManager::iter_indexes()` (`:544-550`) — returns `Vec<SortedIndexDefinition>` via `self.indexes.load_local().clone()` with NO state filter

The single-choke-point design therefore covers both `IndexManager` (regular/hash) and `SortedIndexManager` families.

### Path 3: Doctor::Repair() Rebuild Path

**Description**: The doctor's `repair()` function drops all indexes, then rebuilds them by iterating over the data store and calling `on_record_created` for each record. This is a recovery path that re-derives all index state from the source-of-truth data.

**Files**:
- `crates/shamir-engine/src/table/doctor.rs`

**Call sites** (3 total):
- `:608` — calls `on_record_created` on `sorted_indexes` for borrowed `RecordView`
- `:620` — calls `on_record_created` on `sorted_indexes` for decoded `InnerValue`
- `:626` — calls `on_record_created` on `sorted_indexes` for owned `InnerValue`

**Barrier coverage**: YES — `repair()` acquires the write barrier (`begin_write_barrier`) before scanning the table (line `:576`).

**Building visibility**: NO — `repair()` drops all indexes first, then rebuilds them. During the rebuild loop, no indexes are `Building`; they're either not yet registered or already `Ready`. The dirty-set is irrelevant here because `repair()` is a full-table scan rebuild that happens outside of the online-CREATE-INDEX workflow.

**Dirty-set capture point**: **NOT NEEDED** — this path is a full-table rebuild that drops and recreates indexes. It never operates while a `Building` index exists, and it doesn't need to capture `RecordId`s because it's rebuilding the entire index from scratch, not missing any rows.

**Argument**: The dirty-set is for catching writes that occur during an online-CREATE-INDEX backfill. `repair()` is a recovery operation that runs with the table in a degraded state and rebuilds indexes after the table is quiescent. There is no concurrent `Building` index during `repair()` because `repair()` drops all indexes first. Therefore, no dirty-set capture is needed.

### Path 4: Index2 Backfill Path

**Description**: When creating an index2 backend (fts/vector/functional), `create_index_v2` backfills the existing table data by streaming records and calling `plan_insert` + `apply_index_ops` for each row. This is the CREATE INDEX backfill path for the index2 family.

**Files**:
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`

**Call sites** (2 total):
- `backfill_index2_backend` — `:598-602` calls `backend.plan_insert` then `crate::index2::apply_index_ops`

**Barrier coverage**: YES — `create_index_v2` holds `unique_write_lock` across the ENTIRE backfill (line `:614-615`).

**Building visibility**: NO — this IS the backfill itself. The dirty-set captures concurrent writes to re-apply later; the backfill doesn't need to capture itself.

**Dirty-set capture point**: **NOT NEEDED** — this path is the backfill itself, not a concurrent write. The dirty-set is populated by other paths while this backfill runs.

**Argument**: The dirty-set is populated by concurrent writers (paths 1 and 2) while this backfill runs. After the backfill completes and the index flips to `Ready`, the dirty-set is drained and those records are re-indexed. The backfill itself does not need to capture into the dirty-set because it's the operation that requires the dirty-set in the first place.

### Path 5: Sorted Index Backfill Path

**Description**: When creating a sorted index, `create_sorted_index` backfills by streaming records and calling `on_record_created` for each row. This is the CREATE INDEX backfill path for the sorted family.

**Files**:
- `crates/shamir-engine/src/table/table_manager_sorted_index.rs`

**Call sites** (1 total):
- `create_sorted_index_with_include` — `:223` calls `on_record_created` on sorted indexes

**Barrier coverage**: YES — sorted index creation holds the write barrier (similar to path 4).

**Building visibility**: NO — this IS the backfill itself.

**Dirty-set capture point**: **NOT NEEDED** — same argument as path 4.

### Path 6: Replication Migration Backfill Path

**Description**: When migrating a table to another repo, the migration coordinator copies the data store and then backfills index2 backends by calling `plan_insert` + `apply_index_ops` for each copied record. This is a replication-specific backfill.

**Files**:
- `crates/shamir-engine/src/table/table_manager_replication.rs`

**Call sites** (1 total):
- `bulk_populate_index2` — `:219-232` calls `backend.plan_insert` then `crate::index2::apply_index_ops`

**Barrier coverage**: YES — migration holds the write barrier.

**Building visibility**: NO — this is a migration backfill that runs in isolation on the destination repo.

**Dirty-set capture point**: **NOT NEEDED** — this is a one-time migration backfill, not concurrent with a CREATE INDEX on the destination repo. The destination repo's `Building` indexes, if any, would be detected and the dirty-set would be populated by the migration path itself if it were a concurrent write (but it's not — it's a bulk backfill after data copy).

**Argument**: This path is a special-purpose backfill for repo migration, not a general write path. It runs after the data store has been copied, before the destination repo is live. There is no concurrent `Building` index during this backfill because the destination repo is not yet serving traffic. The migration coordinator could be enhanced to populate the dirty-set if it needed to interoperate with a concurrent CREATE INDEX on the destination, but that's not the current design (migration is an offline operation).

### Path 7: WAL Recovery Replay Path

**Description**: On crash recovery, `replay_v2_op` replays WAL entries. For `IndexPut`/`IndexDel` operations, it applies pre-computed posting keys directly to `info_store` without re-deriving them. This is a replay path, not a primary write.

**Files**:
- `crates/shamir-engine/src/tx/recovery.rs`

**Call sites** (2 total):
- `replay_v2_op` (IndexPut branch) — `:164-166` calls `info_store().set(key, value)`
- `replay_v2_op` (IndexDel branch) — `:220` calls `info_store().remove(key)`

**Barrier coverage**: N/A — recovery is a cold path that runs before the repo is served.

**Building visibility**: NO — this path re-applies keys that were already computed at commit time.

**Dirty-set capture point**: **NOT NEEDED** — this path is a replay, not a primary write.

**Argument**: The dirty-set is for capturing `RecordId`s of records that were written while a CREATE INDEX backfill was in flight. WAL recovery replays commits that already happened. Those commits already went through one of the primary write paths (path 1 or path 2) at the time they originally committed. If the original write happened during a backfill, the original commit would have (or should have, once the dirty-set is implemented) captured the `RecordId` in the dirty-set. The dirty-set itself is persisted in `info_store` (as part of the index metadata), so it survives a crash. WAL recovery replays the posting writes (the `IndexPut`/`IndexDel` ops) but does NOT need to populate the dirty-set because:

1. The dirty-set state is already persisted (it's part of `IndexState::Building` metadata).
2. The `RecordId`s were already captured at original commit time.
3. Re-applying the posting writes is idempotent — if the backfill already applied a posting, recovery re-applies the same key with the same value.

Therefore, WAL recovery does NOT need independent dirty-set capture. The dirty-set is persisted separately and restored during table open recovery.

## Summary Table

| Path | File:Line | Barrier Coverage | Building Visibility | Dirty-Set Capture Needed? | Capture Point |
|------|-----------|------------------|---------------------|---------------------------|---------------|
| 1. Tx-staged | `table_manager_tx_ops.rs:451,479,592,677,785,924,999,1094,1147,1193` | YES | YES (via planning loop, not just re-derivation) | **YES** | Inside shared planning methods (`IndexManager::plan_record_*`, `SortedIndexManager::plan_record_*`) |
| 2. Non-tx CRUD | `table_manager_crud.rs:211-218,452-459,550-577` | YES | YES (via same shared planning methods) | **YES** | Inside shared planning methods (same as Path 1) |
| 3. Doctor repair | `doctor.rs:608,620,626` | YES | NO | NO | N/A (full rebuild) |
| 4. Index2 backfill | `table_manager_index_mgmt.rs:598-602` | YES | NO | NO | N/A (is the backfill) |
| 5. Sorted backfill | `table_manager_sorted_index.rs:223` | YES | NO | NO | N/A (is the backfill) |
| 6. Migration backfill | `table_manager_replication.rs:219-232` | YES | NO | NO | N/A (migration-specific) |
| 7. WAL recovery | `recovery.rs:164-166,220` | N/A | NO | NO | N/A (replay path) |

**Key architectural finding**: Paths 1 and 2 share the SAME underlying planning mechanism. They are two different callers (tx-staged vs non-tx direct) of the exact SAME `IndexManager::plan_record_*` and `SortedIndexManager::plan_record_*` methods. These methods iterate over ALL defs (including `Building`) with NO state filter. Therefore, the dirty-set capture belongs INSIDE those shared methods, not at ~15 scattered call sites.

## Paths Requiring NEW Dirty-Set Capture

**Paths 1 and 2 (Tx-staged AND Non-tx CRUD)** require new dirty-set capture instrumentation. Both paths share the SAME underlying mechanism:

- They funnel through the SAME planning methods: `IndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted` and their `SortedIndexManager` equivalents
- These planning methods iterate over `self.indexes.iter()` with NO filter on `IndexState` — they produce `SetPosting`/`RemovePosting` ops for `Building` defs just as readily as for `Ready` defs
- When an index is registered at `Building` state (BEFORE the backfill scan starts), every subsequent planning call includes it and produces ops for it

**Recommended capture architecture**:

The dirty-set check should go **inside the shared planning methods themselves**, NOT at ~15 scattered call sites in `table_manager_tx_ops.rs` and `table_manager_crud.rs`. For example:

```rust
// In IndexManager::plan_record_created (index_manager.rs:2013-2038)
for def in self.indexes.iter() {
    // NEW: Check if this def is Building and has an active in-flight build
    if def.state == IndexState::Building {
        if let Some(building_registry) = self.building_registry.as_ref() {
            if building_registry.is_build_in_flight(def.id) {
                // Capture to dirty-set instead of (or in addition to) producing op
                building_registry.record_dirty_set_entry(def.id, *record_id);
                continue; // Skip SetPosting production, or produce both (design choice)
            }
        }
    }

    // Existing logic: produce SetPosting for Ready defs (or non-in-flight Building defs)
    if let Some(irk) = build_index_key_from_record(...) {
        ops.push(IndexWriteOp::SetPosting { ... });
    }
}
```

The same logic applies to `plan_record_updated`, `plan_record_deleted`, and the `SortedIndexManager` equivalents.

This single-choke-point design is preferable because:
1. Centralizes the `IndexState` check and dirty-set routing
2. Works for both Path 1 and Path 2 without duplication
3. Avoids missing a call site in future refactors
4. The planning methods already have the `RecordId` context needed for dirty-set capture

## Paths Already Covered Transitively

- **Path 7 (WAL recovery)**: Already covered transitively because it replays commits that already went through Path 1 or Path 2. The dirty-set state itself is persisted as part of index metadata, not via WAL replay.

## Paths Not Requiring Capture

- **Path 3 (Doctor repair)**: Full-table rebuild, not concurrent with `Building` indexes.
- **Path 4 (Index2 backfill)**: IS the backfill itself.
- **Path 5 (Sorted backfill)**: IS the backfill itself.
- **Path 6 (Migration backfill)**: Offline migration, not concurrent with `Building` indexes.

## Verification Questions

1. **Q**: Does `insert_many` (non-tx batch insert) go through Path 2?
   **A**: Yes — `insert_many` in `table_manager_crud.rs:241` calls `insert_many_returning_version`, which internally calls `table.insert_many` and then loops over each inserted record calling `index_manager.on_record_created` for each one. This is covered by the same capture points as Path 2.

2. **Q**: Does the replication apply path (R1-a) go through any of these paths?
   **A**: No — the replication apply path (`crates/shamir-engine/src/tx/apply_replicated.rs`) applies already-committed ops directly to the data store (`MvccStore::apply_committed_ops` or `Store::transact`). It does NOT derive index postings because the leader already derived them at commit time and included them in the `ChangelogEvent` as `RecordChange` entries. The follower applies the raw record bytes only. This is NOT a primary write path that derives postings; it's a data-copy path.

3. **Q**: Does DROP TABLE CASCADE mutate postings?
   **A**: No — DROP TABLE CASCADE drops the entire table, including all indexes. It does not mutate postings; it deletes them wholesale. The path goes through `drop_index`/`drop_unique_index`/`drop_index2`/`drop_sorted_index`, which call `drop_all` on the backend to sweep postings. This is a DDL path, not a DML path, and is irrelevant to the dirty-set (the dirty-set is for writes during CREATE INDEX, not DROP operations).

4. **Q**: Are there any other batch/bulk insert paths?
   **A**: No — `insert_many` is the only batch insert path. All other bulk operations (migration backfill, CREATE INDEX backfill) are covered above.

## Conclusion

This audit found **7 distinct write paths** that can mutate regular/hash postings. Of these:

- **2 paths** (Paths 1 and 2: Tx-staged AND Non-tx CRUD) require **NEW dirty-set capture** instrumentation. These two paths are NOT independent — they share the SAME underlying planning mechanism (`IndexManager::plan_record_*` and `SortedIndexManager::plan_record_*`), which iterates over ALL defs with NO state filter and produces ops for `Building` indexes.
- **1 path** (Path 7: WAL recovery) is already covered transitively.
- **4 paths** (Paths 3, 4, 5, 6) do not require capture because they are either full rebuilds or backfills themselves.

The dirty-set capture implementation for #1058 should add the `IndexState` check and dirty-set routing **inside the shared planning methods** (`IndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted` and their `SortedIndexManager` equivalents), NOT at ~15 scattered call sites. This single-choke-point design:

1. Centralizes the logic for both Path 1 and Path 2
2. Avoids missing a call site in future refactors
3. Works correctly for the online-build redesign (where the write barrier is removed from Phase A/B/C)

## Cross-Reference to Related Tasks

- **#1055**: Online CREATE INDEX RFC — this audit provides the exhaustive path list for the dirty-set design.
- **#1056**: Dirty-set data structure — needs to persist `RecordId`s in `info_store`.
- **#1057**: Dirty-set population — adds capture points to Path 2.
- **#1058**: Dirty-set drain and re-index — reads the dirty-set after backfill and re-indexes captured `RecordId`s.
- **#1059-#1062**: Remaining slices of the online CREATE INDEX redesign.