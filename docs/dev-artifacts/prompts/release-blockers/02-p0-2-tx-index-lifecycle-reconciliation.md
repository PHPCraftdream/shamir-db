# Brief — P0-2: tx plan goes stale relative to CREATE INDEX (regular/unique never rederive; index2/sorted have a TOCTOU + don't clean up retired ops)

Task: #958 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P0-2, verified against the actual source before filing this task. Tasks #960 (P0-4, corrupt unique posting) and #957 (P0-1, DDL admission mutex) are already fixed and committed separately — this task builds on top of #957's `ddl_admission`/`begin_write_barrier` but does not need to modify it further.

**This is the largest and highest-risk task in the release-blocker chain. Read the whole brief before writing any code. If the full scope (sub-bugs 2a + 2b + 2c below) does not fit in your session, prioritize 2a — it is the highest-severity bug (a committed row can be missing its unique-constraint enforcement entirely, or a regular index can permanently miss a posting) — and clearly report what you did NOT get to, rather than claiming full completion on a partial fix.**

## Background: how commit-time re-derivation works TODAY (index2 + sorted only)

A transaction stages index-write plans (`tx.index_write_set`, a `Vec<(table_token, IndexWriteOp)>`) at STAGE time (`insert_tx`/`update_tx`/`delete_tx` in `crates/shamir-engine/src/table/table_manager_tx_ops.rs`), based on a snapshot of the table's indexes taken at that moment. If a DDL registers a NEW index between stage and commit, the staged plan is stale — it doesn't include ops for the new index.

For **index2** (fts/functional/vector) and **sorted** indexes, this is partially handled:

- At stage time, `insert_tx` (and its update/delete counterparts) calls `tx.note_index2_stage_gen(table_token, self.index2_registry().generation())` and `tx.note_sorted_stage_gen(table_token, self.sorted_indexes.generation())` — capturing each registry's generation counter AFTER the stage-time `all_backends()`/snapshot call (see `table_manager_tx_ops.rs` lines ~405-413).
- At commit time, `crates/shamir-engine/src/tx/pre_commit.rs`'s `rederive_index2_ops_post_stage` (lines ~764-1080) and an analogous sorted-index rederive function compare the CURRENT registry generation to the staged one. If unchanged, it's a zero-cost skip. If advanced, it calls `registry.backends_newer_than(stage_gen)` (`crates/shamir-index/src/registry.rs` lines ~132-151) to get only the NEW backends, then re-plans ops for exactly those backends against every staged record (reading the pre-tx value from the data store to distinguish insert-vs-update, per `read_pre_tx_bytes`).

**Regular and legacy unique indexes have NONE of this.** `plan_legacy_insert_ops`/`plan_legacy_update_ops`/`plan_legacy_delete_ops` (`table_manager_tx_ops.rs` lines ~228-309) call `self.index_manager.plan_record_created(...)` etc. ONCE at stage time, gated by `has_any_index()` (an O(1) check). There is no generation captured, no commit-time rederive, and no re-validation of unique guards against indexes created after stage. Confirmed by reading the file: no `note_legacy_stage_gen`-equivalent exists anywhere in this crate.

## Sub-bug 2a (HIGHEST PRIORITY): regular/unique lifecycle has no rederive at all

Concrete failure sequence (unique index):

1. T1 begins a tx, stages an `INSERT` via `insert_tx`. At this moment, `has_any_index()` is `false` (no unique index exists yet on this table) — `plan_legacy_insert_ops` is never called, no `UniqueGuard` is recorded via `tx.record_unique_guard(...)`.
2. A concurrent DDL creates the FIRST unique index on this table, backfilling from the current committed snapshot (T1's staged insert is invisible to it — T1 hasn't committed).
3. T1 commits. `commit_tx`'s existing Phase 2.6 (see `pre_commit.rs`, search for `UniqueGuard` re-validation) only re-validates guards T1 actually recorded — and T1 recorded none, because at stage time there was no unique index to validate against or record a guard for.
4. The row commits with NO unique-index posting at all (a permanently missing posting for regular; for unique, additionally: the row is not constraint-checked and can duplicate an existing value).

