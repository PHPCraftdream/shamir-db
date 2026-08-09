# RFC: Online CREATE INDEX (snapshot → CDC → catch-up → short barrier)

**Status: DRAFT — pending review**
**Version:** v2 — revised 2026-08-09 (see revision notes below)
**Author:** S.H.A.M.I.R. DB engineering
**Date:** 2026-08-07
**Tracks:** #1018 (P1-5, this RFC), #969 (P1-4, the bench + operational-warning
this RFC replaces the recommendation for), #1044 (PERF backlog, unique-family
O(table) memory — related but NOT solved here), #1048 (P1-2 sub-slice B, the
tombstone `op_id` carry this RFC's §5 slicing depends on for CREATE's own
future op-status wiring).

## Revision log (v2)

This revision incorporates the findings of `2026-08-09-p1054-write-path-audit.md`
and corrects four technical gaps identified during code review:

1. **Removed Open Question 1 (§6)** — The RFC incorrectly claimed no `pub` table-wide
   "current committed version" accessor existed. `MvccStore::current_committed_version()`
   (`crates/shamir-tx/src/mvcc_store/mod.rs:266`) is already `pub` and used by
   `TableManager::mvcc_store()` (`table_manager.rs:1290`) for exactly this purpose in
   sorted-index backfill. §2.2's pin description now cites this existing primitive.

2. **Corrected §2.2's `snapshot_stream` framing** — The RFC called this "the single
   largest new piece of engine machinery." False: `MvccStore::current_stream_impl`
   (`crates/shamir-tx/src/mvcc_store/mod.rs:1347-1372`) is already version-pinned at
   stream-open (captures `floor = self.gate.last_committed()` at line 1356). The only
   change needed is exposing `floor` as a parameter instead of hardcoding it — a
   thin wrapper (`snapshot_stream(batch, at_version)`), not new machinery.

3. **Added SnapshotGuard to Phase A (§2.2, §3, §4)** — The RFC lacked GC protection for
   the pinned version during Phase A's potentially long-running scan. MVCC GC uses
   `min_alive()` to decide what versions to collect; without a registered snapshot,
   versions Phase A needs can be reclaimed mid-scan. Phase A must acquire and hold a
   `RepoTxGate::open_snapshot()` RAII `SnapshotGuard` for the entire scan duration
   (`crates/shamir-tx/src/repo_tx_gate.rs:356`, exported at `lib.rs:74`). Crash safety:
   a crash drops the guard with everything else, and restart re-pins a fresh version
   (matches §4.2's conservative restart-from-scratch policy).

4. **Rewrote §2.3 (Phase B) based on the write-path audit** — The RFC's "live write-hook"
   abstraction doesn't exist as described. The actual mechanism: tx-staged writes and
   non-tx CRUD writes both funnel through `IndexManager::plan_record_created`/
   `plan_record_updated`/`plan_record_deleted` (and `SortedIndexManager` equivalents),
   which iterate ALL registered index defs with NO `IndexState` filter. Dirty-set capture
   is an `IndexState`-conditional inside these shared planning methods — when a def is
   `Building` AND has an active in-flight-build registry entry, route to dirty-set
   capture instead of producing direct `SetPosting`/`RemovePosting` ops. This is a single
   choke point, not ~15 scattered call sites (see the audit for full analysis).

5. **Threw dirty-set design through the document** — Per an operator decision (2026-08-09),
   this RFC uses a **dirty-set** of touched `RecordId`s (no values, no `seq` tracking), not
   a CDC log with `(RecordId, DeltaOp)` values. Phase C re-reads each id at the current
   version and recomputes its posting directly. Idempotency and last-write-wins fall out
   by construction. Updated §2.3 (mechanism), §2.4 (Phase C), §3 Claim 2 (concurrency
   argument now simpler, not weaker), and §4.2 (crash recovery matrix tradeoffs).

> This is a **proposal** for review, not a final contract. Every code
> reference below is illustrative unless explicitly marked "existing". Every
> claim about *existing* behavior is grounded in a file + line range read for
> this RFC (re-verified against the working tree immediately before this RFC
> was finished — see the note at the end of each citation block). This
> mirrors the citation discipline of
> `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md`.

---

## 0. TL;DR

`CREATE INDEX` (regular/unique/sorted families) acquires F-70's write barrier
(`begin_write_barrier` → raise intent bit → drain in-flight fast-path writers
→ acquire `unique_write_lock`) and holds it across the **entire**
Phase 1 (register `Building`) → Phase 2 (backfill scan) → Phase 3 (flip
`Ready`) sequence — see
`crates/shamir-index/src/base_index/index_manager.rs:1501-1519`'s own doc
comment, titled "Concurrency — write-delta catch-up is FREE", which states
this is a **deliberate trade**: holding the barrier for the whole build makes
delta-catch-up trivial (the live write-hook activated at Phase 1 registration
handles it, because nothing else can write concurrently), at the cost of every
writer queueing for the FULL build duration. The `f78_writer_latency.rs` bench
(`crates/shamir-engine/benches/f78_writer_latency.rs`) measures this directly:
at 100k rows the build takes ~140–160s and writer p50/p95/p99 ≈ 140–160s — a
**2.5-minute write outage**, not a brief pause; the scan is superlinear, so 1M
rows extrapolates to hours (`docs/guide-docs/KNOWN_LIMITATIONS.md`, §3,
"CREATE INDEX blocks all writers for the ENTIRE backfill scan").

**Recommendation:** replace the whole-build barrier with the four-phase online
build the task's own framing proposes — **snapshot scan** (barrier-free, pinned
to a single MVCC version) → **durable delta capture** (a bounded, per-index
"CDC log" of committed writes that landed on the table during the scan) →
**catch-up** (replay captured deltas against the partially-built index,
looping until the residual is small) → **short publish barrier** (hold F-70's
barrier only to atomically replay the final residual and flip `Building` →
`Ready`). This shrinks the writer-stall window from `O(build duration)` to
`O(final catch-up batch)` — milliseconds instead of minutes, independent of
table size.

**Scope for this pass:** design only, no code. §5 proposes a slicing that
lands the **regular (hash) family first**, defers unique (needs its own
duplicate-detection story under concurrent writes — see §5.2) and sorted
(needs the rekey-settle interaction worked out — see §5.3), and explicitly
does **not** solve #1044 (unique-family O(table) *memory*, a different axis
from writer-stall time — see §2.6 for the relationship).

---

## 1. Problem statement

### 1.1 What the barrier protects today (verified from source)

