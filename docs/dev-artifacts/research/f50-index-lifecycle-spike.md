# F-50 Step 1 — online index build lifecycle design spike (#869, P0)

**Status:** spike complete. **Recommendation: adopt the generation-gated
ops-plan re-derivation mechanism** — re-derive index2 posting ops against
the LIVE backend registry in `pre_commit_prelock` Phase 2.7 (a new phase,
AFTER Phase 2.5's barrier locks, BEFORE Phase 4's WAL begin), gated by a
new `IndexRegistry::generation` counter captured at stage time so the
re-derivation is zero-cost on the common path. The prototype proves the
mechanism for ONE backend kind (functional) and ONE mutation kind
(single-row INSERT): the red test fails deterministically on the
pre-fix code and passes after the fix, and the existing
`stage_and_commit_inside_window_still_misses_new_index_part_b_open` test
has been renamed to `..._now_indexes_new_index_part_b_closed` with an
inverted assertion reflecting the closed behavior. A persisted
`Building`/`Ready` lifecycle state was investigated and is **NOT needed**
for this mechanism (rationale in §1.2) — it is deferred to Step 3 (DDL
cancellation / crash-restart continuation), where it becomes load-bearing.

The prototype code described in §2 is committed alongside this memo as a
clearly-scoped spike artifact. Step 2 (§5) extends it to the remaining
backend kinds (fts/vector), the remaining mutation kinds
(update/delete/set), the sorted-index residual, and the planner
Ready-gating. Step 3 (§5.3) owns crash/restart continuation and DDL
cancellation — genuine design complexity that deserves its own focused
pass, per §1.4's scope decision.

---

## 1. What this spike settled

### 1.1 Open question 1 — ops-plan re-derivation timing/mechanism (DECIDED)

**Decision: a new Phase 2.7 (`redrive_index2_ops_post_stage`) inside
`pre_commit_prelock`, re-deriving posting ops for backends registered
AFTER the tx's stage-time snapshot, gated by `IndexRegistry`'s new
`generation` counter.**

**Investigation and reasoning (alternatives rejected):**

The brief's confirmed constraint (from prior session reading, restated in
the brief's "Critical confirmed finding") is: `wal_ops_from_tx`
(`commit.rs:282-330`) serializes `tx.index_write_set` DIRECTLY into
`WalOpV2::IndexPut`/`IndexDel` entries — the WAL entry's durable content
IS the stage-time ops-plan. Since Phase 4 (WAL `begin_grouped`) is the
commit point and recovery replays the WAL entry's own logged ops, any fix
that re-derives ops AFTER Phase 4 creates a crash-safety hole: a crash
after WAL-write but before Phase 5c apply would recover using the STALE
(WAL-logged) ops, silently reintroducing the exact miss the fix closes.
**A correct fix must re-derive BEFORE `wal_ops_from_tx` runs.**

Reading `pre_commit.rs`'s phase structure settled the insertion point:
the pre-Phase-4 pipeline is `pre_commit_prelock` (Phase 1 interner merge,
Phase 2.5 barrier-lock acquisition, Phase 2.6 unique re-validation) →
`pre_commit_locked_validate` (Phase 2 SSI, Phase 2-bis phantom, C6
empty-tx check, `claim_write_set`, Phase 3 version-assign, **`wal_ops_from_tx`
call** at `:695`). So the latest feasible point that is still pre-WAL is
inside `pre_commit_prelock` — specifically AFTER Phase 2.5 (which
acquires the per-table `unique_write_lock` that a concurrent
`create_index_v2` holds across its full backfill→register sequence), so
the live registry snapshot includes any in-flight create that has now
finished registering.

**Why OUTSIDE `commit_lock` and BEFORE Phase 2.6** (not inside
`pre_commit_locked_validate`): the re-derivation is per-table, async,
and has no commit-window dependency (no `record_commit_writes` /
`predicate_conflicts_batch` involvement), so holding the global
`commit_lock` for it would be needless contention. Phase 2.6's unique
re-validation is unaffected (re-derived index2 ops are non-unique
postings). Placing it right before the F-48b test seam at the end of
`pre_commit_prelock` keeps it pre-lock and pre-WAL with a clean
happens-before chain.

**Why a generation counter (not an unconditional re-derivation, and not
a full-snapshot diff):**

