# Brief — #1048: P1-2 sub-slice B — thread op_id through DDL tombstones and write recovery status

## Context

S.H.A.M.I.R. Database. Continuation of #1015 (P1-2, DDL result contract —
wire types, op-status log, `admin_result_with_op_id` stamping, SDK methods
— all merged). This is sub-slice B, deferred by #1015's own delegate as a
separate logical slice. Full spec: `docs/dev-artifacts/research/
2026-08-05-ddl-result-contract-rfc.md` §2.2, §3.3, §4 п.1 — **read this
RFC in full before starting**, it is the primary source for this task; this
brief only anchors you to the current tree's exact state.

**The gap**: `op_id` (minted at DDL dispatch time in `crates/shamir-db/src/
shamir_db/execute/admin_table_index.rs`'s `handle_drop_index`/
`handle_rename_index`, e.g. `RecordId::system(&format!("ddl_drop_index_{}",
...))`) currently only reaches the SYNCHRONOUS op-status log write on the
happy path (#1015's sub-slice A). It is NEVER threaded into the CRASH
RECOVERY path — so if a DDL op crashes mid-flight and recovery resumes it
later, the recovered completion is never recorded against the SAME `op_id`
a client may still be polling via `GetDdlOpStatus`. The client would poll
forever (or time out) for an op that, from the server's perspective,
actually succeeded via crash recovery.

## Already investigated — exact current state, verified by reading the code

1. **`HashRenameTombstone`** (`crates/shamir-index/src/base_index/
   index_manager.rs:95-106`) already HAS the field:
   ```rust
   pub struct HashRenameTombstone {
       pub old_name: String,
       pub new_name: String,
       pub paths: Vec<String>,
       pub op_id: Option<String>,  // ← exists, but every construction site hard-codes None
   }
   ```
   Both real construction sites (`crates/shamir-engine/src/table/
   table_manager_index_mgmt.rs:1672` and `:1748`, inside `rename_index`'s
   two `add_to_renaming(...)` calls — one for the "create-first" path, one
   for the "barrier-held" path) write `op_id: None` literally. Confirm this
   yourself (`grep -n "op_id: None" crates/shamir-engine/src/table/
   table_manager_index_mgmt.rs`).

2. **`TableManager::rename_index`'s signature has no `op_id` parameter at
   all** (`table_manager_index_mgmt.rs:1573`: `pub async fn rename_index(&self,
   old_name: &str, new_name: &str) -> DbResult<()>`) — the op_id minted in
   `admin_table_index.rs`'s `handle_rename_index` is NOT currently passed
   down to this function. Threading it through requires changing this
   signature (and its one caller in `admin_table_index.rs`) — this is the
   core mechanical work, not just a tombstone-field fill-in.

3. **The three recovery functions in scope**, located and confirmed:
   - Hash DROP INDEX (regular+unique): `IndexManager::recover_in_progress_drops`
     (`crates/shamir-index/src/base_index/index_manager.rs:1009`).
   - index2 DROP INDEX: `TableManager::recover_index2_drops`
     (`table_manager_index_mgmt.rs:1114`).
   - Hash RENAME INDEX (regular+unique, including the RFC §2.3 SEVERE
     case): `TableManager::recover_hash_renames`
     (`table_manager_index_mgmt.rs:1245`).
   (Sorted family's own `recover_in_progress_drops`,
   `crates/shamir-index/src/base_index/sorted_index_manager.rs:828`, is
   OUT OF SCOPE for this task per the task's own family list — do not
   touch it.)

## What to implement

1. **Thread `op_id` from dispatch to tombstone construction**, for all
   three families:
   - `TableManager::rename_index`'s signature gains an `op_id:
     Option<RecordId>` (or whatever type the wire `op_id` actually is —
     check `admin_table_index.rs`'s existing `RecordId::system(...)`
     minting and #1015's `QueryResult::op_id` field type, match it
     exactly) parameter, threaded from its one caller
     (`admin_table_index.rs::handle_rename_index`) into both
     `HashRenameTombstone { op_id, ... }` construction sites (replacing
     the hard-coded `None`).
   - The equivalent threading for hash DROP INDEX's own tombstone type
     (find it — likely a sibling of `HashRenameTombstone` used by
     `add_to_dropping`/`drop_index`, check `index_manager.rs` near where
     `HashRenameTombstone` lives) and for index2 DROP's tombstone type
     (`crate::index2::persistence::add_to_dropping_index2`, referenced in
     #1037/#1038's own commits this session).
2. **Each of the three recovery functions writes
   `DdlOpState::SucceededViaCrashRecovery`** (confirm this exact variant
   name exists in `crates/shamir-query-types/src/read/ddl.rs`'s
   `DdlOpState` enum — #1015 built this type, verify it has a crash-recovery
   variant distinct from the synchronous `Succeeded`) to the op-status log
   under the SAME `op_id` the tombstone carried, when it resumes and
   completes an in-flight op it found via tombstone. If the tombstone's
   `op_id` is `None` (e.g. a tombstone written by a pre-#1048 binary,
   or a legitimate non-DDL-tracked path), skip the status write — do not
   error, this must stay backward-compatible with tombstones that predate
   this field's use.
3. **Retire #967's enriched-error-TEXT sites for these three families** —
   find them (`grep -rn "#967"` in the three files above) and convert them
   to the structured `DdlOpState::Failed { detail }` shape #1015 already
   established for the synchronous path, instead of a free-text error
   string. Confirm `Failed`'s exact shape in `ddl.rs` before implementing.

## Tests

- **End-to-end crash-recovery test — mandatory, reuse the existing test
  seam.** `IndexManager::maybe_pause_rename_mid()` (referenced in
  `table_manager_index_mgmt.rs` around the tombstone-write site) is the
  existing pause hook `#997`'s own recovery tests already use
  (`p997_hash_rename_durability_tests.rs` — read this file for the
  established pattern). Write the RFC §2.3 "worked example" scenario as a
  real test: mint an op_id, start a RENAME, park it mid-flight via the
  pause hook (simulating a crash), "restart" (drop and recreate the
  `TableManager`/reopen), let recovery resume and complete the rename, then
  poll `GetDdlOpStatus` for the SAME op_id and assert it now reads
  `SucceededViaCrashRecovery` — not `InProgress`, not absent.
- Equivalent recovery-completes-and-status-writes tests for hash DROP
  INDEX and index2 DROP INDEX, using their own existing pause-hook test
  seams (find them — `p03b`/`p972`-tagged test files or similar, mirroring
  #997's pattern for rename).
- A backward-compatibility test: a tombstone with `op_id: None` (simulating
  a pre-#1048 crash) recovers successfully WITHOUT attempting an op-status
  write and without erroring.
- A test for the #967 error-TEXT retirement: a recovery path that fails
  produces a structured `DdlOpState::Failed { detail }`, not a bare string
  error — assert the structured shape, not just that SOME error occurred.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- This changes public-ish `TableManager` method signatures
  (`rename_index` and whatever the DROP-family equivalents are) — audit
  every caller (test files included) and update them, don't leave a
  compile break.
- Gate: `cargo fmt -p shamir-index -p shamir-engine -p shamir-db -p
  shamir-query-types -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `./scripts/test.sh -p shamir-index -p shamir-engine -p
  shamir-db -p shamir-query-types --full`. Use the wrapper, never raw
  `cargo test`/`cargo nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] `op_id` threaded from DDL dispatch through to tombstone construction
      for all three families (hash DROP, hash RENAME, index2 DROP) —
      `None` literals replaced with the real minted `op_id`.
- [ ] All three recovery functions write
      `DdlOpState::SucceededViaCrashRecovery` under the tombstone's op_id
      when it's present, skip silently when it's `None` (backward compat).
- [ ] #967 enriched-error-TEXT sites for these three families retired to
      structured `DdlOpState::Failed { detail }`.
- [ ] End-to-end crash-recovery test for the RFC §2.3 worked example
      (rename), reusing `maybe_pause_rename_mid()`.
- [ ] Equivalent recovery tests for hash DROP and index2 DROP.
- [ ] Backward-compat test for a `None`-op_id tombstone.
- [ ] fmt/clippy/test gates green, real output reported.