`TableManager::begin_write_barrier` (`crates/shamir-engine/src/table/table_manager.rs:979-1007`)
is the **canonical** DDL write-barrier acquisition path (F-70, #897):

```
1. self.ddl_admission.clone().lock_owned().await   // per-table DDL admission mutex
2. WriteBarrierGuard::set(flags, bit, admission)    // raise intent bit — new writers take slow path
3. self.drain_writers().await                       // wait for in-flight fast-path writers
4. self.unique_write_lock.clone().lock_owned().await // acquire the lock LAST
```

This exact order (admission → raise bit → drain → lock) is load-bearing and
documented at length in
`crates/shamir-engine/src/table/writer_drain_barrier.rs:50-146` ("F-70 — THE
canonical lock-order hierarchy"): the *reverse* order (lock-then-drain, F-57's
original shape) is a proven, reproduced deadlock against the tx-commit path's
own drain-guard-then-lock shape on a second table
(`crates/shamir-engine/src/table/tests/f70_lock_order_inversion_tests.rs`).
**Any online-build redesign that still needs a barrier for its final step
MUST go through this exact same entry point, in this exact same order** — the
"short publish barrier" phase proposed here is not a new locking primitive, it
is the *same* `begin_write_barrier` call, just held for `O(catch-up residual)`
instead of `O(whole build)`.

`TableManager::create_index` (`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:622-713`)
calls `begin_write_barrier(REGULAR_INDEX_CREATE)` at line 646-648 and holds
BOTH the returned `WriteBarrierGuard` (`_barrier`) and the
`OwnedMutexGuard<()>` (`_uwl_guard`) for the **entire** method body — the
preflight name check, the interner persist, AND the full call into
`IndexManager::create_index_from_stream`
(`crates/shamir-index/src/base_index/index_manager.rs:1528-1673`), which is
itself the full Phase 1→2→3 sequence. The unique family
(`create_unique_index`/`create_unique_index_body`,
`table_manager_index_mgmt.rs:731-849`) and the index2 family
(`create_index_v2`, `table_manager_index_mgmt.rs:140-142`) do the same —
acquire the barrier once, hold it across register+backfill+publish.

`create_index_from_stream`'s own doc comment
(`index_manager.rs:1501-1519`) states the invariant this whole design is built
on, verbatim:

> "The caller (`TableManager::create_index`) holds F-70's write barrier
> (`begin_write_barrier(REGULAR_INDEX_CREATE)` → raise bit → drain →
> `unique_write_lock`) across the ENTIRE Phase 1→2→3 sequence, so no
> concurrent writer can land a row *during* this loop... So streaming
> introduces no new lost-write window and needs no new catch-up mechanism...
> Reducing writer-blocked time would require releasing the barrier between
> batches, which is explicitly out of scope."

That "explicitly out of scope" is precisely what this RFC now scopes in.

### 1.2 The measured severity

`crates/shamir-engine/benches/f78_writer_latency.rs` (module doc,
lines 1-81) runs `create_index` concurrently with 64 writers and reports the
build duration and writer p50/p95/p99 at three scales:

| scale | build duration | writer p50/p95/p99 |
|---|---|---|
| 5k rows | 147–168 ms | ≈135–160 ms (≈ build duration) |
| 100k rows | ~140–160 s | ≈140–160 s (≈ build duration) |
| 1M rows | not run (extrapolated: hours — superlinear scan) | — |

The bench's own doc explains why writer latency tracks build duration
exactly: "all 64 writers queue on `unique_write_lock` for ~(build duration)
then drain" (lines 56-63). This is confirmed structurally, not just
empirically, by §1.1's barrier-scope reading — it is not a bug, it is the
barrier doing exactly what it is coded to do.

`docs/guide-docs/KNOWN_LIMITATIONS.md` §3 (lines 351-397) documents this as a
known, accepted alpha-stage limitation with an operational workaround ("run
`CREATE INDEX` on large tables during a maintenance window") and explicitly
flags the target architecture this RFC now designs:

> "A full lock-free 'online build' (persist `Building` → snapshot version →
> lock-free bulk scan → delta replay → short cutover → `Ready`, releasing the
> barrier between batches) is planned as a future improvement." (lines
> 393-397)

### 1.3 Why this needs a redesign, not a bigger batch size