`IndexRegistry` had no version/epoch counter before this spike — only
`next_id` (an `AtomicU32` for id allocation, unrelated to membership
changes). An unconditional re-derivation for EVERY index2-bearing-table
tx would call `backends_newer_than` + `data_store.get` per staged record
on every commit, paying the O(rows×new_backends) cost even when no DDL is
concurrent. The generation counter makes it zero-cost on the common path:
capture `generation()` once at stage time (`note_index2_stage_gen`),
compare once at commit — a single `AtomicU64` Acquire load short-circuits
to `continue` when unchanged.

A full snapshot diff (capture the `all_backends()` `Vec` at stage, diff at
commit) was rejected: it stores a `Vec<Arc<dyn IndexBackend>>` per table
per tx (memory cost on every index2-bearing tx, even ones that never see
a concurrent create), and the diff is O(old∩new) to avoid double-planning.
The per-backend insertion-generation tag stored alongside the `Arc` in
`by_id` (`scc::HashMap<u32, (Arc<dyn IndexBackend>, u64)>`) gives the same
filter with one counter per tx and no snapshot retention — `backends_newer_than(gen)`
returns exactly the backends with `insertion_gen > stage_gen`, never
re-planning (and thus never double-counting `BumpFtsStats` for) backends
the tx already planned against.

**Old-value resolution (insert vs. update vs. delete):**
`tx.write_set` carries only the NET staged op (`KvOp::Set`/`KvOp::Remove`),
not whether a `Set` is a fresh insert or an overwrite, nor the pre-tx
value needed to plan the REMOVE half of an update/delete posting diff.
Phase 5a materialize has NOT run yet at Phase 2.7 (we are still in
prelock), so the data store still holds the PRE-tx committed value. A
single `data_store.get(key)` per staged record settles it: `NotFound` ⇒
`plan_insert_tx`, `Some(old)` ⇒ `plan_update_tx` (Set) or
`plan_delete_tx` (Remove). The cost is bounded by the number of staged
records AND gated behind the generation check — only paid in the rare
DDL-concurrent case.

### 1.2 Open question 2 — lifecycle state: Building/Ready NOT needed for the re-derivation mechanism (DECIDED)

**Decision: NO persisted `Building`/`Ready` lifecycle state is added in
this spike. The generation-gate re-derivation mechanism does not require
it. A `Building`/`Ready` (or richer) state is deferred to Step 3 (crash/
restart continuation + DDL cancellation), where it becomes load-bearing.**

**Investigation and reasoning:**

1. The brief asked for a persisted lifecycle state so "the planner only
   uses `Ready` backends; a `Building` backend is invisible to the planner
   even though it exists in the registry." That requirement is real for a
   register-FIRST-then-backfill design (where a not-yet-backfilled backend
   would be visible to reads). But the current `create_index_v2`
   (`table_manager_index_mgmt.rs:311-325`) does the OPPOSITE: it
   **backfills FIRST, then registers** — the backend is invisible to the
   planner (not in `by_id`) until AFTER its backfill is complete. So a
   newly-created index2 backend is effectively "Ready" the moment it
   appears in the registry; there is no `Building` window to hide from
   reads.

2. The generation-gate re-derivation (§1.1) is a per-COMMIT mechanism,
   not a per-backend-lifecycle one. It does not consult any backend state
   field — it only checks `insertion_gen > stage_gen`. A `Building` flag
   would add a field the re-derivation never reads.

3. `IndexDescriptor` (the persisted shape, `persistence.rs`'s
   `PersistedIndexes { next_id, Vec<IndexDescriptor> }`) would need a new
   `state: IndexState` field with `#[serde(default)]` (for
   forward-compat with existing on-disk metadata) plus a `match` in the
   planner's `try_plan_index2` (`read_planner.rs:32-104`) to skip
   non-`Ready` backends. That is real persistence + query-planning work
   that is NOT needed to close Part B's guaranteed miss. Forcing it into
   this spike would expand the blast radius without closing any additional
   bug.

4. **Where it DOES become load-bearing (Step 3):** crash/restart
   continuation (a build interrupted by a crash — does it resume, restart
   from scratch, or leave a permanently-`Building` orphan?) and DDL
   cancellation (a `DROP` or explicit cancel mid-build not leaving a
   partially-queryable index). Both REQUIRE a persisted state to
   distinguish "this index was fully built" from "this index was
   interrupted" across a restart. That design work is scoped to Step 3
   (§5.3) with the reasoning in §1.4.

