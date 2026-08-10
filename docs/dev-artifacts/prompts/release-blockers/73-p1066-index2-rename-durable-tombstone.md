# Brief 73 — #1066 (P0): index2 RENAME is not atomic — add a durable rename tombstone + recovery, mirroring the hash/sorted RENAME pattern

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## The defect

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`, the
`is_index2` branch inside the RENAME dispatch function (currently around
lines 2288–2311 — line numbers WILL have shifted since this brief was
written if other work landed first; find it by searching for
`if is_index2 {` in that file, it's the last of four family branches
— regular hash, unique hash, sorted, then index2 — inside one function):

```rust
if is_index2 {
    let (_barrier, _uwl_guard) = self
        .begin_write_barrier(crate::index::write_barrier_flags::INDEX2_CREATE)
        .await;
    let ok = self
        .index2_registry
        .rename_entry(old_id, new_name.to_string(), new_id)
        .await;
    if !ok {
        return Err(...);
    }
    crate::index2::persistence::save_index2_metadata(
        &self.index2_registry,
        &self.info_store,
    )
    .await?;
}
```

`rename_entry` (step 1) mutates the LIVE registry in memory (both the
`by_name` map and the authoritative name slots inside the `by_id` entry —
see `crates/shamir-index/src/registry.rs:539` onward, `IndexRegistry::rename_entry`).
`save_index2_metadata` (step 2) persists it, with a `?` that propagates any
error straight out.

**There is no tombstone, no pending-state marker, and no rollback between
these two steps.** The write barrier (`begin_write_barrier(INDEX2_CREATE)`)
IS held across both steps — this is NOT a race against concurrent DDL
(R0-A/#1012 already closed that). It is a **non-atomicity between two
sequential mutations under a storage error or task cancellation**: if
`save_index2_metadata` returns `Err` (a real storage failure) or the
enclosing future is cancelled between the two steps, the in-memory registry
now has the NEW name but the disk still has the OLD name. The process
returns an error with no usable `op_id` response. On restart, the renamed
descriptor reverts to its old name (disk wins on reload) — but until then,
runtime and disk are silently divergent.

**Every other index family already has a durable rename mechanism for this
exact window** — this is the asymmetry the task exists to close:
- Hash regular/unique RENAME: `IndexManager::add_to_renaming` writes a
  `HashRenameTombstone { old_name, new_name, paths, op_id }` BEFORE the
  drop+create sequence (see this same file, ~line 2108–2124, the unique
  branch right above the one you're fixing — read it as your template),
  and `clear_all_renaming` clears it after. Recovery
  (`table_manager.rs`/`table_manager_index_mgmt.rs`, search for the hash
  RENAME recovery block) replays or reconciles from this tombstone on
  restart.
- Sorted RENAME: `rename_index_sorted` has its own durable path (see the
  `is_sorted` branch immediately above `is_index2` in this same function).
- index2 DROP already has a durable tombstone: `add_to_dropping_index2` /
  `load_dropping_index2` / `clear_from_dropping_index2`
  (`crates/shamir-index/src/persistence.rs:357–401`) — written BEFORE the
  posting sweep, cleared AFTER `save_index2_metadata` persists the
  reduction. This is your exact template, adapted for rename instead of
  drop.
- index2 RENAME has nothing. This is the gap.

## The fix — a durable rename tombstone for index2, mirroring `add_to_dropping_index2`

### 1. New persistence functions in `crates/shamir-index/src/persistence.rs`

Add a rename-tombstone triple mirroring `add_to_dropping_index2` /
`load_dropping_index2` / `clear_from_dropping_index2`
(`persistence.rs:357–401`) exactly in style — free functions (not methods
on `IndexRegistry`, since it doesn't own an `info_store`, per that existing
code's own comment), same bincode-vec-under-one-key persistence shape, same
backward-compat-tolerant decode pattern `load_dropping_index2` uses (try
new format, fall back gracefully — though for a brand-new key you don't
need a legacy fallback, just document that this key is new as of this
task):

- `meta_key_indexes_rename() -> RecordId` — new system key, follow
  `meta_key_indexes_drop()`'s naming convention (`persistence.rs:313`) for
  the literal key string.
- `load_renaming_index2(info_store: &Arc<dyn Store>) -> Result<Vec<(u32, String, String, Option<String>)>, DbError>`
  — `(id, old_name, new_name, op_id)`.
- `add_to_renaming_index2(id: u32, old_name: String, new_name: String, op_id: Option<String>, info_store: &Arc<dyn Store>) -> Result<(), DbError>`
  — MUST be called BEFORE `rename_entry` mutates the live registry, so a
  crash/error at any point after this call is recoverable from the
  tombstone.
- `clear_from_renaming_index2(id: u32, info_store: &Arc<dyn Store>) -> Result<(), DbError>`
  — MUST be called AFTER `save_index2_metadata` durably persists the
  renamed registry.

### 2. Wire the tombstone into the `is_index2` RENAME branch

In `table_manager_index_mgmt.rs`'s `is_index2` branch:

1. Resolve the descriptor's u32 id BEFORE mutating anything — the same
   `self.index2_registry.get_by_name(old_id)` call already used earlier in
   this function to compute `is_index2` (around line 1891) returns
   something with `.id()` giving you the u32; reuse that resolution (don't
   re-derive it a different way).
2. Write the tombstone via `add_to_renaming_index2` — BEFORE
   `rename_entry` — with the resolved id, the old/new string names (you
   already have `old_name`/`new_name` as `&str` params to this function —
   confirm from the function signature), and `op_id.as_ref().map(|id| id.to_string())`
   (the function already receives `op_id: Option<RecordId>` as a
   parameter — check its exact name in this function's signature, mirror
   how the unique-hash branch above passes it into
   `HashRenameTombstone { op_id: op_id.as_ref().map(|id| id.to_string()), .. }`).
3. If the tombstone write itself fails, propagate the error and do NOT
   proceed to `rename_entry` — nothing has moved yet, so returning the raw
   error is safe (mirrors `add_to_dropping_index2`'s own doc: "If the
   persist fails, the on-disk tombstone is unchanged — the caller
   propagates the error and does NOT proceed").
4. Keep `rename_entry` and `save_index2_metadata` exactly as they are now,
   in the same order.
5. After `save_index2_metadata` succeeds, call `clear_from_renaming_index2`
   for the resolved id. If clearing fails, log loudly
   (`log::error!`, not swallowed) but do NOT fail the whole rename — the
   rename itself already succeeded and is durable; a stale-but-harmless
   tombstone will be reconciled by recovery on next restart (same
   reasoning `add_to_dropping_index2`'s sibling `clear_from_dropping_index2`
   callers already use elsewhere in this file — grep for how the index2
   DROP path handles a clear failure and match that convention).

### 3. Recovery — reconcile a stale index2 rename tombstone on table open

Add a `recover_index2_renames` counterpart to the existing
`recover_index2_drops` (same file, `table_manager_index_mgmt.rs` — find it
via `pub(crate) async fn recover_index2_drops`) and wire it into
`TableManager::create` at the same point index2 DROP recovery and hash
RENAME recovery already run (`crates/shamir-engine/src/table/table_manager.rs`,
search for where `recover_index2_drops().await?` and the hash-rename
recovery block are called — add your new call in the same sequence,
respecting whatever ordering comment already explains why these run in
the order they do).

Recovery semantics — on restart, for each `(id, old_name, new_name, op_id)`
entry in `load_renaming_index2`:
- If the registry's live/persisted state already reflects `new_name` for
  that id (the rename fully completed, only the tombstone-clear was lost)
  → this is the common case after a crash between `save_index2_metadata`
  and `clear_from_renaming_index2`. Just clear the tombstone (idempotent
  finish).
- If the registry still shows `old_name` for that id (crash happened
  before `save_index2_metadata` completed, or before it even ran) → re-run
  the rename to completion: `rename_entry` + `save_index2_metadata`, THEN
  clear the tombstone. This makes the operation crash-restartable rather
  than silently reverted.
- Log which case triggered (`log::info!`), same style as
  `recover_index2_drops`'s own completion log
  (`"P0-3b (#988): recovery complete — {} index2 DROP(s) finalized"`) —
  write an analogous message for renames.

Do NOT write any `DdlOpStatus`/`ddl_op_log` entries as part of this
recovery — index2 RENAME's op-status integration (family classification,
missing op_id plumbing) is explicitly out of scope here and tracked
separately as task #1067 (blocked on #1065, which is now done — #1067 is
the next task after this one, not this one). Keep this task scoped to
atomicity/durability only, per its own description.

## Tests — required, follow the established crash-injection pattern for this exact family of bug

Per this task's own description: inject an error on EVERY storage write
and a cancellation after EVERY `.await` in the `is_index2` RENAME branch,
and confirm runtime and disk converge on the SAME name after restart in
every case. Copy the pattern from
`crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs`
(the equivalent matrix for hash RENAME) for structure, and
`crates/shamir-engine/src/table/tests/p1048_index2_drop_durability_tests.rs`
(cited in this task's own description at lines 163–170) for the actual
race mechanics against index2 specifically.

**⚠️ Do NOT use `tokio::spawn` + `drop(JoinHandle)` to simulate a crash** —
dropping the handle does not cancel the spawned task; this hangs the test
for the full 180s nextest timeout (the exact trap task #1048 hit before).
The correct pattern is `tokio::select!` racing the real operation against a
pause hook, per `p1048_index2_drop_durability_tests.rs:163–170` — read that
code directly and copy its shape, including whatever pause-hook seam
already exists on the index2 rename path or needs to be added (check first
whether one already exists before adding a new one — search for existing
`*pause_hook*` seams touching `rename_entry`/`is_index2` before assuming
none exists).

Minimum required tests:
1. Tombstone written BEFORE `rename_entry` mutates the live registry — race
   and assert the tombstone is durably present while the registry still
   shows the OLD name.
2. Crash/error injected between tombstone-write and `rename_entry` — after
   "restart" (re-open the table from the same store), both registry and
   disk show the OLD name, tombstone is reconciled/cleared, no data loss.
3. Crash/error injected between `rename_entry` and `save_index2_metadata`
   — after restart, recovery completes the rename (registry and disk both
   show NEW name), tombstone cleared.
4. Crash/error injected between `save_index2_metadata` and
   `clear_from_renaming_index2` — after restart, recovery just clears the
   stale tombstone (rename already fully durable), no double-execution, no
   error.
5. Happy path (no injected failure) still works exactly as before —
   regression guard.
6. Rename each of the three index2 kinds at least once across the matrix
   (fts, functional, vector) if the existing test fixtures make this cheap
   — if not, one representative kind (pick whichever
   `p1048_index2_drop_durability_tests.rs` already has fixtures for) is
   acceptable, but say explicitly which kind(s) you covered and why.

**Every test must FAIL on code lacking the mechanism it proves** — this
codebase's established convention. For at least one crash-injection test,
verify this yourself by temporarily removing the tombstone write and
confirming the test goes red, then restoring the fix.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
```

Paste the actual final summary line from each `./scripts/test.sh`
invocation (pass/fail counts) — literal output, not a paraphrase. List
every test you wrote by name with individual pass/fail status, and the
outcome of the mandatory revert-and-check self-verification. If anything
fails, fix it before reporting done. This codebase's #1065 task went
through 4 rounds because earlier attempts self-reported success that
direct verification disproved (untested claims, dead/uncompiled test
files, an unrun gate) — the standard here is that everything you report
is something you personally watched pass, with the command's actual
output as evidence, not a paraphrase or an assumption.
