# P34-03c revision 2 — static review 1

Scope: commit `b44133612` (HEAD) on top of the docs commits. Brief:
`docs/dev-artifacts/prompts/remediation/p34/03c-revision-2.md`. Method: I
compiled the code by reading it. No cargo, rustc, clippy, rust-guard, layout
script or test was run. Every changed item was checked against the code it
calls, at that code's current definition.

## Verdict

**Ready for CI (draft PR, fast tier) after the MINOR fixes, which are
optional.** I found no BLOCKER and no MAJOR code defect. S1–S5 match the brief.
Lock order and cancellation look correct, and every acceptance oracle can be
satisfied as written, with the two runtime caveats under "Could not verify".
The one item that needs a decision from the orchestrator is design-level
(D1): it follows from the answer to Q11 and is allowed by the brief, but it is
missing from the failure-mode table.

## Findings, by severity

### BLOCKER

None.

### MAJOR

None in the code.

**D1 (design, allowed by the brief, needs an explicit decision): the visibility
wait holds the repository permit, and some version holders wait for that same
permit.** See Q11. When such a holder exists, every rename attempt:

- holds the repository permit for the full `RENAME_VISIBILITY_WAIT` (30 s in
  production). During that time all DDL on the repository, every cold
  `get_table` (the first access to any dormant table), `RecordCounter::persist`
  / `flush_metadata`, follower receipt apply and drainer orphan replay are
  blocked;
- rejects writers to the source with `MigrationFrozen` for the whole 30 s;
- then fails with `Conflict("rename visibility wait timed out ...")`.

On a follower with parked receipt guards and no further receipts, a rename can
never succeed until a receipt publishes.

This is bounded and never a hang, as the brief intended. It is still an
availability cost that M1–M7 do not list.

Proposal: add it as M8 in the note. Optionally lower the production timeout
(for example 5 s). Or fail fast before waiting: in `await_visible`, check
`gate.visible_floor() >= target`, and fail with the timeout `Conflict` when the
gate reports post-WAL orphans or parked guards. That second option needs a
`shamir-tx` accessor, which is out of this stage's scope, so it belongs to 03d.

### MINOR

1. **The destination-exists check now runs after the freeze and the
   visibility wait.**
   - Location: `catalog.rs:391-402` and `:422-433`. The check itself is in
     `prepare_table_rename_admitted` at `:455`.
   - Effect: renaming onto an existing name now freezes and drains the source
     and waits for visibility first. Under an unsettled version it returns the
     visibility `Conflict` instead of `Validation("Table 'b' already exists")`.
   - It also instantiates a dormant source (`get_table_admitted`), which runs
     open-time recovery inside the rename. Master did not do this. I found no
     test that depends on the old behaviour, including
     `rv_db_72e8717c_rename_reads_table_catalogue_once_for_many_siblings`,
     because `create_table_context` reads no system catalogue.
   - Cheap fix in both transitions, right after the `from` precheck:
     ```rust
     if !self.configs.contains_key(from) {
         return Ok(false);
     }
     if self.configs.contains_key(to) {
         return Err(DbError::Validation(format!("Table '{to}' already exists")));
     }
     ```
     Keep the existing re-check in `prepare_table_rename_admitted`.

2. **The cancellation tests can poll a completed future.**
   - Location: `cancel_tests.rs:48-53` and `:66-75`.
   - If the rename returns `Ready(Err(..))` before reaching `persist` or the
     freeze, the next `poll!` panics with "`async fn` resumed after
     completion". That hides the real error.
   - Fix (add `use std::task::Poll;` at the top of the file):
     ```rust
     for _ in 0..100 {
         match poll!(rename.as_mut()) {
             Poll::Ready(result) => panic!("rename completed early: {result:?}"),
             Poll::Pending if entered.load(Ordering::Acquire) => break,
             Poll::Pending => tokio::task::yield_now().await,
         }
     }
     ```
     Apply the same shape to (b), with the freeze-bit predicate.