### 1.3 Open question 3 — sorted-index residual: SEPARATE fix, shares the root cause (DECIDED)

**Decision: the sorted-index residual (`table_manager_sorted_index.rs`'s
register-before-backfill) is a DIFFERENT mechanism from Part B and does
NOT share the generation-gate re-derivation fix, but shares the same
root-cause class (register-before-ready). It needs its own, simpler
fix — scoped to Step 2.**

**Investigation and reasoning:**

1. `table_manager_sorted_index.rs:7-108` (`create_sorted_index` /
   `create_sorted_index_with_include`) registers the `SortedIndexDefinition`
   via `self.sorted_indexes.register(def)` at `:90` BEFORE the backfill
   stream (`:97-106`) completes. A row written by a concurrent tx during
   the backfill window can be missed by the backfill cursor and by the
   live `on_record_created` hook (the def is registered, but... actually
   the hook SHOULD fire — investigate in Step 2 whether the live hook
   catches it). The doc comment at `:7-16` marks this `cancel-safe: NO`.

2. **Crucially, sorted indexes do NOT have the `tx.index_write_set`
   staleness problem.** Confirmed by reading
   `table_manager_tx_ops.rs:228-309` (`plan_legacy_insert_ops` /
   `plan_legacy_update_ops` / `plan_legacy_delete_ops`): these call
   `self.sorted_indexes.plan_record_created(&rid, rec, 0)` etc. at STAGE
   time, producing `IndexWriteOp`s that go into `tx.index_write_set` —
   the SAME mechanism as index2. So a tx that stages before a new sorted
   index is registered WOULD have the same stale-plan miss. BUT the
   `SortedIndexManager` has NO generation counter and NO registry
   abstraction equivalent to `IndexRegistry` — the generation-gate
   re-derivation mechanism cannot be applied directly without first
   adding an equivalent generation/registry abstraction to
   `sorted_index_manager`. That is a non-trivial refactor of the legacy
   index path, scoped to Step 2.

3. The sorted-index residual is LOWER priority than Part B because: (a)
   sorted indexes are the legacy path (the `index2` pipeline is the
   forward direction); (b) the register-before-backfill hazard is the
   SAME class already narrowed by `create_index`'s
   `collect_all_current_records`-under-lock pattern (not fully closed,
   but the window is the backfill duration, narrower than Part B's
   stage-to-commit window which spans the ENTIRE tx lifetime). Step 2's
   fix should evaluate whether a generation counter on
   `SortedIndexManager` (mirroring `IndexRegistry`) or a simpler
   register-AFTER-backfill reordering closes it.

### 1.4 Open question 4 — Step 2 vs. Step 3 scope boundary (DECIDED)

**Decision: Step 2 owns the remaining backend kinds + mutation kinds +
sorted-index + planner Ready-gating. Step 3 owns crash/restart
continuation + DDL cancellation + the persisted `Building`/`Ready`
lifecycle state that those require.** Mirrors the F-48 → F-48b precedent:
F-48 handled what was mechanically reachable in one delegation, F-48b
handled a deeper, separately-scoped follow-up.

**Reasoning:**

1. **Step 2 is mechanical-extension work.** The prototype's pattern
   (`note_index2_stage_gen` capture at stage → `redrive_index2_ops_post_stage`
   at commit) generalizes to the remaining backend kinds and mutation
   kinds by mirroring the capture call site and the plan_*_tx dispatch.
   The vector backend needs one extra step (re-derive `tx.staged_vectors`
   for the new HNSW backend, since `plan_insert_tx` is a no-op for
   vectors — §5.1). The sorted-index needs a generation/registry
   abstraction added first (§5.2). The planner Ready-gating needs the
   `IndexState` field + the `try_plan_index2` match (§5.1) — but WITHOUT
   the persistence/restore work that crash-restart requires.

