# Brief — P0-3b follow-up: index2 (fts/functional/vector) DROP INDEX crash-safety

Task: #988 in the session TaskList. Source: this exact gap was identified
and DELIBERATELY DEFERRED by #972 (P0-3b), whose own brief
(`docs/dev-artifacts/prompts/release-blockers/04-p0-3b-sorted-index2-drop-durability.md`)
scoped it out, and re-confirmed as still-open by a post-campaign `@oh`
review (2026-08-04/05). **The current code already contains an
unusually detailed deferral doc — read it in full before writing any
code**: `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`,
`TableManager::drop_index2`'s doc comment (~line 745-828, especially the
"CRASH-SAFETY GAP" section ~line 771-813). This brief operationalizes that
doc's own recommended plan (option (a), the tombstone approach) — do not
redesign, just implement what's already been scoped.

## What's already true — verified this session, do not re-derive

- **The gap**: `TableManager::drop_index2` (`table_manager_index_mgmt.rs:829`)
  does retire (`index2_registry.remove_by_id`) → sweep
  (`backend.drop_all()`) → persist (`save_index2_metadata`), with NO durable
  tombstone. A crash between sweep and persist leaves on-disk metadata still
  listing the index as `Ready` with zero postings — on restart the planner
  routes queries to a dead index, silently returning wrong (empty) results.