3. **The visibility test reads a row whose version is above the settled floor.**
   - Location: `visibility_wait_tests.rs:55`.
   - `source.get(id)` goes through `MvccStore::get_current_bytes`
     (`shamir-tx/src/mvcc_store/history/version_entry.rs:346`). It uses the
     current-version cell when one exists, so the read is probably fine. But if
     that cell filters by the floor, `get` returns `NotFound` while the guard is
     held.
   - Low-risk alternative: assert the receipt is `Pending` only, and move the
     `get` after `drop(version)`.

4. **`begin_migration_freeze` on a detached handle returns
   `CommitError::Storage(DbError::MigrationFrozen{..})`, not
   `CommitError::MigrationFrozen`.**
   - Location: `ddl_barriers.rs:91`, via the new check in `admit`.
   - Harmless: `From<TxError> for DbError` (`commit/entry.rs:421-439`) turns
     both into `DbError::MigrationFrozen`.
   - Optional fix: map that error in `begin_migration_freeze`.

5. **A stale handle reports wire code `table_migrating`**
   (`shamir-storage/src/core/error.rs:292`).
   - For a handle on a table that was renamed away, "migrating" is misleading.
     An embedded client that retries on that code would loop forever.
   - The brief chose this variant, so it is recorded only. Revisit it with the
     R10 wire-code work in 03d.

6. **The Q12 inventory in the note is incomplete.**
   - `rg -l "rename_table_stores|rename_table_as|rename_table\(|recover_table_rename_with" crates`
     also returns four files the note does not list:
     - `shamir-db/src/shamir_db/tests/core/ddl_namespace_lock_tests.rs`
     - `shamir-db/src/shamir_db/tests/ddl_contracts/unique_store_names_tests.rs`
     - `shamir-db/tests/rename_migration/renames/rename_table_durability.rs`
     - `shamir-server/src/sync/replication_seed/install/tests/disk_tests.rs`
   - I read them. None of them writes or runs DDL through a pre-rename handle
     after a successful rename:
     - `unique_store_names_tests.rs:48-63,275-305` only reads through the old
       handle before the rename.
     - `disk_tests.rs:131-153` holds `rows` across `rows->renamed->rows` but
       only reads it before the first rename.
   - So the conclusion holds. Only the note's inventory is wrong.

7. **Incidental edits.**
   - `prelock.rs:788-789`: the blank line before `Ok(PreLockResult {` was
     removed. The note claims it was restored.
   - `write_domain_index_core.rs:133-135`: `ensure_attached` now sits between
     two identical `debug_check_permit` calls. The duplicate already existed.
     This is cosmetic; the check is still correct.

8. **The note is inconsistent.**
   - `## Progress`, `## Design` and `## Audit append` each appear four times.
   - "Test evidence" says the visibility test probes with buffer-config DDL,
     but `visibility_wait_tests.rs:57-60` uses `create_index`.
   - The note still mentions an unmounted `write_domain_core.rs`. That file is
     no longer in the tree (`index_admission/` has 3 entries), so the warning
     is stale.
   - Fix: append a short errata line. Do not delete anything, per the rule.

9. **The `#[cfg(test)]` 200 ms wait applies to every engine lib test that
   renames.**
   - I found no current test that renames while versions on other tables are
     in flight, so this is not red today.
   - It is a latent flake on a slow runner if such a test is added.
   - `shamir-db` and `shamir-server` tests compile the engine without
     `cfg(test)`, so they use 30 s.

10. **Allocation on every commit.**
    - Location: `prelock.rs:794-830`.
    - `touched_table_tokens` allocates and sorts a `Vec` on every commit, even
      when all four sidecar maps are empty. This is negligible next to the WAL
      encode.
    - An optional fast path when all four sidecar sets are empty would skip the
      `Vec`, the sort and the dedup. Not required.

## Check 1: compile, visibility, types, borrows, Send

I compiled all of these by reading and found no error:

- **Imports and visibility in `rename_freeze.rs`:**
  - `crate::repo::{RepoInstance, RepoWritePermit}` (`repo/mod.rs:24-34`).
  - `crate::table::{MigrationFreezeGuard, TableManager}` (`table/mod.rs:45,51`).
  - `shamir_tx::repo_tx_gate::VisibleOutcome` (`shamir-tx/src/lib.rs:48`). It
    has exactly two variants and is not `non_exhaustive`, so the match is
    exhaustive with no `_` arm.
  - `DbError::{RepoRequiresReopen, Conflict, MigrationFrozen}` exist.
  - `.map_err(DbError::from)` uses `From<TxError> for DbError`, and
    `CommitError` is an alias of `TxError` (`tx/mod.rs:34`).