2. **Step 3 is genuine design work, not mechanics.** "A build interrupted
   by a crash — does it resume, restart from scratch, or leave a
   permanently-`Building` orphan?" has no obviously-correct answer and
   interacts with: the `allocate_id` reserve-persist (already landed,
   `:100-119`), the `save_index2_metadata` final-persist (`:327-328`),
   recovery replay, and the doctor's `repair()` path. DDL cancellation
   ("a DROP or explicit cancel mid-build not leaving a
   partially-queryable index") needs a clean interrupt of the backfill
   stream + orphan-posting cleanup. Both need the persisted
   `Building`/`Ready` state that §1.2 deferred. Forcing them into Step 2
   would either rush the design or leave it half-done — the F-48/F-48b
   precedent shows a clean split avoids both.

---

## 2. What was prototyped

### 2.1 Prototype code (committed alongside this memo)

Files changed:

- **`crates/shamir-index/src/registry.rs`** — new `generation: AtomicU64`
  field on `IndexRegistry`, bumped (`fetch_add(AcqRel)`) on every
  successful `insert` / `remove_by_id`. The `by_id` map's value type
  widened from `Arc<dyn IndexBackend>` to `(Arc<dyn IndexBackend>, u64)`
  so each entry carries the generation at which IT was inserted. Three
  new accessors: `generation()` (the gate value, Acquire load),
  `backends_newer_than(threshold_gen)` (filtered snapshot of backends
  with `insertion_gen > threshold`). All existing accessors
  (`get_by_id`, `all_backends`, `all_descriptors`, `find_by_field_and_kind`,
  `remove_by_id`) updated to destructure the tuple.

- **`crates/shamir-tx/src/tx_context.rs`** — new field
  `index2_stage_gens: TFxMap<u64, u64>` (table_token → generation at stage
  time), initialized empty in `TxContext::new`. One new method
  `note_index2_stage_gen(&mut self, table_token, gen)` (capture;
  `or_insert` makes re-capture in the same tx a no-op — the EARLIEST
  generation is the most stale, so it is the one the commit gate must
  compare against). Plain (non-`Mutex`) map: every capture/read site
  holds `&mut TxContext` (staging methods + commit pipeline's
  `pre_commit_prelock`), so no interior mutability is needed (unlike
  `ri_barrier_tokens`, which is recorded through a shared `&TxContext`).

- **`crates/shamir-engine/src/table/table_manager_tx_ops.rs`** — the
  `note_index2_stage_gen` capture call wired into the THREE INSERT
  staging paths (`insert_tx` after `stage_mutation`;
  `insert_tx_many` + `insert_tx_many_bytes` after the index_ops extend).
  Each captures `self.index2_registry().generation()` AFTER its stage-time
  `all_backends()` snapshot (so every backend in that snapshot has
  `insertion_gen ≤ stage_gen`). UPDATE/DELETE/SET paths deliberately NOT
  wired (§5.1 — Step 2).

- **`crates/shamir-engine/src/tx/pre_commit.rs`** — new
  `redrive_index2_ops_post_stage(tx, repo)` async fn, called from
  `pre_commit_prelock` AFTER Phase 2.5's barrier-lock loop and BEFORE
  the F-48b test seam (so it runs pre-lock, pre-WAL). The fn: (1) skips
  if `index2_stage_gens` is empty (zero-overhead gate); (2) for each
  touched table, reads the current generation and `continue`s if
  unchanged; (3) `backends_newer_than(stage_gen)` to get the diff; (4)
  `snapshot_ops()` the staged `KvOp`s for that table; (5) per staged
  record, `data_store.get(key)` to resolve insert-vs-update-vs-delete;
  (6) dispatch `plan_insert_tx` / `plan_update_tx` / `plan_delete_tx`
  for each new backend; (7) append to `tx.index_write_set`.

- **`crates/shamir-engine/src/table/tests/f50_index_lifecycle_spike_tests.rs`**
  — new file, 2 tests (the red→green proof + the quiescent control).

- **`crates/shamir-engine/src/table/tests/index2_create_barrier_tests.rs`**
  — `stage_and_commit_inside_window_still_misses_new_index_part_b_open`
  renamed to `..._now_indexes_new_index_part_b_closed`, doc rewritten,
  assertion INVERTED (was `!owners.contains` documenting the miss; now
  `owners.contains` asserting the fix). Section header comment updated.

- **`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`** —
  `backfill_index2_backend`'s doc comment §Part B rewritten from "still
  OPEN, not attempted here" to "CLOSED for the functional/INSERT case by
  F-50 (#869 spike); full closure (vector/fts/update/delete/sorted-index)
  is Step 2." Net-effect paragraph updated to reference the renamed test.

### 2.2 The test (no pause-seam needed)

The brief noted the interleaving is deterministic by construction — three
sequential steps, no injection needed. The prototype's test
(`tx_staging_before_index_register_commits_with_posting_after_fix`) does
exactly that:

1. STAGE a tx's insert via `execute_insert_tx` BEFORE the functional
   index exists (so the stage-time `all_backends()` snapshot is empty —
   the tx's `index_write_set` permanently carries zero ops for the
   not-yet-created backend; the generation is captured here).
2. COMPLETE `create_index_v2` (backfill + register). The tx's row is
   still only STAGED (not committed), so the backfill stream cannot see
   it — only the pre-existing row gets backfilled. The register bumps
   the generation past the captured value.
3. COMMIT the tx. Pre-fix: Phase 5c has no functional posting to write.
   Post-fix: Phase 2.7 sees the generation advanced, re-derives the
   posting, appends it pre-WAL → Phase 5c writes it.

The quiescent control (`quiescent_tx_unchanged_generation_indexes_via_stage_plan`)
creates the index FIRST, so the generation never changes — it asserts
the row is indexed via the normal stage-time plan and exactly ONE posting
exists (no double-application).

### 2.3 Results (run 2026-07-28)

**Red (fix disabled — `redrive_index2_ops_post_stage` call commented out):**

```
./scripts/test.sh -p shamir-engine -- f50_index_lifecycle_spike
```

```
        PASS [   0.183s] (1/2) shamir-engine table::tests::f50_index_lifecycle_spike_tests::quiescent_tx_unchanged_generation_indexes_via_stage_plan
        FAIL [   0.183s] (2/2) shamir-engine table::tests::f50_index_lifecycle_spike_tests::tx_staging_before_index_register_commits_with_posting_after_fix
    test result: FAILED. 0 passed; 1 failed; 0 ignored; 1727 filtered out
    thread '...' panicked at ...:173:5:
    F-50: the row staged before the index was registered MUST be indexed after commit — a guaranteed miss here means #538 Part B is still open
     Summary [   0.231s] 2 tests run: 1 passed, 1 failed, 1726 skipped
exit=100
```

The deterministic guaranteed-miss is reproduced exactly — Bob's row is
physically present (`tbl.get(bob)` succeeds) but absent from the
functional index (the assertion at `:173` fails).

**Green (fix restored):**

```
./scripts/test.sh -p shamir-engine -- f50_index_lifecycle_spike
```

```
        PASS [   0.178s] (1/2) shamir-engine table::tests::f50_index_lifecycle_spike_tests::quiescent_tx_unchanged_generation_indexes_via_stage_plan
        PASS [   0.195s] (2/2) shamir-engine table::tests::f50_index_lifecycle_spike_tests::tx_staging_before_index_register_commits_with_posting_after_fix
     Summary [   0.232s] 2 tests run: 2 passed, 1726 skipped
exit=0
```

**Existing barrier suite (renamed Part B test now asserts the fix):**

```
./scripts/test.sh -p shamir-engine -- index2_create_barrier
```

```
        PASS [   0.812s] (6/7) shamir-engine table::tests::index2_create_barrier_tests::stage_and_commit_inside_window_now_indexes_new_index_part_b_closed
     Summary [   0.883s] 7 tests run: 7 passed, 1721 skipped
exit=0
```

All 7 tests in `index2_create_barrier_tests.rs` pass — the renamed Part B
test now asserts Carol's row IS indexed (the fix closed what it used to
document as open), and the other 6 (Part A's commit-time serialization,
the backfill regression, the crash-orphan-id tests) are unaffected.

### 2.4 Why the placement is load-bearing for crash safety

The call site comment in `pre_commit.rs` documents this precisely; the
memo restates it as the design's central invariant:

- **AFTER Phase 2.5's barrier locks** (`pre_commit.rs:447-482`): the
  per-table `unique_write_lock` is acquired for every table the tx wrote
  to whose `needs_write_barrier()` is true. A concurrent `create_index_v2`
  holds that SAME lock across its full backfill→register sequence
  (`table_manager_index_mgmt.rs:78`), so by the time Phase 2.7 reads the
  live generation, any in-flight create has finished registering — the
  `backends_newer_than(stage_gen)` snapshot includes it.
- **BEFORE Phase 4's WAL begin** (`pre_commit_locked_validate:695` calls
  `wal_ops_from_tx`): the re-derived ops MUST be in `tx.index_write_set`
  before serialization, or recovery would replay the stale stage-time
  plan. Re-deriving AFTER the WAL write is FORBIDDEN — it would create
  the exact crash-safety hole the brief warned against.