- **The proven pattern to mirror**: the SORTED family
  (`crates/shamir-index/src/base_index/sorted_index_manager.rs`, landed by
  #972) already solved the IDENTICAL problem: a durable tombstone
  (`system:sidx_drop`, a bincode `Vec<u64>` of names-in-progress) written
  BEFORE the sweep, cleared AFTER the persist, with an open-time
  `recover_in_progress_drops()` that resumes any interrupted drop
  (idempotent — safe to run twice) plus a namespace-reuse guard in
  `register()` rejecting a CREATE for a name still in the tombstone set.
  Read `sorted_index_manager.rs` lines 583-982 (specifically
  `add_to_dropping_sorted`, `clear_from_dropping_sorted`,
  `recover_in_progress_drops`, `drop_index`, and the guard inside
  `register`) as your exact template — the shapes should be near-identical,
  adapted for index2's different storage architecture (below).
- **Key architectural difference from sorted (verified this session)**:
  `SortedIndexManager` owns its own `info_store` directly, so its tombstone
  helpers are plain `&self` methods. `IndexRegistry`
  (`crates/shamir-index/src/registry.rs`) does NOT own an `info_store` — it
  is a pure in-memory lock-free registry (`scc::HashMap`-based). Index2's
  existing persistence (`save_index2_metadata`/`load_index2_metadata`,
  `crates/shamir-index/src/persistence.rs`) is already shaped as FREE
  FUNCTIONS taking `(&IndexRegistry, &Arc<dyn Store>)` explicitly, not
  struct methods. **Follow that existing shape**: add the new tombstone
  functions as siblings in `persistence.rs` (`add_to_dropping_index2`,
  `clear_from_dropping_index2`, `load_dropping_index2`), each taking
  `info_store: &Arc<dyn Store>` explicitly — do NOT add an `info_store`
  field to `IndexRegistry` just to mirror sorted's method-based shape.
- **The tombstone key — already vetted, use verbatim**: `"_m.idx.drop"`
  (11 bytes). Verified safe against `RecordId::system`'s 12-byte truncation:
  distinct from the two existing index2 keys `"_m.idx"`
  (`meta_key_indexes`, `persistence.rs:39`) and `"_m.idx.lfv"`
  (`meta_key_legacy_index_version`, `persistence.rs:44`) — none share the
  same first-12-bytes prefix. The tombstone value is `Vec<u32>` (descriptor
  **ids**, not `name_interned` — index2 backends are keyed by compact `u32`
  id, unlike sorted/base_index which use `name_interned`; verify this
  against `IndexDescriptor`'s `id` field before implementing).
- **Where the recovery hook goes**: `TableManager::new`/`create`
  (`crates/shamir-engine/src/table/table_manager.rs`, ~line 393-477) already
  has the F-50 Step 3b "Building-state self-heal" block that loads
  `load_index2_metadata`, iterates descriptors, self-heals any `Building`
  one, and re-persists if anything changed. The new drop-recovery call goes
  RIGHT AFTER this block (after line ~477's closing `}`), reading the
  tombstone and resuming any interrupted drop — mirroring exactly where
  sorted's `recover_in_progress_drops()` is called from the same
  `TableManager::new` (search for `recover_in_progress_drops` in
  `table_manager.rs` to find the sorted call site and match its relative
  position/ordering — e.g. does it need to run before or after index2's
  restore_on_open loop that follows at ~line 479-495? Read both blocks and
  decide based on the same reasoning sorted's placement used).
- **Namespace-reuse guard location**: `create_index_v2`
  (`table_manager_index_mgmt.rs:24`) is the index2 creation entry point.
  Before it proceeds to register/backfill a new backend, add a check
  mirroring sorted's `register()` guard (`sorted_index_manager.rs:589-600`):
  if the target name (or its would-be id — verify which is the right check
  given index2's id-based tombstone) is in the dropping set, reject with a
  clear error ("a DROP INDEX for this name is still in progress").

## Required work

1. **Tombstone read/write functions** in `crates/shamir-index/src/persistence.rs`:
   - `add_to_dropping_index2(ids: &[u32], info_store: &Arc<dyn Store>) -> DbResult<()>` (or single-id, match whatever shape reads most naturally against the call site — sorted's is single-name per call)
   - `clear_from_dropping_index2(id: u32, info_store: &Arc<dyn Store>) -> DbResult<()>`
   - `load_dropping_index2(info_store: &Arc<dyn Store>) -> DbResult<Vec<u32>>`
   - Follow `save_index2_metadata`/`load_index2_metadata`'s existing error handling / bincode-encoding conventions in the same file.

2. **`TableManager::drop_index2`** (`table_manager_index_mgmt.rs:829`): add the tombstone write BEFORE `remove_by_id`, with rollback-on-persist-failure (mirror sorted's `add_to_dropping_sorted` failure handling — the write itself is the "MUST succeed before proceeding" step). Clear the tombstone AFTER `save_index2_metadata` succeeds, at the very end. Update the method's own doc comment (the "CRASH-SAFETY GAP" section) to state the gap is now CLOSED, describing the final sequence — do not just delete the old doc, transform it into an accurate "how this is now safe" account, following the style of `sorted_index_manager.rs::drop_index`'s doc.

3. **Recovery hook** in `TableManager::new`/`create` (`table_manager.rs`): after the F-50 Step 3b self-heal block, load the index2 tombstone and for each id still listed: if the registry still has it (crash happened before `remove_by_id`'s effect was visible... actually verify — `remove_by_id` and the tombstone-clear are separate persisted things; walk through the actual crash-point matrix analogous to sorted's documented 3-row table at `sorted_index_manager.rs:786-790`, adapting it for index2's specific steps) — retire/sweep/persist as needed, idempotently, then clear the tombstone. Write this crash-state matrix as a doc comment on the new recovery function, matching sorted's exact table format.

4. **Namespace-reuse guard** in `create_index_v2`: reject creation of a name/id still in the dropping set with a clear `DbError` message.

5. **Tests**: mirror #972's own test file pattern for sorted crash-safety
   (find it — likely `p0_3b_sorted_drop_crash_tests.rs` or similar under
   `shamir-engine`'s or `shamir-index`'s test directories, search for tests
   referencing `sidx_drop`/`recover_in_progress_drops` to find the exact
   file). You will need a pause-hook test seam for index2's drop path — the
   code ALREADY HAS ONE: `drop_index2_pause_hook`
   (`table_manager_index_mgmt.rs:852`, currently used for the F-76
   concurrent-reader test). Check whether it parks at the right point for
   YOUR new crash-recovery tests (it currently parks "after registry
   removal, before sweep" — you likely need additional pause points, or can
   reuse this one plus a new one after-sweep-before-persist, mirroring
   whatever granularity sorted's own tests use). Write tests for each crash
   point in your matrix: crash after tombstone-write/before sweep, crash
   after sweep/before persist, crash after persist/before tombstone-clear —
   each proving `TableManager::new` (simulating a fresh reopen) reaches a
   consistent state (index gone, no orphan postings, tombstone cleared).
   Also add the register-time namespace-reuse-guard test (mirroring
   whatever sorted's own guard test looks like).

## Scope discipline

- Do NOT touch the base_index (#959) or sorted (#972) crash-safety
  mechanisms — this task only extends the SAME pattern to index2.
- Do NOT redesign — option (a) (tombstone) was already chosen by the
  existing doc comment as "the lower-risk, pattern-consistent choice for a
  release"; do not switch to option (b) (state-based).
- Do NOT touch `create_index_v2`'s backfill/registration logic beyond
  adding the namespace-reuse guard check.
- If you find the crash-point matrix has MORE distinct cases than sorted's
  3-row table (index2 has an extra "pending" mechanism sorted doesn't —
  verify whether that interacts with this gap at all, or is orthogonal),
  document them precisely rather than forcing a 3-row match.

## Gate (MANDATORY)

```
cargo fmt -p shamir-index -p shamir-engine -- --check
cargo clippy -p shamir-index -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-index -p shamir-engine --full
```

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Show the exact tombstone key used and confirm the 12-byte-truncation
collision check yourself (don't just trust this brief — verify against the
CURRENT `persistence.rs` contents at the time you make the change, in case
a third key was added since this brief was written). Show the crash-point
recovery matrix you implemented (as a doc comment, mirroring sorted's
table). List every new test and which crash point each proves. Give exact
gate command output.