- **Gate API:**
  - `RepoInstance::tx_gate` is `pub async`.
  - `version_allocation_high_water_mark(&self) -> u64` is `pub`.
  - `await_visible(&self, u64) -> VisibleOutcome` is `pub async`.
- **`RenameFreeze` and its signatures:**
  - It is `pub(crate)` inside a private module, and
    `prepare_table_rename_admitted` (`pub(crate)`) takes `&RenameFreeze`.
    `private_interfaces` uses the type's nominal visibility, which is
    `pub(crate)`, so it does not warn.
  - `prepare_table_rename_admitted` and `commit_table_rename_admitted` are only
    called from `catalog.rs`.
- **`TableManager` and the guard:**
  - `detached: pub(super)` is reachable from every writer, because
    `barriers/` and `admission/...` are descendants of `crate::table`.
  - All three struct literals carry the field: `table_manager.rs:533`
    (Clone), `table_manager.rs:707` (test constructor) and
    `write_domain_barrier.rs:231`. No other literal exists
    (`rg "in_flight_creates:"`). `MigrationFreezeGuard` has a single literal.
- **The 24 explicit checks:** every enclosing function returns `DbResult` of
  the right type, so `?` type-checks in each.
- **Accessors:**
  - `counter()` (`table_accessors.rs:226`) and `index_manager_ref()` are
    `pub(crate)`. `RecordCounter::persist_admitted` is `pub`.
- **Moves and drop order:**
  - `RenameFreeze::retire` takes the guard out, so the outer `Drop` becomes a
    no-op.
  - The guard's `retire(mut self)` disarms it, and its `Drop` respects
    `armed`.
  - In every function the permit is declared before the freeze or borrowed
    from the caller, so the freeze is always dropped first.
- **Send:** the rename futures hold `&RenameFreeze`, which holds a
  `TableManager` (already `Sync`; it crosses awaits in many places) and a
  guard (`Arc<AtomicU8>` + `bool`). The spawned writer in test 6 captures only
  a `TableManager`, oneshot senders and a `Vec<RecordId>`.
- **Test API:**
  - These all exist with the signatures the tests use: `RepoInstance::new`,
    `TableConfig::new`, `has_table`, `write_domain`,
    `set_buffer_config_admitted` (`pub(crate)`), and `MemBufferConfig` (5
    fields, not `non_exhaustive`).
  - Also: `index_exists -> bool`, `count -> DbResult<usize>`,
    `get -> DbResult<InnerValue>`, `insert_with_receipt -> WriteOutcome`
    (with `value` and `visibility`), `VisibilityState::Pending{..}`,
    `VisibilityWait::Bounded{timeout_ms}`, `assign_next_version_guarded`,
    `TxContext::bump_counter`, and `Provenance: Default`.
- **Closure inference:** `|_| async { Ok(()) }` against
  `Fut: Future<Output = DbResult<()>>` infers the same way as the existing
  `register_fn(.., |_, _, _| async { Ok(QueryValue::Null) })`.

## Check 2: clippy, rust-guard and layout

- **Clippy:**
  - `absolute_paths`: every new inline path is either `std`/`tokio` (allowed)
    or has 5 or fewer segments.
  - None of the named lint classes applies: no `manual_async_fn`, no guard
    held across `.await`, no `needless_bool`, no `type_complexity`, no
    `uninlined_format_args` (only `written.len()` is a method call), and no
    dead code.
  - Both `#[cfg]` constants are used, so neither configuration has dead code.
- **rust-guard:**
  - `repo/tests/rename/mod.rs` contains only module declarations. There is no
    inline `#[cfg(test)]` module, no local `use` and no `#[allow]`.
  - The new files are mounted: `catalog.rs:14` mounts `rename_freeze`, and
    `repo/tests/mod.rs:5` mounts `rename`.
  - `owned_impl.toml` governs only `RepoInstance`. No existing item is made
    more visible.