- **OUTSIDE `commit_lock`** (the whole `pre_commit_prelock` fn runs
  pre-lock): the re-derivation is per-table, async, and has no
  commit-window dependency, so holding the global lock for it would be
  needless contention.

`cancel-safe: YES` — the fn appends to `tx.index_write_set` only
(in-memory, tx-scoped, RAII-dropped on abort). The `data_store.get`
reads are read-only. No durable mutation happens here, so cancellation
before Phase 4 is a clean abort (re-derived ops never reached the WAL).

---

## 3. What this spike did NOT do (Step 2 / Step 3's job)

1. **The remaining mutation staging paths** — `update_tx` /
   `update_tx_bytes` / `delete_tx` / `set_tx` in `table_manager_tx_ops.rs`
   do NOT call `note_index2_stage_gen`. A tx touching only those paths is
   not re-derived (the status quo — no regression, since
   `index2_stage_gens` stays empty for them and the gate short-circuits).
   Mirroring the one-line capture onto those paths is mechanical Step 2.

2. **The vector backend's `staged_vectors` re-derivation** —
   `VectorBackend::plan_insert_tx` is a no-op (HNSW embeddings are
   buffered in `tx.staged_vectors` separately, `table_manager_tx_ops.rs:77-89`).
   Phase 2.7 re-derives POSTING ops only; it does not re-derive staged
   vectors for a new vector backend registered mid-tx. Step 2 must add a
   `staged_vectors` re-derivation branch (call each new vector backend's
   `staged_vector(rid, rec)` and `tx.stage_vector(token, rid, vec)`).