Same missing-posting failure mode applies to **regular** (non-unique) indexes with no unique-guard angle (just a silently missing posting, forever, until the row is manually re-touched).

### Required fix for 2a

Mirror the index2/sorted pattern for the legacy `IndexManager` (regular + unique):

1. Find (or add) a generation counter on `IndexManager` analogous to `index2_registry`'s / `sorted_indexes`'s (`crates/shamir-index/src/registry.rs`'s `generation()`/`backends_newer_than()` pattern, or the sorted-index manager's equivalent — check `crates/shamir-index/src/legacy/sorted_index_manager.rs` for whatever generation mechanism it already has, since sorted already participates in rederive). Legacy `IndexManager` (`crates/shamir-index/src/legacy/index_manager.rs`) tracks regular AND unique indexes — you likely need ONE generation counter covering index creation/registration for both regular and unique index definitions, bumped whenever `create_index`/`create_unique_index_from_records` registers a new definition.
2. At stage time (`table_manager_tx_ops.rs`'s `insert_tx`/`update_tx`/`delete_tx` and their `_many` batch variants), capture this new generation the same way `note_index2_stage_gen`/`note_sorted_stage_gen` do — add a `tx.note_legacy_stage_gen(table_token, generation)` (or fold it into an existing `TxContext` method if that's a cleaner shape; check `shamir-tx`'s `TxContext` for where `index2_stage_gens`/`sorted_stage_gens` are defined and add a parallel field).
3. At commit time (`pre_commit.rs`), add a `rederive_legacy_ops_post_stage` function mirroring `rederive_index2_ops_post_stage`'s shape: gate on the generation comparison, and for tables where it advanced, re-plan ops for the NEW index definitions only, against every staged record (same `read_pre_tx_bytes`-based insert-vs-update distinction). For UNIQUE index definitions specifically that appeared after stage: this rederive must ALSO validate the new unique constraint against the staged value (equivalent to `validate_unique_for_create`, but for a definition that didn't exist at stage time) and record a fresh `UniqueGuard` so the existing Phase 2.6 re-validation-under-commit-lock covers it too — a rederive that only adds the posting op WITHOUT the guard/validation reopens the "duplicate slips through" hole for the specific case of a unique index created mid-flight.
4. Ensure `plan_legacy_insert_ops`/`plan_legacy_update_ops`/`plan_legacy_delete_ops`'s existing behavior (called when `has_any_index()` is already `true` at stage time) is UNCHANGED — this fix adds a commit-time top-up for indexes that didn't exist at stage time, it does not replace the existing stage-time path.

## Sub-bug 2b: index2/sorted generation is read AFTER the stage-time snapshot, creating a TOCTOU window

Confirmed by reading `crates/shamir-index/src/registry.rs`'s `insert` (lines ~71-103): `inserted_gen = self.generation.fetch_add(1, AcqRel) + 1` happens, THEN `self.by_id.insert_async(id, (backend, inserted_gen, state))` publishes the backend. Between the `fetch_add` and the `insert_async` completing, `generation()` already reflects the bump but `all_backends()` (which iterates `by_id`) does NOT yet see the new backend.

Sequence: tx stage calls `all_backends()` (misses the new backend, not yet published) — THEN, in the window before `note_index2_stage_gen` runs, the DDL's `insert_async` completes AND the generation was already bumped — tx captures `stage_gen = registry.generation()` which is ALREADY the post-insert value. At commit, `current_gen == staged_gen` → rederive is skipped entirely, even though the tx's stage-time snapshot never saw the new backend.

### Required fix for 2b