F-78 (#905, already landed) already converted the regular family's backfill
from materializing the whole table (`Vec`) to a batched stream
(`create_index_from_stream`, O(batch) memory) — but the bench's own doc
(lines 24-33) confirms this did **not** touch writer latency at all: "The
build itself is decode-bound... so its wall-time — and therefore the
writer-blocked time — is ~UNCHANGED by F-78's memory-only fix." Batch size is
orthogonal to barrier *scope*. The barrier is held across every batch, not per
batch, so shrinking batches shrinks nothing about writer-stall duration. The
fix has to change *when* the barrier is held, not how the scan is chunked.

---

## 2. Design

### 2.1 Overview — four phases, only the last one barriered

```
Phase A: Snapshot scan   (barrier-free, version-pinned, existing code reused)
Phase B: Delta capture   (durable, keyed to this build, running WHILE A runs)
Phase C: Catch-up        (barrier-free, replay captured deltas, looped)
Phase D: Publish barrier (F-70 barrier, replay FINAL residual + flip Ready)
```

Phase A and Phase B run concurrently (B starts before A, so no delta window
is missed — see §2.3). Phase C runs after A finishes, and may loop multiple
times if new deltas keep arriving faster than they can be drained (see
§2.5's convergence criterion). Phase D is the only phase holding
`begin_write_barrier`.

### 2.2 Phase A — snapshot scan (no barrier)

**What changes.** `create_index_from_stream`'s Phase 2 body
(`index_manager.rs:1564-1635`, the `while let Some(batch) = stream.next()`
loop) is unchanged in *shape* — it still consumes a
`Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>>` batch-wise and writes
postings via `set_many`. What changes is the **stream itself**: today's
caller builds it from `TableManager::list_stream`
(`table_manager_index_mgmt.rs:703`, itself
`table_manager_streaming.rs:91-116`), which wraps
`MvccStore::current_stream` (`crates/shamir-tx/src/mvcc_store/mod.rs:1307-1312`)
— a walk of the **live current-winner keyspace**, not a version-pinned
snapshot (verified: `current_stream_impl`'s doc at `mvcc_store/mod.rs:1325-1333`
explicitly contrasts it with `AsOf`'s `get_at(id, pinned_version)`, confirming
`current_stream` has no snapshot-version parameter at all).

The online build needs a **version-pinned** scan so Phase B (§2.3) can define
"everything after this version is a delta" precisely. The codebase already has
the primitive: `MvccStore::get_at(key, snapshot_version)`
(`mvcc_store/mod.rs:1097`) is exactly what `read_as_of`
(`crates/shamir-engine/src/table/read_temporal.rs:45-72`) uses to serve
`AsOf(Version(v))` reads. Phase A's snapshot scan needs an **enumeration**
variant of this idea — walk the keyspace as of a pinned version, not just
point-lookup it. **This is NOT new machinery:** `MvccStore::current_stream_impl`
(`crates/shamir-tx/src/mvcc_store/mod.rs:1347-1372`) is already version-pinned at
stream-open — it captures `floor = self.gate.last_committed()` at line 1356,
filters "newest version ≤ floor" in its group-by state machine, and merges the
overlay via `self.overlay.snapshot_le(floor)` (lines 1367-1372). The only change
needed is exposing `floor` as a parameter instead of hardcoding it to
`last_committed()` — a thin wrapper `snapshot_stream(batch, at_version)` that
calls `current_stream_impl` with the caller-specified floor. This is tens of
lines of parameterization, not new engine machinery.

**What the pinned version is.** At Phase A's start, before any scan work:
read the table's current committed version via `MvccStore::current_committed_version()`
(`crates/shamir-tx/src/mvcc_store/mod.rs:266`, already `pub`) and
`TableManager::mvcc_store()` (`crates/shamir-engine/src/table/table_manager.rs:1290`,
already `pub`). This accessor is already used for exactly this purpose in
sorted-index backfill (`table_manager_sorted_index.rs:286`), so the design reuses
an existing primitive without adding new surface.

**GC protection — holding the version alive during the scan.** Phase A runs for
potentially minutes without a barrier, so MVCC garbage collection must not reclaim
the versions Phase A's scan still needs. MVCC GC uses `min_alive()`
(`crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:286`) to determine which versions to
collect — "the oldest live snapshot, or `last_committed` when no snapshot is open."
Without a registered snapshot, `min_alive` tracks the moving watermark and can
collect versions Phase A's scan still needs mid-scan.

Phase A must acquire and hold a `RepoTxGate::open_snapshot()` RAII
`SnapshotGuard` (`crates/shamir-tx/src/repo_tx_gate.rs:356`, exported at
`crates/shamir-tx/src/lib.rs:74`) for the ENTIRE duration of the scan — released
only when Phase A completes or the build aborts. The guard registers the version
in `active_snapshots`, keeping it alive for GC purposes until the guard drops.

Crash safety: a crash drops the guard along with everything else. Restart re-pins
a FRESH version (the old pin is stale — time has passed, more writes landed), which
matches §4.2's conservative restart-from-scratch policy (Phase A is always redone
from scratch after a crash, never resumed). This is not a new hazard beyond what
§4.2 already handles.

### 2.3 Phase B — durable delta capture (concurrent with Phase A)

**The core problem this phase solves.** While Phase A's snapshot scan is
in flight (not barriered — writers proceed normally), some writer commits a
row that is (a) newer than the snapshot's pinned version, and (b) never seen
by Phase A's enumeration. That row must not be lost from the index being
built.

**Mechanism: capture dirty `RecordId`s via a single choke point.**
Per the write-path audit (`2026-08-09-p1054-write-path-audit.md`), tx-staged
writes and non-tx CRUD writes both funnel through the SAME shared planning
methods:
- `IndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted`
  (`crates/shamir-index/src/base_index/index_manager.rs:2023`, `:2074`, `:2130`)
- `SortedIndexManager::plan_record_created`/`plan_record_updated`/`plan_record_deleted`
  (`crates/shamir-index/src/base_index/sorted_index_manager.rs:1564`, `:1644`, `:1713`)

These methods iterate over **all registered index definitions with NO filter on
`IndexState`** — they produce `SetPosting`/`RemovePosting` ops for `Building`
defs just as readily as for `Ready` defs. This is why the dirty-set capture
belongs INSIDE these shared methods, NOT at ~15 scattered call sites in
`table_manager_tx_ops.rs` and `table_manager_crud.rs`.

**The dirty-set design (operator decision, 2026-08-09).** This RFC uses a
**dirty-set** of touched `RecordId`s (no values, no `seq` tracking), not a
CDC log with `(RecordId, DeltaOp)` values. Storage cost is O(distinct rows
touched) instead of O(writes × value size). Phase C (§2.4) re-reads each id at
the current version and recomputes its posting directly. Idempotency and
last-write-wins fall out by construction — recompute-from-current-state is
inherently idempotent, and the last write wins because we read the final
committed state.

**Capture logic inside the shared planning methods:**

1. Acquire `begin_write_barrier(REGULAR_INDEX_CREATE)` — same call, same
   order, as today.
2. Under the barrier: read the pinned snapshot version (§2.2), acquire the
   `SnapshotGuard` for GC protection, register the index definition at
   `Building` (identical to today's Phase 1, `index_manager.rs:1559-1562`).
3. **Release the barrier immediately** (drop the guard) — this is the "short"
   part; step 1-2 together are sub-millisecond.
4. Only NOW start Phase A's (barrier-free) snapshot scan, reading at the
   pinned version from step 2.
5. **Dirty-set capture starts the moment the def is registered at `Building`**:
   Inside each shared planning method's loop over defs, check:
   - If `def.state == IndexState::Building` AND this Building index has an
     active in-flight-build registry entry (a new per-`IndexManager` registry,
     one entry per name, mirroring the existing `in_flight_creates` RAII-guard
     set already used for `degraded_index_count()` bookkeeping,
     `table_manager_index_mgmt.rs:630`):
     - Add the `RecordId` to the dirty-set for this index (persisted in
       `info_store` under a key prefix like `system:ddl_dirty_set:<build_id>`).
     - Do NOT produce a `SetPosting`/`RemovePosting` op for this specific def
       (or produce it AND dirty-set — either works, but skipping the op is
       simpler and avoids unnecessary posting writes).
   - If `def.state == IndexState::Ready` (or no active build):
     - Produce the `SetPosting`/`RemovePosting` op as usual.

This single-choke-point design works for both tx-staged and non-tx CRUD writes
without duplication, avoids missing a call site in future refactors, and
matches the audit's finding that both paths share the SAME underlying planning
mechanism.

**Storage and GC:** The dirty-set lives in the same `info_store` the tombstones
and the new `ddl_op_log` module already use
(`crates/shamir-engine/src/table/ddl_op_log.rs:1-11`'s own doc explicitly
states it "lives in the same `info_store` that tombstones use"). A new key
prefix `system:ddl_dirty_set:<build_id>` stores the set of touched `RecordId`s
as a compact set (e.g., a roaring bitmap or a sorted array of `RecordId`s —
the exact encoding is an implementation detail, not a design constraint). The
entire dirty-set for a `build_id` is deleted once Phase D completes (either
success or a permanent abort). A crash mid-build leaves the dirty-set around;
see §4 for the recovery story.

### 2.4 Phase C — catch-up (barrier-free, looped)

Once Phase A's scan completes (every pre-pin row has a posting), Phase C
drains the dirty-set built during Phase A's run:

1. Read all dirty-set entries for this `build_id`.
2. For each `RecordId` in the dirty-set:
   - Re-read the record at the **current version** (not the pinned version)
     via `MvccStore::get_at(record_id, current_committed_version())`.
   - Recompute the posting by calling the SAME planning methods that a live
     `Ready` index uses (`IndexManager::plan_record_created`/`plan_record_updated`/
     `plan_record_deleted` for the current record state). This is NOT new
     posting-maintenance logic — it's the same code path, just invoked from
     the catch-up loop instead of inline from the write path.
   - Apply the computed posting to the building index's posting keyspace.
3. Because Phase A can take minutes, MORE writes will have accumulated by
   the time step 1-2 finish than existed when step 1 started (writers continue
   unobstructed during Phase C too). Loop: go back to step 1.
4. **Convergence criterion** (the "moving target" problem the task brief
   flags): stop looping and proceed to Phase D once EITHER (a) a full
   iteration of step 1-2 finds the dirty-set empty (caught up), OR (b) the
   dirty-set size drops below a small fixed threshold (e.g. `< 100` entries,
   tunable) AND has been non-increasing for N consecutive iterations (prevents
   chasing a workload with sustained write throughput above the catch-up apply
   rate forever). Whichever fires first hands off to Phase D with a small,
   bounded residual to finish under the barrier. **This is a genuine open
   design point flagged for review, not fully settled here — see §6.2.**

This phase never holds `unique_write_lock` or raises a `WriteBarrierFlags`
bit — writers continue completely unobstructed. Only the actual dirty-set
drain work (bounded by however many distinct rows were touched) consumes time;
no `O(table)` work happens here.

**Idempotency and last-write-wins by construction.** Because Phase C re-reads
each `RecordId` at the CURRENT committed version (not a stored value from the
time of capture), recompute-from-current-state is inherently idempotent —
applying Phase C multiple times for the same id converges to the same final
posting state. Last-write-wins falls out too — we read the final committed
state, so the last write to that id wins by definition. No `seq`/`last_applied_seq`
bookkeeping is needed.

### 2.5 Phase D — short publish barrier

1. Acquire `begin_write_barrier(REGULAR_INDEX_CREATE)` — same call, same
   canonical order as §1.1/§2.3 step 1.
2. Apply the FINAL residual of the dirty-set (whatever accumulated since
   Phase C's last convergence check — bounded by the loop's threshold, i.e.
   small by construction).
3. Flip `Building → Ready` and persist (identical to today's Phase 3,
   `index_manager.rs:1645-1664`).
4. Delete the dirty-set for this `build_id` (GC, §2.3).
5. Release the barrier (RAII drop, same as today).

Because step 2's work is bounded (the convergence criterion from §2.4
guarantees it), the barrier is held for `O(final residual)` time —
milliseconds — not `O(table)` time. This is the entire point of the redesign:
every writer that queues on `unique_write_lock` during Phase D queues for a
duration independent of table size.

### 2.6 Relationship to #1044 (unique-family O(table) memory)

**Different axis — not solved by this redesign, and not made worse by it.**
`create_unique_index_body`'s F-78-deferral comment
(`table_manager_index_mgmt.rs:818-826`) documents that the unique family
still materializes the WHOLE table into a `Vec` before one `set_many`,
because duplicate detection needs global knowledge — this is a *peak-memory*
problem, orthogonal to *writer-stall-duration*, which is what THIS RFC
targets. §5.2 discusses why unique is deferred from slice 1 for a different,
correctness-related reason (concurrent-write duplicate detection during
Phase B/C), but even if that concurrency question is solved, #1044's
memory-materialization problem is independent and remains open — a future
online-unique-build design still needs *some* bounded-memory duplicate-check
strategy (e.g. spilling to a temporary sorted structure, or a probabilistic
pre-filter backed by an exact recheck) regardless of whether the barrier is
held for the whole build or just the tail. This RFC does not propose that
mechanism; #1044 remains its own tracked task.

---

## 3. Concurrency / correctness argument

**Claim 1 — no writer can observe a partially-built index as `Ready`.**
Phase D is the ONLY point that sets `IndexState::Ready`
(mirroring today's single Phase-3 flip site,
`index_manager.rs:1645-1651`). Before Phase D, the index is `Building`, and
`Building` indexes are already planner-invisible today (this invariant is
NOT new — it's the existing `Building`-gate the planner already respects;
`doctor.rs:97-101`'s own doc: "A `Building` index... is permanently
planner-invisible until an explicit `doctor::repair()`"). No new planner
change is needed for this claim; it is inherited unchanged from the existing
state machine.

**Claim 2 — no committed write during the snapshot+catchup window is lost.**
This is the crux the redesign has to prove that today's whole-barrier design
gets "for free."

**This claim depends on the `SnapshotGuard` from §2.2 holding for Phase A's
entire duration.** Without it, MVCC GC could reclaim a version Phase A's
scan still needs mid-scan (`min_alive()` tracks the moving watermark absent
a registered snapshot — `mvcc_gc.rs:286`), silently truncating what "every
write before the pin is captured by the scan" means: a row whose pre-pin
version was garbage-collected would read as absent rather than as its
pinned-version value, which is indistinguishable from a genuinely-deleted
row and would corrupt the built index. The argument below assumes the guard
is held; §2.2 is the enforcement mechanism, this is the correctness
consumer of it.
- Every write that commits **before** Phase B step 2 (index registered at
  `Building`, dirty-set capture now active) is covered by Phase A's snapshot scan
  IF its commit version ≤ the pinned version read in the same step. Since
  step 2 reads the pin and registers the index in the SAME barriered critical
  section (Phase B steps 1-3 all happen under one `begin_write_barrier`
  acquisition), there is no window between "pin chosen" and "dirty-set active"
  for a write to fall into — this is exactly why Phase B needs its own (short)
  barrier acquisition, not merely "start the scan and start dirty-set capture
  independently." A write that raced the barrier itself (arrived before the
  intent bit went up, drained by `drain_writers()`) is guaranteed to have
  committed (and thus be captured by the pin, being ≤ the version read after
  the drain) before Phase A ever starts — same guarantee `begin_write_barrier`'s
  existing drain step already provides for every other DDL path.
- Every write that commits **after** Phase B step 2 (dirty-set active) adds
  its `RecordId` to the dirty-set (§2.3), regardless of whether Phase A has
  already scanned that row or not — the capture does not consult Phase A's
  progress, it unconditionally adds the id. A row updated twice while Phase A
  hasn't reached it will appear once in the dirty-set (duplicates are
  deduplicated by the set's structure), and Phase C will re-read the FINAL
  state at the current version when it drains the set — no bookkeeping of
  intermediate states is needed. This is SIMPLER than the CDC-log+seq argument
  in v1: last-write-wins falls out by construction because we read the final
  committed state.
- **The one genuine new hazard vs. today: Phase A's own write of a posting
  for a row that's ALSO in the dirty-set.** If Phase A's snapshot scan (at
  pinned version V) writes a posting for row R at the V-state, and R was
  ALSO written after V (captured in the dirty-set), Phase A's posting write
  for R and Phase C's recompute for R must not race destructively. This is
  why Phase C's recompute is idempotent (as §2.4 argues) — applying Phase
  A's posting for R, then recomputing R's posting from the current state,
  converges to the SAME final posting state. No lock is needed between Phase
  A's write and Phase C's recompute for the SAME key because both are
  idempotent operations on the SAME (key → posting) mapping — the last one
  to run wins, and "last" is well-defined because Phase C runs strictly
  AFTER Phase A's stream completes (no concurrent execution of the two for
  the same index).