3. **FTS `BumpFtsStats` idempotency across re-derivation** — the
   `IndexWriteOp::BumpFtsStats` variant is in-memory only (not a posting;
   `commit.rs:334` skips it in `wal_ops_from_tx`). A re-derived FTS op
   that emits a `BumpFtsStats` for a backend the tx never planned against
   at stage time is correct (the tx genuinely inserted a doc). But a tx
   that DID plan against the FTS backend at stage time is correctly
   EXCLUDED by the generation filter (`insertion_gen > stage_gen`), so no
   double-count. Step 2 should add a test confirming this for the FTS
   case specifically.

4. **The sorted-index residual** — `table_manager_sorted_index.rs`'s
   register-before-backfill (§1.3). Needs its own generation/registry
   abstraction on `SortedIndexManager` (Step 2, §5.2).

5. **Planner `Ready`-gating** — `read_planner.rs::try_plan_index2`
   (`:32-104`) does not yet filter by lifecycle state. It is currently
   safe because backends are only registered post-backfill (§1.2), but
   Step 3's `Building` state requires the `match` gate. Step 2 may add a
   non-persisted in-memory `Ready` flag as a forward-compat scaffold; the
   persisted `IndexState` field is Step 3.

6. **Crash/restart continuation** — a `create_index_v2` interrupted by a
   crash between `allocate_id` and the final `save_index2_metadata`
   leaves a reserved-but-never-registered id (already handled by #534
   finding 2's reserve-persist). A crash AFTER register but before the
   final metadata save leaves a registered backend not persisted to disk
   — on restart it vanishes, and any postings a concurrent tx wrote for
   it (via Phase 2.7 re-derivation) become orphan garbage under the dead
   id. This is Step 3 (§5.3).

7. **DDL cancellation** — a `DROP INDEX` or explicit cancel issued
   mid-build. Step 3 (§5.3).

8. **`KNOWN_LIMITATIONS.md`** — NOT updated (Step 2's job, once the full
   implementation lands).

---

## 4. Decision summary

| Question | Decision | Rationale |
|---|---|---|
| Q1: re-derivation timing/mechanism | New Phase 2.7 `redrive_index2_ops_post_stage` in `pre_commit_prelock`, AFTER Phase 2.5 barrier locks, BEFORE Phase 4 WAL begin; gated by `IndexRegistry::generation` captured at stage time | Must be pre-WAL (recovery replays logged ops) or it reopens the crash-safety hole. Generation counter makes it zero-cost on the common path; per-backend insertion-gen tag gives exact diff without snapshot retention. |
| Q2: persisted lifecycle state | NOT added in this spike. Deferred to Step 3 (crash/restart + DDL cancellation) where it is load-bearing. | Current `create_index_v2` backfills-then-registers, so a backend is effectively "Ready" the moment it appears — no `Building` window to hide from reads. The re-derivation never consults a state field. Forcing it in expands blast radius without closing a bug. |
| Q3: sorted-index residual | SEPARATE fix (Step 2). Shares the register-before-ready root cause, NOT the generation-gate mechanism. | `SortedIndexManager` has no `IndexRegistry`-equivalent abstraction/generation counter. The tx stale-plan miss DOES apply (sorted ops go into `tx.index_write_set`), but applying the fix needs the registry abstraction added first. |
| Q4: Step 2 vs Step 3 | Step 2 = remaining backends + mutation kinds + vector `staged_vectors` + sorted-index + planner gating. Step 3 = crash/restart continuation + DDL cancellation + persisted `Building`/`Ready` state. | Mirrors F-48→F-48b. Step 2 is mechanical-extension; Step 3 is genuine design work (resume-vs-restart-vs-orphan, orphan cleanup) that interacts with recovery replay + doctor repair. |

---

## 5. Implementation plan

### 5.1 Step 2 — extend to all backend kinds + mutation kinds + sorted-index

Each touch point mirrors the prototype's pattern (capture generation at
stage via `note_index2_stage_gen`; Phase 2.7 already handles the
re-derivation generically once the capture is present).