- **Layout:** I counted with `ls` and `wc -l`.
  - Files: the largest touched file is `write_domain_rename_admitted.rs` at
    834 lines. All others are at most 830 (`prelock.rs`) and 801
    (`doctor.rs`).
  - Folders: the largest touched folders have 7 entries
    (`repo_instance`, `table`, `accessors`, `pre_commit`). `catalog/` has 3,
    `repo/tests` 6 and `rename/` 4.
  - `admin_table_index.rs` (967 lines) was not touched.
- **fmt:** I did not run it. The note says the parent ran it and it passed.
  The two-line `.store(..)` chains in `ddl_barriers.rs` match rustfmt's
  default 60-character `chain_width`.

## Check 3: behaviour against the brief

- **S1** (`prelock.rs:794-830`):
  - The token set is the union of `write_set`, `index_write_set` (with the
    "same as the previous op" skip and the "already in `write_set`" skip),
    `counter_deltas`, `staged_vectors` and `staged_vector_deletes`, then
    sorted and deduplicated.
  - The loop body is unchanged, and `pre_commit_prelock` is the only commit
    entry point (`commit/entry.rs:672`).
  - Behaviour for commits on unfrozen tables is unchanged, apart from the
    order in which drain guards are taken (now sorted). Production staging
    keeps sidecar tokens a subset of `write_set`.
- **S2:** `admit` checks attachment after obtaining the permit, and the 24
  methods have the check. I re-ran `fn \w+_admitted` over `table/`: there are
  35 hits (34 methods plus `identity_migration.rs:144`), all classified. The
  remaining permit-taking non-`_admitted` methods are:
  - `phase_b_a_backfill` and `phase_c_d_catchup_and_publish`, reached only
    from the checked `create_index_admitted`;
  - `begin_migration_freeze` and `repair`, which go through `admit`;
  - internal bodies.
  The exemption reasons hold.
- **S3:** matches the brief.
- **S4:**
  - `begin` freezes, then detaches; nothing is detached if the freeze fails or
    is cancelled.
  - The high-water mark is sampled after the freeze, inside the timeout, with
    gate init inside too.
  - `Drop` only touches atomics.
- **S5:**
  - All four transitions check `contains_key(from)` after admission and
    freeze before any drain.
  - The counter flush runs between `drain_to_history` and the copy.
  - `retire` runs synchronously right after the commit or after
    `remove_table`, so no cancellation point exists between them.
  - The recovery branches freeze only when `source_matches`, and skip the
    wait and the flush.
  - Public signatures are unchanged.
- **Lock order:** the rename takes the permit, then `ddl_admission(source)`
  (released), then the freeze bit, the writer drain and the uwl fence
  (released). It holds only the permit during the wait, the drain to history,
  the counter `persist_lock`, the copy and `persist`.
  - `ddl_admission` is never held while waiting for the permit:
    `begin_write_barrier`, `begin_migration_freeze` and orphan replay all
    admit first.
  - Writers in the drain set never admit: `RecordCounter::increment` only
    updates its cache, and a dormant-table `get_table` from inside
    materialize is the same pre-existing exposure that every
    permit-plus-drain DDL already has.
  - So the change adds no new cycle.
- **Error codes:**
  - Stale-handle DDL and CRUD return `MigrationFrozen` (`table_migrating`).
  - The timeout returns `Conflict` (`tx_conflict`).
  - `ReopenRequired` returns `RepoRequiresReopen`.
  - On the wire, rename errors still carry `code: None`
    (`admin_table_index.rs:255-259,294`, M6).

## Q11: does any committer wait on the permit while holding an allocated version?

**Yes, in three conditional cases. None is on the ordinary path, so H does not
fire.**

1. **Follower receipt worker.** When a pipeline fails, its fresh
   `VersionGuard`s are parked in the lease
   (`receipt/resume/worker.rs:24-33,81`). They are released only by a later
   successful publication, and that publication must admit first
   (`run_receipt_pipeline`, `receipt/apply.rs:95-147`). The executor missed
   this case.