The generation capture and the backend snapshot must be a SINGLE atomic operation, not two separate reads. Options (pick whichever fits the existing `scc`-based registry structure with least churn — this codebase's concurrency rules in CLAUDE.md prefer `scc::HashMap`/lock-free constructs, avoid introducing a `std::sync::Mutex`/`RwLock` around the whole registry as a shortcut):

- Add a `registry.snapshot_with_generation()` method that returns `(Vec<Arc<dyn IndexBackend>>, u64)` computed from ONE consistent read — e.g. read `generation()` FIRST, then iterate `by_id` (reversing the current insert()'s ordering isn't enough by itself; you need the READ side to establish a happens-before: reading `generation()` with `Acquire` before iterating `by_id` only works if `insert()`'s publish to `by_id` happens-before its `generation` bump is OBSERVABLE — check whether swapping `insert()`'s own internal order, i.e. publish to `by_id` FIRST then bump `generation` last with `Release`, combined with the reader doing `Acquire` generation THEN iterating `by_id`, gives you the right total order: any backend whose `insert()` bumped generation to a value ≤ the reader's observed generation is guaranteed already visible in `by_id`). Reason carefully about the ordering here — this is the crux of the fix — and document your chosen invariant in a doc comment the way this codebase's existing F-56/F-69/F-70 fixes do (see `writer_drain_barrier.rs` and `write_barrier_flags.rs` for the documentation style/rigor expected).
- Alternatively (simpler, more conservative, acceptable if the ordering proof above is too subtle to get right confidently): make stage-time snapshot + generation capture happen while holding a brief read-side synchronization point that the registry's `insert()` also synchronizes against, so there's no window at all. Prefer the lock-free ordering fix above if you can prove it correct; fall back to this only if not.
- Apply the SAME fix to whatever generation mechanism the sorted-index manager uses (check `crates/shamir-index/src/legacy/sorted_index_manager.rs` — does it reuse `crates/shamir-index/src/registry.rs`'s `IndexRegistry`, or does it have its own separate generation counter with the same insert-order bug? Verify before assuming.).
- Apply the SAME new-generation-counter pattern from 2a's fix to the legacy `IndexManager` too, so all four families snapshot consistently.

## Sub-bug 2c: rederive only ADDS ops for new backends; it never REMOVES ops/guards for backends retired (DROP/RENAME'd) after stage

`rederive_index2_ops_post_stage` only `extend`s `appended` with newly-derived ops — there is no `retain`/`remove` step for backend/index IDs that were retired (dropped or renamed away) between stage and commit. A tx staged against an index that gets DROP'd mid-flight can still apply its now-stale posting ops at commit, resurrecting an orphan posting for an index that's supposed to be gone. Same issue for the sorted rederive path.

### Required fix for 2c

This is more subtly related to task #959 (P0-3, DROP INDEX safety) which is a LATER task in this chain — that task will introduce a persisted `Dropping` state (tombstone) for retired indexes. For THIS task (#958), the minimum viable fix that doesn't require #959's full state machine:

- When rederiving, ALSO check for staged ops whose target backend/index ID is no longer present in the CURRENT registry (i.e. it was removed since stage) and exclude/retract them from what gets applied at commit. At minimum, `retain` the tx's ALREADY-staged `index_write_set` entries (from the ORIGINAL stage-time plan, not just the newly-derived ones) against the current live backend/index ID set for the affected table, dropping ops that target IDs no longer registered.
- If a full, provably-correct fix here genuinely depends on #959's tombstone state machine landing first (i.e. you cannot safely distinguish "retired and safe to drop the op" from "still transitioning" without it), STOP and clearly document this dependency in your report instead of half-fixing it — do not invent an ad-hoc state machine here that #959 will have to rip out. A reasonable stopping point is: fix the common case (index fully removed from the registry by commit time → its ops are dropped), and leave the "index retired mid-drain" partial-transition case to #959.

## Required tests

Follow this crate's existing `tests/` layout (see `crates/shamir-engine/src/table/tests/mod.rs` for how existing suites like `f70_lock_order_inversion_tests`/`f95_ddl_admission_tests`/`f72_planner_invisibility_tests` are wired in — add new modules the same way).

Minimum required matrix (per the review's own requirement — INSERT/UPDATE/DELETE × {regular, unique} × {stage-before-create, overlap-with-create, commit-after-create}):

- **2a regular**: tx stages INSERT/UPDATE/DELETE before a regular index is created; DDL creates the index; tx commits; assert the posting for the new index IS present and correct after commit (query via the index's own lookup path, not just "commit succeeded").
- **2a unique — the critical one**: tx stages INSERT before a unique index is created; DDL creates the FIRST unique index on the table (backfilling from committed snapshot only); tx commits; assert (a) the unique posting for the tx's row IS present, and (b) a SECOND tx attempting to insert the SAME unique value after the first commits is correctly rejected as a duplicate (proves the guard was retroactively recorded and re-validated, not just that a posting silently exists).
- **2a duplicate-introduced-after-stage**: two concurrent txs, T1 stages a value BEFORE a unique index exists, T2 (or a plain non-tx write, whichever exercises the path more directly) also introduces the same value through a path where the new unique index IS visible, DDL/commit ordering arranged so BOTH could plausibly land — assert only one wins and the other is rejected, not that both silently commit.
- **2b TOCTOU**: a deterministic (paused/synchronized, not sleep-based — mirror the `Notify`/`AtomicBool` conventions in `f70_lock_order_inversion_tests.rs`/`f95_ddl_admission_tests.rs`) test that forces the exact window described above (generation bumped, `by_id` insert not yet visible, tx captures stage_gen) and proves the tx's commit-time rederive STILL fires (i.e. does not skip based on `current_gen == staged_gen` alone when the true backend-set changed).
- **2c retired ops dropped**: tx stages ops against an index; that index is DROP'd before the tx commits; assert the tx's commit does NOT apply the stale posting op for the dropped index (verify by checking the dropped index's storage/namespace has no orphan entry after commit — or, if the index is fully gone, verify no error/panic and no leaked data through whatever introspection is available).

## Scope discipline

- This task's natural blast radius is real (it touches `table_manager_tx_ops.rs`, `pre_commit.rs`, `registry.rs`, `sorted_index_manager.rs`, legacy `index_manager.rs`, and `shamir-tx`'s `TxContext`) — that's expected and matches the review's own assessment that this is the deepest fix in the wave. Do NOT additionally touch: DROP INDEX's persisted state machine (that's #959), RENAME INDEX (#961/#962), the write-barrier admission mutex from #957 (already fixed, do not modify), or anything in `shamir-query-builder`/TS clients.
- Run ONLY the centralized test entry point: `./scripts/test.sh -p shamir-engine` and, if you touch `shamir-index`/`shamir-tx` directly, `./scripts/test.sh -p shamir-index -p shamir-tx` too. Raw `cargo test` is blocked by this repo's perimeter guard.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` (workspace-wide, since this touches multiple crates) must be clean before you declare done. Note PRE-EXISTING unrelated failures instead of fixing them inline.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` either — the orchestrator verifies your diff and the test run, then commits. Only edit files and run read-only/build/test commands. Delete any stray log files you create in the repo root yourself (plain `rm <file>.log` is fine, that's not a git command) rather than leaving them, but mention it in your report if you do leave any.

## What to report back

Given the size of this task, structure your final report clearly by sub-bug (2a/2b/2c): what you fixed, what design decision you made and why (especially for 2b's ordering proof and 2c's scope boundary with #959), which tests you added and what each proves, and the exact `cargo fmt`/`cargo clippy`/`./scripts/test.sh` commands with real pass/fail counts and exit codes. If you stopped short of full scope (very possible given the size — see the priority note at the top), say EXACTLY what remains and why, so the orchestrator can decide whether to re-delegate a follow-up round into the same session before moving to the next task in the chain (#959).