**A. Remaining mutation staging capture sites (4 sites in
`table_manager_tx_ops.rs`):**

1. **`update_tx` (`:749+`)** — add `tx.note_index2_stage_gen(self.table_token(), self.index2_registry().generation())`
   after the `stage_mutation` call, mirroring `insert_tx`.

2. **`update_tx_bytes` / `update_tx_ref`** (the lens-driven update
   variants) — same one-line capture after staging.

3. **`delete_tx`** — same, after the delete staging.

4. **`set_tx`** — same, after the set staging.

   Each capture must run AFTER the stage-time `all_backends()` snapshot
   in the corresponding `plan_*_ops` call, so every backend in that
   snapshot has `insertion_gen ≤ stage_gen`.

**B. Vector backend `staged_vectors` re-derivation (1 site in
`pre_commit.rs::redrive_index2_ops_post_stage`):**

5. After the posting-ops loop, add a branch: for each backend in
   `new_backends` that is a `VectorBackend` (check `descriptor().kind`),
   and for each staged `KvOp::Set` record, call
   `backend.staged_vector(rid, &new_rec)`; if `Some(vec)`, call
   `tx.stage_vector(table_token, rid, vec)`. This mirrors
   `table_manager_tx_ops.rs:77-89`'s `stage_vectors`. Phase 5d's existing
   promote logic then handles it at commit. NOTE: the old-value
   resolution for a vector UPDATE (old carried a vector, new does not →
   `stage_vector_delete`) must also be mirrored — see
   `stage_vector_deletes_on_update` (`:127-145`).

**C. FTS `BumpFtsStats` test (1 new test):**

6. Add a test in `f50_index_lifecycle_spike_tests.rs` (or a Step 2
   permanent file) that stages a tx, creates an FTS index, commits, and
   asserts the doc_count is exactly 1 (not 2) — confirming the generation
   filter prevents double-counting `BumpFtsStats` for a backend the tx
   planned against at stage time. The mechanism is already correct (the
   backend is excluded by `insertion_gen > stage_gen`), but an explicit
   test guards against a future regression in the filter.

**D. Sorted-index residual (§1.3 — needs investigation + design):**