2. **Drainer orphan replay.** It admits while the orphan's version is still
   unsettled, and the settled floor waits for orphans:
   - `drain_step.rs:365` calls `replay_v2_op`, which admits
     (`recovery/v2.rs:56-67`);
   - `drain_step.rs:615-620` calls `begin_write_barrier`, which goes through
     `admit` (corrected in the note's errata).
3. **Commit materialize resolving a dormant table.** `table_by_token`
   (`materialize.rs:289,620,681`, `commit_phases.rs:771`) calls `get_table`,
   which admits on a cold init (`repo_membership_impl.rs:50-66`). This
   happens after version allocation, and only if the token's table cell is
   dormant. That needs a drop and re-add between staging and commit, so it is
   rare.

Not affected:

- non-tx CRUD (`table_manager_crud.rs:206-258`: the version comes from
  `set_versioned_no_wait`, with no admit before settlement);
- the leader commit (no admit between allocation at `locked.rs:150-170` and
  settlement);
- `tx_gate` init and `gate_provenance` (they only read).

Consequence: a bounded, retryable timeout, as the brief intended. See D1 for
the cost.

## Q12: tests that write through a pre-rename handle after a successful rename

**None. No test needs to change.** I checked the full `rg -l` inventory,
including the four files the note missed (MINOR 6). The contract tests in
acceptance item 10 stay green by reading:

- `durability_tail_failure_retry...`: after the failed attempt it only reads
  through the old handle. The retry takes the recovery branch, freezes and
  then retires. The second retry finds `b` registered and no source, so it
  takes no freeze.
- `precommit_persist_failure...`: reads only.
- `rename_membuffer_wrapped...`: uses a fresh destination handle.
- `ddl_interlock` at line 128: the receipt publishes before it releases the
  permit, so the wait sees the version visible.
- `rv_db_72e8717c`: no extra catalogue reads.

## Q13: the migration twins

Confirmed. `migration_coordinator_admitted` and
`migration_coordinator_recovered_admitted` (`admitted_ops.rs:249,291`) capture
both `self`'s stores and `dst`'s stores. The checks are on both
(`:259,261` and `:299,301`).

## Check 4: test oracles and hangs

- **Item 2:**
  - On master the counter delta and the posting would be applied, so the
    oracles are red there.
  - With S1, the frozen table takes the uwl slow path, which is free, then
    the recheck rejects the commit. No version is allocated.
  - The control test passes because base-index re-derivation skips tokens
    that have no `write_set` entry (`base_index.rs:66-77`).
- **Items 4–6 and 9:** the oracles are exact and satisfiable.
  - In item 6, the persist closure waits for a oneshot that the writer fires
    on its first rejection. That rejection needs only the uwl, which is free.
  - Every await in the multi-thread test is bounded.
- **Item 7:** the internal 200 ms timeout bounds it. A strict insert would
  hang behind the held guard, so the bounded receipt is a justified
  deviation. `create_index` does not wait on visibility
  (`write_domain_online_index.rs:260-334`). Caveat: MINOR 3.
- **Item 8:** dropping a guard that was never committed marks the version
  `Aborted` (`version_guard.rs:232-245`), so the strict insert in
  `assert_source_recovered` cannot hang. Caveat: MINOR 2.
- **Fault-injection seam:** none is needed beyond `persist` closures and held
  version guards.

## Could not verify

- Real compilation, clippy `-D warnings`, rustdoc and fmt. I did not run any
  of them.
- Whether `get_current_bytes` returns a row whose version is above the floor
  (MINOR 3).
- Whether `InMemoryStore::get` returns `Err(NotFound)` for a missing key. The
  test `sidecar_index_only_commit_rejected_while_frozen` asserts that exact
  variant; an existing test only checks `.is_err()`.
- How many polls `drain_to_history` and the store copy need on an in-memory
  current-thread runtime. I assumed fewer than 100.
- CI timing of the 200 ms wait.
- Whether shamir-server or WASM-host code keeps `TableManager` clones across
  a rename (`rg` found no `TableManager` cache in `shamir-db`, `shamir-server`
  or `shamir-engine`).
- Whether a cross-repository ABBA exists between the user repository permit
  (held across `persist`) and the system repository. It exists on master too
  and is unchanged here.