**Claim 3 — no crash leaves an unrecoverable half-state.** See §4 in full;
summary: the dirty-set persists across a crash (durable `info_store` writes,
same durability class as the existing tombstones), so unlike today's
restart-from-scratch model, a crash during Phase C/D can, in principle,
RESUME catch-up from where it left off — see §4.2 for why this RFC still
recommends restart-from-scratch for Phase A itself (matching the existing
precedent) but proposes resumable catch-up as a genuinely new capability THIS
design enables that the old design structurally could not. With the dirty-set
design, resumability is simpler than the CDC-log+seq approach in v1: Phase C
just drains the dirty-set from where it left off (no `last_applied_seq` to
track — the dirty-set is the set of all ids that were touched, and Phase C
recomputes from current state for each, so resuming just means "continue
draining").

**Residual risk this RFC does NOT fully close (flagged for review, §6.2):**
the convergence criterion (§2.4) is a heuristic, not a proof — a sustained
write rate into the indexed table's paths that exceeds the catch-up apply
rate could in theory never converge, looping Phase C forever. §6.2 proposes
a hard iteration cap that forces Phase D regardless (accepting a longer, but
still BOUNDED, final barrier) as the safety valve; this needs reviewer
sign-off on the exact cap.

---

## 4. Crash recovery story

### 4.1 What's genuinely new vs. what's inherited

Today's crash-restart model (F-50 Step 3a/3b, fully proven for index2 and
adapted for base_index by #966/#1013) is **restart-from-scratch**: a crash
leaving a `Building` descriptor causes the self-heal path to drop whatever
partial postings exist and redo the ENTIRE backfill. The decision memo
(`docs/dev-artifacts/research/f50-step3-crash-restart-spike.md`, §2.2)
explicitly rejected resumable backfill as "over-engineering for a rare,
operator-driven DDL path with a checkpoint-less backfill" — because at the
time, the backfill genuinely had no checkpoint, no persisted cursor, and
no idempotency guarantee for a resumed range scan.

**This RFC's design changes the premise that rejection was based on.** The
online build introduces exactly the missing pieces:
- A durable **dirty-set** of touched `RecordId`s (§2.3) — this IS a checkpoint
  mechanism, just not one the F-50 spike had in scope.
- Phase C/D's recompute-from-current-state is ALREADY required to be idempotent
  (Claim 2, §3) for the concurrency argument to hold — so idempotent-resume is
  not extra work, it is a byproduct of correctness the design already needs.

**What is still NOT resumable, and why that's still the right call:** Phase
A's snapshot scan itself. The F-50 spike's core argument — "the scan has no
persisted cursor, and `list_stream`/its successor has no range-resume
variant" — still applies to the NEW `snapshot_stream(batch_size, at_version)`
primitive proposed in §2.2 unless THIS RFC additionally proposes a
resume-from-`RecordId` cursor for it, which it does not (kept out of scope —
see §5.4). So a crash DURING Phase A still means: redo Phase A's scan from
the start, at a FRESH pinned version (the old pin is stale — time has
passed, more deltas exist).

### 4.2 The crash-state matrix

| crash point | on-disk state | dirty-set | recovery action |
|---|---|---|---|
| before Phase B (no barrier acquired yet) | no `Building` descriptor persisted | none | nothing to recover — CREATE never started durably; client sees the connection drop / error, may retry the whole DDL |
| during Phase B (barrier held, registering) | possibly `Building` persisted, dirty-set capture not yet active | none or partial | **restart-from-scratch**, same as today: table-open self-heal (mirroring #966/#1013's existing `Building`-detection) drops any partial postings and the stale dirty-set, re-runs the WHOLE online-build sequence (fresh pin, fresh Phase A) |
| during Phase A (barrier-free scan) | `Building` persisted, dirty-set active | growing | table-open self-heal detects `Building`, discards the stale dirty-set (its ids are relative to a pin that's now behind current state anyway, and Phase C re-reads from current state so the stale dirty-set is harmless but misleading), restarts the WHOLE sequence — same as the row above. **Not resumable, by design (§4.1).** |
| during Phase C (catch-up loop) | `Building` persisted, Phase A's postings ALREADY durably written (`set_many` per batch, same as today) | non-empty, durable | **NEW capability with dirty-set:** recovery can, in principle, resume catch-up from where it left off — Phase C just continues draining the dirty-set (no `last_applied_seq` to track, unlike v1's CDC-log approach). Phase A's own postings are already correct and durable (nothing about Phase A itself needs redoing), only the dirty-set drain needs to continue. This is the concrete payoff of the dirty-set design. **Left as an explicit slice-2+ optimization, not required for slice 1's correctness** — slice 1 may conservatively restart-from-scratch here too (simpler, still correct, just gives up the resumability payoff) until the resume path is itself implemented and tested; see §5.1. **Tradeoff:** dirty-set loses the per-op detail of a CDC-log (no exact sequence of operations to replay, just the set of ids touched). This is accepted as a deliberate design choice — storage is O(distinct rows) instead of O(writes × value size), and recompute-from-current-state is simpler than seq-ordered replay. |
| during Phase D (short barrier held) | `Building` still on disk (Ready-flip is the LAST step, same ordering as today's Phase 3) | small residual | same as today's Phase-3-interrupted case: `Building` on disk, self-heal restarts the whole sequence. Because Phase D is bounded/short by construction, this crash window is proportionally much SMALLER (a few ms of exposure vs. minutes today) even though the recovery action itself is unchanged. |
| after Phase D's Ready-flip persist, before dirty-set GC | `Ready` persisted, correct | stale, unreferenced | recovery (or a lazy periodic sweep) deletes the orphaned dirty-set for this `build_id` — harmless, no correctness impact, purely a cleanup residual (mirrors the existing accepted pattern of a tombstone surviving past its logical need, e.g. `ddl_op_log.rs`'s own `DDL_OP_LOG_CAP`/FIFO-eviction TODO) |

### 4.3 Does the doctor/`verify()`/`repair()` machinery need to change?

**Minimally.** `IndexHealth`/`Index2Health` (`doctor.rs:93-150`) already
report a `Building` index as unhealthy with a diagnostic message,
independent of HOW it got stuck there — that reporting contract does not
need to change. What DOES need a small addition: `repair()`'s existing
rebuild-from-scratch path (`doctor.rs:509+`, `632-652`'s `Building | Failed`
gate) currently assumes "rebuild = re-run the whole barriered build" — for
the online-build design, `repair()` should call the SAME new online-build
entry point (Phase A→D) instead of the old one, so a manually-triggered
repair also gets the reduced-writer-stall benefit, not just a fresh
`CREATE INDEX` call. This is a call-site swap, not a new health-reporting
concept.

### 4.4 Interaction with #1015's `DdlOpStatus`/op-status log

**Conclusion (the task brief asks this to be settled explicitly): CREATE
INDEX status SHOULD eventually be wired into `ddl_op_log`
(`crates/shamir-engine/src/table/ddl_op_log.rs`), and the wire types already
anticipate it — but it should land as its OWN follow-up slice, not bundled
into this RFC's implementation, and the reason is now MORE clearly a
distinct kind of work than pure "wire it up," not less.**

Grounding: `DdlOpKind` (`crates/shamir-query-types/src/read/ddl.rs:27-77`)
already declares `CreateHashIndex { index_name, table_name }` and
`CreateUniqueHashIndex { index_name, table_name }` variants (lines 28-41) —
but grep across the whole workspace confirms these two variants are
constructed **nowhere** in production code today (only referenced inside
`ddl.rs` itself and matched generically in `ddl_op_log.rs`'s storage
primitives, which are kind-agnostic). `admin_table_index.rs`'s CREATE INDEX
handler (`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`,
the `create_index`/`create_unique_index`/`create_index_v2` dispatch around
lines 350-554) calls the plain `admin_result(...)` builder, never
`admin_result_with_op_id(...)` — contrast with the DROP handler at lines
702-737, which mints an `op_id` (`RecordId::system(&format!("ddl_drop_index_{}",
...))`, a NAME-derived deterministic id, not a random one — worth noting for
CREATE too, since CREATE is also name-scoped) and calls
`ddl_op_log::write_op_status` explicitly. **So `CreateHashIndex`/
`CreateUniqueHashIndex` are confirmed dead/placeholder variants today** — the
enum shape was reserved ahead of time (per #1015's RFC §4 explicit deferral:
"`CREATE INDEX` status (it has a `Building` state worth surfacing, but its
recovery story is partly owned by #966 self-heal — needs a careful ownership
split)" — that deferral's cited doc location has since moved with #1048's
edits but the SAME "#966 self-heal ownership resolution" section now lives at
`table_manager_index_mgmt.rs:1174-1190`, and its substance is unchanged: the
base_index family has NO automatic Building self-heal the way index2 does,
so CREATE INDEX's op-status "recovery" story does not cleanly reduce to "the
recovery function writes `SucceededViaCrashRecovery` as it clears a
tombstone" the way DROP/RENAME's does).

**Why THIS RFC sharpens, rather than resolves, that ownership question.**
Under the online-build design, CREATE INDEX gains a genuinely NEW
multi-phase lifecycle (`InProgress[SnapshotScan]` → `InProgress[CatchUp]` →
`Succeeded`) that the current `DdlOpState` vocabulary
(`ddl.rs:82-124`: `InProgress | Succeeded | SucceededViaCrashRecovery |
Failed | Unknown`) does not distinguish at all — a client polling
`GetDdlOpStatus` mid-build today would only ever see undifferentiated
`InProgress`, which is a much weaker "progress visibility" answer than the
task brief's own requirement #5 ("Progress must be queryable, not just
log-visible"). A genuinely useful queryable-progress answer needs an
EXTENSION to `DdlOpState::InProgress` (e.g. a `phase: BuildPhase` field
carrying `SnapshotScan { rows_scanned }` / `CatchUp { residual_deltas }` /
`Publishing`), which is new wire-contract surface, not just "call
`write_op_status` at the existing dispatch/recovery choke points" the way
DROP/RENAME's #1048 sub-slice is. **Recommendation: land this RFC's Phase
A-D mechanism first (its own slice, §5), land basic `op_id` + terminal
`Succeeded`/`Failed` wiring for CREATE as a small follow-up mirroring
#1048's DROP/RENAME pattern (no NEW `DdlOpState` shape needed for that much),
and treat the richer `BuildPhase` progress-detail extension as a SEPARATE,
later follow-up** — bundling all three into one PR would couple a
correctness-critical concurrency redesign to a wire-format extension that
has its own independent review surface (additive `DdlOpState` field, same
`#[serde(default, skip_serializing_if)]` discipline as #1015's RFC §3.1
already established for `QueryResult::op_id`/`ddl_status`).

---

## 5. Rollout / implementation slicing

This is explicitly NOT a one-PR feature — mirroring #1015's RFC §4 style.

### Slice 1 — regular (hash) family only, conservative recovery

**In scope:**
- `MvccStore::snapshot_stream(batch_size, at_version)` (§2.2) — the new
  version-pinned enumeration primitive.
- The per-build dirty-set (§2.3): storage primitives (mirroring
  `ddl_op_log.rs`'s `write`/`read` shape), the in-flight-build registry that
  gates the live write-hook between "direct posting write" (today's
  behavior, unchanged for any index NOT mid-online-build) and "add to
  dirty-set" (new behavior, only for an index actively in Phase B-D).
- Phase A/B/C/D wired into `TableManager::create_index` (regular family
  only — `create_unique_index`/`create_index_v2` UNCHANGED, still today's
  whole-barrier path).
- Crash recovery: **conservative** — restart-from-scratch for EVERY crash
  point in §4.2's matrix, including the Phase-C row (the resumable-catch-up
  optimization is explicitly deferred to slice 2, per §4.2's own note). This
  keeps slice 1's correctness surface identical in KIND to the existing,
  already-proven F-50/#966 restart-from-scratch model — only the STEADY-STATE
  (non-crash) writer-stall behavior changes.
- `f78_writer_latency.rs` extended (see §7) to prove the writer-stall
  reduction for the regular family at the SAME 5k/100k/1M scales already
  benchmarked.
- `doctor::repair()`'s regular-family rebuild path swapped to call the new
  online-build entry point (§4.3).

**Deferred out of slice 1 (explicit, with reasons):**
- Unique family (§5.2).
- Sorted family (§5.3).
- index2 family (§5.4).
- Resumable Phase-C-crash recovery (§4.2's deferred row) — ship
  restart-from-scratch first, add resume once the dirty-set mechanics are
  proven in production-shaped tests.
- `DdlOpStatus` wiring for CREATE INDEX (§4.4) — its own follow-up slice(s),
  NOT bundled here.
- The convergence-criterion tuning (§2.4/§6.2) — ship with a conservative
  fixed cap (e.g. a hard N-iteration ceiling that forces Phase D regardless
  of residual size, same "ship a fixed cap, tune later" discipline #1015's
  RFC used for the op-status log's retention policy, §4 there).

### Slice 2 — unique family

**Why deferred, not just "later for schedule reasons."** The unique family's
correctness argument (§3, Claim 2) gets materially harder: duplicate
detection needs GLOBAL knowledge of all keys seen so far, but under the
online design, Phase A's snapshot scan and Phase C's dirty-set drain are
happening at DIFFERENT times against a growing key set — a duplicate could be
introduced by a dirty-set-captured write (a row updated to collide with an existing value)
AFTER Phase A already validated no-duplicates-as-of-the-pin. Today's
whole-barrier design sidesteps this entirely (nothing else can write, so
"no duplicates" is checked once, atomically, against a fully static view).
The online design needs an explicit answer to "what happens when a
Phase-B-captured delta would introduce a duplicate against the
snapshot-built index" — options include (a) re-validating uniqueness at
Phase C/D replay time and failing the WHOLE build if a delta-introduced
duplicate is found (simple, but means a build can fail late, after most of
the work is done, for a reason unrelated to the pre-existing data), or (b)
holding a NARROWER barrier just for the specific key range a delta touches
during replay (more complex, avoids failing late). **This RFC does not pick
between (a) and (b) — flagged as an open question, §6.3.** Slice 2 should
resolve this BEFORE implementation starts, likely with its own short design
note.

### Slice 3 — sorted family

**Why deferred.** The sorted family has its OWN concurrent-mutation
machinery already (the "rekey settle loop" referenced in #1015's RFC's
migration-poll-precedent framing and P0-3a's plan doc,
`docs/dev-artifacts/research/2026-08-06-p0-3a-reader-drain-gate-plan.md:48`:
"Slice 2 — sorted family (`SortedIndexManager`, 8 chokepoints)" for the
READER side of a similar problem). A sorted index's backfill also has
ordering invariants (key-range structure) a hash index's flat posting-set
backfill does not — interaction between THIS RFC's dirty-set drain and the
sorted family's existing rekey/settle machinery needs its own investigation
before slicing in an online build for it. Mechanically similar in spirit to
slice 1, but not "the same code with a different backend" — deferred.

### Slice 4 — index2 family (fts/functional/vector)

**Why deferred, and why it may need a DIFFERENT mechanism entirely, not just
a later slice of the SAME one.** `create_index_v2`'s own doc comment
(`table_manager_index_mgmt.rs:110-122`) already documents a PARTIAL,
pre-existing gap for THIS family specifically: the write barrier "does NOT
reach the tx-commit path... which is how every real client DML statement is
actually served" — meaning index2's barrier is ALREADY narrower in coverage
than the base_index families' barrier, for unrelated historical reasons (the
commit pipeline plans index2 ops at STAGE time against an `all_backends()`
snapshot, materializing later at commit Phase 5a, neither of which consults
the barrier flag today). Layering a dirty-set online-build design on top of
an ALREADY-incomplete barrier is a bigger, structurally different problem
than slices 1-3, which all build on top of a barrier that IS complete for
their write paths. index2 needs its own scoping pass, likely starting from
closing THAT pre-existing gap first.

---

## 6. Open questions for review

1. **Convergence criterion — exact thresholds.** (§2.4) "Zero new deltas OR
   residual < threshold for N iterations" is a shape, not a number. Needs
   reviewer input on the threshold/N (or agreement to ship the
   simpler "hard iteration cap, unconditionally publish after cap" version
   for slice 1 and treat the adaptive version as a slice-1.5 refinement).

2. **Unique-family duplicate-detection strategy under concurrent deltas.**
   (§5.2) Option (a) late-fail-whole-build vs. (b) narrow per-key barrier
   during replay — genuinely undecided, needs its own short design pass
   before slice 2 starts.

3. **Resumable Phase-C crash recovery — worth the complexity, or is
   restart-from-scratch acceptable indefinitely?** (§4.2) Slice 1 ships
   conservative (always restart-from-scratch). Given that Phase D's barrier
   is already short, is a crash specifically inside Phase C rare/cheap
   enough that resumability is never worth building? Lean "build it
   eventually" (the dirty-set makes it nearly free once proven), but flag
   for reviewer — this could also be explicitly deferred forever if the
   team judges Phase-C crash windows rare enough in practice.

4. **Should the dirty-set be per-index-build or per-table?** (§2.3) This
   RFC assumes per-`build_id` (one dirty-set per in-flight CREATE INDEX). If
   MULTIPLE indexes are ever created concurrently on the same table (today's
   `ddl_admission` mutex serializes DDL per table, so this cannot happen
   currently — but if `ddl_admission`'s per-table serialization is ever
   relaxed, e.g. to allow concurrent creates on DIFFERENT indexes of the
   SAME table), would a shared per-table dirty-set (filtered by which
   index's paths a write touches) be more efficient than N independent full
   per-build dirty-sets each duplicating the same underlying write stream? Not
   urgent (today's serialization makes this moot), but worth a one-line
   decision so slice 1's storage-key scheme doesn't need to change later.

5. **`DdlOpState` extension for build-phase progress** (§4.4) — new
   `BuildPhase` sub-status, additive field, exact shape. Deliberately NOT
   designed in this RFC (its own follow-up), but flagged so reviewers know
   it is coming and can weigh in on whether it should piggyback on THIS
   RFC's implementation slices or genuinely wait.

---

## 7. Bench / test plan

### 7.1 What `f78_writer_latency.rs` should measure after the redesign

The bench's fundamental shape (spawn `create_index`, spawn N concurrent
writers shortly after, measure writer p50/p95/p99) stays — but its
INTERPRETATION changes. Today, "writer p95 ≈ build duration" is the
EXPECTED, correct-by-design result (the bench's own doc says so explicitly,
`f78_writer_latency.rs:56-63`). After this redesign, the bench should assert
(or at minimum report clearly enough for a human to assert) the OPPOSITE:
**"writer p95 should be bounded by Phase D's residual-apply time, decoupled
from Phase A's scan duration"** — concretely, add a NEW scenario variant
that:
- Runs the SAME 5k/100k/1M scale matrix.
- Reports Phase A's scan duration and Phase D's barrier-hold duration
  SEPARATELY (today's bench only reports one "build" duration because
  there's only one phase that matters for writer latency; the new bench
  needs to expose the phase split, likely via a new tracing span or a
  return-value breakdown from the (now internal, test-only) online-build
  entry point).
- Asserts (or reports for a human gate, if a hard assertion is judged too
  brittle for CI) that writer p95/p99 tracks Phase D's duration, NOT Phase
  A's — e.g. `writer_p95_ms < phase_d_ms * K` for some slack constant K,
  while `phase_a_ms` is allowed to be arbitrarily large (the whole point).
- Keeps the OLD unmodified scenario (today's whole-barrier path, if it
  remains reachable for the unique/sorted/index2 families per §5's slicing)
  running unchanged, so the bench continues to serve as the evidence trail
  for KNOWN_LIMITATIONS.md's claims about the families NOT yet migrated.

### 7.2 Correctness tests needed (new, not just the bench)

Mirroring the existing test-file conventions
(`crates/shamir-engine/src/table/tests/`, one file per topic, `tests/mod.rs`
manifest-only per this repo's §"Test organisation" rule):

1. **Delta-capture completeness proof** — deterministic pause-hook test
   (mirroring `f76_drop_visibility_tests.rs`'s / P0-3a's proof-test pattern,
   `2026-08-06-p0-3a-reader-drain-gate-plan.md` §4.1): pause Phase A
   mid-scan via a new test seam, issue N writes (insert/update/delete mixed)
   from a separate task, resume Phase A, assert every one of the N writes'
   FINAL state is correctly reflected in the built index's postings —
   proves Claim 2 (§3) empirically, not just by argument.
2. **Convergence-loop termination test** — a synthetic write-generator that
   sustains a rate ABOVE the catch-up apply rate for a bounded time, then
   stops; assert Phase C eventually converges (or, if testing the hard-cap
   variant, assert Phase D is forced after the cap and STILL produces a
   correct index once Phase D's own residual-apply completes).
3. **Publish-barrier boundedness test** — assert Phase D's barrier-hold
   duration is `O(residual)`, independent of table size, by running the
   SAME residual-delta-count scenario at two wildly different Phase-A table
   sizes (e.g. 1k rows vs 500k rows) and asserting Phase D's duration is
   statistically indistinguishable between them (this is the test-level
   expression of the bench's assertion in §7.1, but as a hard pass/fail
   correctness gate rather than a reported number).
4. **Crash-recovery matrix tests** — one test per row of §4.2's table,
   using the SAME "pause hook + drop the manager + reopen" pattern the
   existing F-50/#988/#997 recovery tests already use (e.g.
   `p03b_index2_drop_durability_tests.rs`'s pattern, referenced at
   `table_manager_index_mgmt.rs:63`), confirming restart-from-scratch
   recovers correctly from every crash point in slice 1's conservative
   design.
5. **Regression sweep** — re-run the existing `f78_writer_latency`
   correctness-equivalence test (materialize-vs-stream postings-identical
   assertion, mentioned in `create_index_from_stream`'s "Why a separate
   method" doc, `index_manager.rs:1483-1490`) against the NEW online-build
   path too, proving the dirty-set-mediated build produces byte-identical
   posting sets to the old whole-barrier build for the SAME fixture with NO
   concurrent writes (the degenerate case where Phase B/C/D's dirty-set is
   empty).

---

## Appendix A — primary sources read for this RFC

- Barrier mechanism: `crates/shamir-engine/src/table/table_manager.rs:930-1150`
  (`unique_write_lock`, `begin_write_barrier`, `set_schema_activation_barrier`,
  `drain_writers`, `enter_writer_drain` doc comments); full module doc of
  `crates/shamir-engine/src/table/writer_drain_barrier.rs:1-146` (F-48/F-56/
  F-69/F-70 history and the canonical lock-order hierarchy); bit constants in
  `crates/shamir-index/src/base_index/write_barrier_flags.rs:106-138`.
- CREATE INDEX call sites: `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1-849`
  (`create_index_v2`, `create_index`, `create_unique_index`/
  `create_unique_index_body`), `:1000-1230` (`recover_index2_drops`,
  `recover_hash_renames` doc incl. the "#966 self-heal ownership resolution"
  section at `:1174-1190`).
- Streaming backfill: `crates/shamir-index/src/base_index/index_manager.rs:1440-1673`
  (`create_index_from_records`'s Phase 3 tail, `create_index_from_stream`'s
  full doc + body).
- Reader-drain prior art: `crates/shamir-index/src/reader_drain_gate.rs:1-90`
  and `docs/dev-artifacts/research/2026-08-06-p0-3a-reader-drain-gate-plan.md`
  (full document) — precedent for a flag+counter primitive design and its
  ABBA-deadlock placement argument.
- Crash-restart precedent: `docs/dev-artifacts/research/f50-step3-crash-restart-spike.md`
  (full document, esp. §2 "restart-from-scratch DECIDED" reasoning).
- Doctor / health reporting: `crates/shamir-engine/src/table/doctor.rs:1-150`
  (`VerifyReport`, `IndexHealth`, `Index2Health`), `:509-652` (`repair()`'s
  `Building | Failed` gate).
- `IndexState`: `crates/shamir-index/src/state.rs:49-64`.
- DDL result contract / op-status log: `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md`
  (full document, cited for style + the migration-poll precedent + retention
  policy discipline); `crates/shamir-query-types/src/read/ddl.rs:1-125`
  (`DdlOpStatus`/`DdlOpKind`/`DdlOpState`); `crates/shamir-engine/src/table/ddl_op_log.rs:1-86`
  (storage primitives + `DDL_OP_LOG_CAP` deferred-eviction note);
  `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:690-738`
  (DROP's `op_id`-minting + `write_op_status` call site, contrasted with
  CREATE's lack of one, confirmed via `grep -rn "CreateHashIndex\|
  CreateUniqueHashIndex\|write_op_status"` across `crates/`).
- Bench: `crates/shamir-engine/benches/f78_writer_latency.rs:1-190` (full
  file — module doc + scenario code).
- KNOWN_LIMITATIONS: `docs/guide-docs/KNOWN_LIMITATIONS.md:312-397` (§3
  "Indexes", the CREATE INDEX write-outage bullet + the F-78 unique-family
  memory sub-bullet + the "future improvement" online-build framing already
  present there).
- MVCC snapshot/versioning primitives:
  `crates/shamir-tx/src/mvcc_store/mod.rs:1097` (`get_at`), `:1307-1349`
  (`current_stream`/`current_stream_with_tombstones`/`current_stream_impl`
  doc contrasting live-current vs. version-pinned reads), `:1445`
  (`current_version`, confirmed `pub(crate)`-scoped); `crates/shamir-engine/src/table/read_temporal.rs:45-72`
  (`read_as_of`'s `At::Version`/`At::Timestamp` resolution, the existing
  version-pinned-read precedent this RFC's Phase A snapshot reuses in
  spirit); `crates/shamir-engine/src/table/table_manager_streaming.rs:91-116`
  (`list_stream`, confirmed to wrap `current_stream`, i.e. NOT version-pinned
  today); `crates/shamir-tx/src/mvcc_store/mod.rs:266` (`current_committed_version`,
  the `pub` accessor used for the pinned version); `:1347-1372`
  (`current_stream_impl`, the already-version-pinned stream primitive this RFC
  parameterizes); `crates/shamir-tx/src/repo_tx_gate.rs:356` (`open_snapshot`,
  returns a `SnapshotGuard` RAII guard for GC protection during Phase A,
  exported at `crates/shamir-tx/src/lib.rs:74`).
- Write-path audit: `docs/dev-artifacts/research/2026-08-09-p1054-write-path-audit.md`
  (full document — exhaustive enumeration of all write paths that mutate postings,
  proving the single-choke-point capture design in §2.3).
- IndexManager planning methods: `crates/shamir-index/src/base_index/index_manager.rs:2023`
  (`plan_record_created`), `:2074` (`plan_record_updated`), `:2130` (`plan_record_deleted`);
  `crates/shamir-index/src/base_index/sorted_index_manager.rs:1564`
  (`plan_record_created`), `:1644` (`plan_record_updated`), `:1713`
  (`plan_record_deleted`).