7. **`crates/shamir-index/src/legacy/sorted_index_manager.rs`** (or
   wherever `SortedIndexManager` lives) — add a generation counter
   mirroring `IndexRegistry::generation`, OR re-order
   `create_sorted_index` to register AFTER backfill (closing the
   register-before-ready window directly). The re-ordering is simpler but
   changes the cancel-safety contract (the doc at `:7-16` says
   `cancel-safe: NO` because register precedes backfill — re-ordering
   makes a mid-backfill cancel a clean no-op). Step 2 should evaluate
   both and pick based on whether the live `on_record_created` hook
   already catches concurrent writes for a registered-but-backfilling
   sorted index (investigate `sorted_index_manager::on_record_created`).

**E. Planner Ready-gating scaffold (1 site):**

8. **`crates/shamir-engine/src/table/read_planner.rs::try_plan_index2`
   (`:32-104`)** — add an in-memory `Ready` check (e.g. a method on
   `IndexBackend` or a wrapper) so a future `Building` backend is
   invisible to the planner. Non-persisted in Step 2 (a forward-compat
   scaffold); the persisted `IndexState` field + restore-on-open is
   Step 3.

### 5.2 Step 2 test plan

- Extend `f50_index_lifecycle_spike_tests.rs` (or fold into a permanent
  `index2_lifecycle_tests.rs`) with: FTS re-derivation, vector
  re-derivation (assert the HNSW graph contains the staged embedding
  post-commit), update-path re-derivation (stage update → create index →
  commit → assert both old and new postings correct), delete-path.
- Add the sorted-index equivalent test once §5.1.D's design is settled.

### 5.3 Step 3 — crash/restart continuation + DDL cancellation

Genuine design work; each item needs its own focused investigation.

9. **Persisted `IndexState` field** — add `state: IndexState` to
   `IndexDescriptor` with `#[serde(default = "IndexState::default_ready")]`
   (forward-compat with existing on-disk metadata, which has no state
   field — all existing indexes are `Ready`). Wire
   `save_index2_metadata` to persist it. On `create_index_v2`, set
   `Building` before backfill, flip to `Ready` after register + final
   save. Recovery replay + doctor `repair()` must handle a `Building`
   index found on disk (resume / restart / drop).

10. **Crash-restart continuation** — a `Building` index found on restart:
    resume the backfill (re-stream from where it stopped — needs a
    checkpoint of the backfill cursor, currently none), restart from
    scratch (simplest — drop + recreate), or leave as a
    permanently-`Building` orphan (the doctor repairs). The reserve-persist
    (#534 finding 2) already prevents id reuse; the orphan-posting
    problem (postings a concurrent tx wrote via Phase 2.7 re-derivation,
    under an id that vanishes on restart) needs a GC path or an
    accept-and-document decision.

11. **DDL cancellation** — a `DROP INDEX` or explicit cancel issued
    mid-build: cleanly interrupt the backfill stream (currently
    `cancel-safe: NO` per the doc), clean up orphan postings under the
    reserved id, and remove the registry entry. Interacts with any
    in-flight tx that already captured a generation including the
    now-cancelled backend (its Phase 2.7 re-derivation would find the
    backend absent via `backends_newer_than` — already correct, since
    `remove_by_id` bumps the generation and the backend is gone from
    `by_id`).

12. **Update `KNOWN_LIMITATIONS.md`** — once Step 2 + Step 3 land,
    document the closed gap and any residual (e.g. crash-restart's
    resume-vs-restart choice).

---

## 6. Exact commands to reproduce

```
# Compile check:
cargo check -p shamir-index -p shamir-tx -p shamir-engine --tests

# Run the spike's red→green proof (2 tests):
./scripts/test.sh -p shamir-engine -- f50_index_lifecycle_spike

# Run the full index2_create_barrier suite (7 tests, includes the renamed Part B closed test):
./scripts/test.sh -p shamir-engine -- index2_create_barrier

# Full integration/e2e scope:
./scripts/test.sh -p shamir-engine --full

# Gate checks:
cargo fmt -p shamir-engine -p shamir-index -p shamir-tx -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `2 tests run: 2 passed` for the spike invocation; `7 tests run:
7 passed` for the barrier suite; clippy `0` warnings/errors workspace-wide.

The red proof (fix disabled) is reproducible by commenting out the
`redrive_index2_ops_post_stage(tx, repo).await;` call in `pre_commit_prelock`
(`pre_commit.rs:553`) and re-running the spike invocation — the
`tx_staging_before_index_register_commits_with_posting_after_fix` test
fails at `:173` with the "guaranteed miss" message; the quiescent control
still passes.
