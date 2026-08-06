# P0-3a (#1011) — reader-epoch / drain-gate implementation plan

Status: design/plan only, produced by an `@oh`-effort consultative pass over
a prior read-only factual investigation. No files modified as part of this
plan. Line numbers are from the working tree at commit `37cc59a3` (verify
against current source before implementing — several commits have landed
since, including F-3/#1030 and #1032, neither of which touches the files
this plan targets).

## Context

`crates/shamir-index/src/base_index/index_manager.rs`'s `drop_index` (around
line 1663) has a documented KNOWN GAP ("sub-bug 3a" in its own doc comment,
around line 1643): DROP retires the definition from the planner-visible RCU
Vec, then immediately sweeps postings from `info_store` with no
synchronization against a reader who resolved the definition just before
the retire. Such a reader can observe a partially-swept keyspace — an
incomplete but never corrupted result. This document is the decision +
implementation plan the review's own §4 explicitly required before closing
this out. The user chose to implement the fix now rather than merely
document the limitation.

## Decision

Close the gap for **regular** and **sorted** families (real, unprotected
production reader paths). The **unique** family needs no new primitive —
its only production reads are already serialized against DROP UNIQUE by
`unique_write_lock`/`drain_writers` (see plan body below for the exact call
chain). **index2** needs a structurally different fix (a leased `Arc`
handle, not a chokepoint guard) and should land as its own slice.

## Primitive

`ReaderDrainGate` (new, `crates/shamir-index/src/reader_drain_gate.rs`):
flag + counter + reader back-off, NOT epoch-parity RCU (parity buys nothing
here — there is only one copy of the postings, not two generations to
serve reads from). Mirrors the proven memory-model shape of this
codebase's own `WriterDrainBarrier`
(`crates/shamir-engine/src/table/writer_drain_barrier.rs`) with reader and
writer roles swapped. Full design, call-site plan, cross-family rollout,
staging recommendation, test plan and risk analysis: see the complete
`@oh` output captured in this session's transcript (agent id
`a7df3d155d524cffc`) — reproduced in full below for durability.

Slicing:
- **Slice 1** — primitive + regular family (`IndexManager::lookup_by_index`,
  `drop_index`) + unique-family doc-only tripwire. This is the template.
- **Slice 2** — sorted family (`SortedIndexManager`, 8 chokepoints).
- **Slice 3** — index2 (`IndexRegistry`) lease-based variant.

Each slice: write its own brief under
`docs/dev-artifacts/prompts/ddl-lifecycle/`, commit it, implement (TDD:
failing admission/back-off proof test first), gate-verify
(`fmt`/`clippy --workspace --all-targets -- -D warnings`/`./scripts/test.sh`
scoped to touched crates), then commit. Do NOT delete or weaken the
`drop_index` doc's "KNOWN GAP" section until slice 3 lands — narrow it
per-slice instead ("closed for regular and sorted; still open for index2").

---

## Full plan (verbatim from the `@oh` design pass)

# P0-3a — DROP INDEX vs. in-flight readers: implementation plan

**Status of this document:** design/plan only. No files were modified. Line numbers are from the working tree at `37cc59a3`.

## 0. Executive position (the "decision" half of #1011)

Close the gap for regular hash and sorted (genuine unprotected production
reader paths). Document unique as already-covered with proof.

Key findings (verified by reading, not assumed):
- **C-1**: `IndexManager::lookup_by_unique_index` has zero production
  callers — test-only today.
- **C-2**: `check_unique_key` is reached in production only via
  `validate_unique_for_create`/`validate_unique_for_update`, both already
  serialized against DROP UNIQUE by `unique_write_lock` taken BEFORE the
  validate call.
- **C-3**: a fourth chokepoint the original brief missed — commit Phase 2.6
  in `pre_commit.rs:590-604` reads `info_store().get()` directly, covered
  by the Phase 2.5 `unique_write_lock`.
- **C-4**: the sorted family has 8 unprotected chokepoints, not one
  (`lookup_range`, `lookup_range_with_values`, `lookup_min`, `lookup_max`,
  `lookup_last_k`, `lookup_range_first_k_page`, `lookup_first_k`,
  `entry_count`); the 5 `*_tx` variants delegate to these.
- **C-5**: `sweep_index_postings` does `remove_many` then
  `posting_cache.retain` — a racing reader can populate the cache with a
  partial result AFTER the retain, and that poisoned entry can outlive the
  DROP and be served again if the index name is re-created (same
  `name_interned` → same physical prefix). The gate fixes this for free.

## 1. Primitive design

Rejected: two-generation/epoch-parity reader-count pair (no second version
of the data exists to serve new readers from — parity buys nothing, adds
SRCU-class complexity). Rejected: reusing `shamir_numa::NodeReplicated`'s
`Guard` (wrong object — it pins the definition list, not the postings, and
current code never threads it to the read chokepoint anyway).

Adopted: **flag + counter + reader back-off**, mirroring
`WriterDrainBarrier`'s already-proven memory-model proof
(`table_manager_crud.rs:155-170`): bump the counter (SeqCst) before reading
the flag (SeqCst) so the cross-atomic happens-before edge is carried by the
single SeqCst total order.

- Reader: `in_flight.fetch_add(1, SeqCst)` → `drops_active.load(SeqCst)`.
  `0` → proceed; `>0` → decrement and back off (`None`, caller re-plans).
- Drainer: `drops_active.fetch_add(1, SeqCst)` → wait for
  `in_flight.load(SeqCst) == 0`.

Livelock-free by construction: once the flag is up, no new reader can join
`in_flight`, so it monotonically drains to zero — no epoch-parity dance
needed.

**Guard acquired INSIDE the chokepoint** (`lookup_by_index` itself), NOT at
the earlier `iter_indexes_ready()` resolve step — the planner never
threads a definition/token from resolve to read today (only a bare `u64`
name crosses that boundary), and acquiring at resolve time would hold the
guard across `unique_write_lock` acquisitions in engine code, creating a
constructible ABBA deadlock against DROP's own
`ddl_admission`+`unique_write_lock`+drain-wait sequence. Acquired inside
the chokepoint, the guard is always the innermost lock — no inversion is
constructible (verified against `table_manager_crud.rs:163-180` and
`pre_commit.rs:500-604`'s actual lock nesting).

Granularity: per-family, manager-wide (not per-index) in v1 — a per-index
counter needs an `scc::HashMap` probe on every hot-path read to buy back a
DDL-rare-window optimization; not worth it until measured. During a DROP of
index A, reads of index B on the same table fall back to full scan for the
drain+sweep window — bounded, consistent with the write-barrier bit already
being up for that table.

```
pub struct ReaderDrainGate {
    in_flight: AtomicUsize,
    drops_active: AtomicUsize,
    dropping_ids: scc::HashSet<u64, THasher>,
    drained: tokio::sync::Notify,
    drain_blocked: tokio::sync::Notify,  // test rendezvous
    drain_waits: AtomicU64,              // telemetry + test oracle
}
impl ReaderDrainGate {
    pub fn enter(&self, index_id: u64) -> Option<ReadGuard<'_>>;
    pub fn begin_drop(&self, index_id: u64) -> DropDrainGuard<'_>;
    pub fn drain_waits(&self) -> u64;
}
```

Bounded wait (never unbounded): `wait_for_readers(budget)` logs
`log::error!` and returns `TimedOut` on expiry, letting DROP proceed with
the sweep anyway — no worse than the pre-fix status quo. Budget: new
`ddl_defaults::DROP_INDEX_READER_DRAIN_BUDGET` in `shamir-tunables`
(suggest 5s). This is mandatory — the drain wait sits inside the
already-held `ddl_admission`+`unique_write_lock` critical section; an
unbounded wait would wedge every DDL op and every barriered writer on that
table.

## 2. Call-site plan (regular family)

`IndexManager::lookup_by_index` signature becomes
`DbResult<Option<Arc<[RecordId]>>>` (crate is `publish = false`, internal
API break only). Guard acquired as the FIRST statement, before
`build_index_key`; posting-cache probe must be INSIDE the guard (else C-5
reopens). New test seam `lookup_pause_hook` (production-compiled, not
`#[cfg(test)]` — cross-crate test consumer, same rationale as the existing
`drop_index_pause_hook`).

`entry_count` — do NOT gate; fix at the caller instead. `doctor::verify()`
doesn't hold `ddl_admission` (unlike `doctor::repair()`, F-3/#1030) — wrap
its index-count block in the same `begin_write_barrier` acquisition
`repair()` uses. Covers `SortedIndexManager::entry_count` too.

`drop_index`'s 6 steps become 8: tombstone → **begin_drop (2.5)** → RCU
retire → existing pause hook → **wait_for_readers (3.5)** → sweep →
**drop(drain) (4.5)** → persist → clear tombstone. Fits inside the
already-held `ddl_admission` critical section (`TableManager::drop_index`
already wraps the call in `begin_write_barrier` — no new lock ordering).

7 production engine call sites need to handle the new `None` (re-plan/full
scan fallback), NOT return an empty result set — enumerated in the full
transcript (fk_actions.rs, fk_on_update.rs ×2, fk_restrict.rs,
read_index_scan.rs, read_exec.rs ×2); one site
(table_manager_index_mgmt.rs → repo_instance.rs → db_instance.rs, a public
introspection-by-name API) should surface `DbError::NotFound` instead of a
silent empty set.

## 3. Cross-family rollout

One shared `ReaderDrainGate` type, instantiated per manager:
- Regular `IndexManager` — own gate field, wired in slice 1.
- Unique (same `IndexManager` struct) — doc-only, no gate (§C-1/C-2/C-3).
- Sorted `SortedIndexManager` — own gate field, wired in slice 2 (8
  chokepoints + `drop_index`'s same 2.5/3.5/4.5 insertions, including its
  extra `!existed` rollback branch).
- index2 `IndexRegistry` — structurally different: lease the guard WITH the
  `Arc<dyn IndexBackend>` handle at the read-DISPATCH accessors only
  (`find_by_field_and_kind`/`get_by_name`), never at the tx-stage-time
  accessors (`all_backends()`/`backends_newer_than()`, which are held for a
  whole transaction's duration — leasing those would let one long tx stall
  every DROP INDEX on the table). Slice 3, own review.

## 4. Test plan

New `crates/shamir-engine/src/table/tests/p1011_reader_drain_tests.rs`
(mirrors `f76_drop_visibility_tests.rs`'s pause-hook + spawn + deterministic
rendezvous pattern — no `sleep`-based timing assumptions beyond the
established "block until released" shape).

1. **Proof test**: parked read holds the guard → spawned DROP blocks in
   `wait_for_readers` → direct `info_store` scan proves the sweep has NOT
   started while parked → release read → assert it returned the COMPLETE
   pre-drop set → DROP completes → sweep verified to have run only after.
2. **Back-off test**: a read arriving during the drop-in-progress window
   returns `Ok(None)`, never a silently-empty `Ok(Some([]))`.
3. **Negative/perf-sanity**: assert `drain_waits() == 0` for an uncontended
   DROP (no timing assertion), paired with a `drain_waits() == 1` assertion
   from the racing test — the pairing is mandatory, a lone `==0` check
   passes vacuously if the counter is never wired (same defect class #1005
   fixed for the variant-coverage check earlier this session).
4. **Guard-release-on-error test**: force `lookup_by_index` to error
   mid-scan, confirm the guard still released (RAII/`Drop`, not a manual
   `leave()`).
5. Regression sweep: re-run `f76`, `f72`, `p03`, `f95`-tagged tests
   unchanged.

## 5. Risk callouts

- **Deadlock**: excluded by construction ONLY because the guard is the
  innermost lock (§1's placement argument) — write this as a hard invariant
  in the gate's own doc comment so nobody "helpfully" moves the guard to
  resolve time later.
- **Livelock**: excluded by the back-off (not parity) design — once the
  flag is up, no reader joins `in_flight`.
- **Asymmetric acquire/release**: RAII guards only, no public `leave()`.
  Covered by the guard-release-on-error test.
- **Collateral full-scan during DROP**: accepted, bounded, documented in
  `drop_index`'s doc so an operator reading a latency spike finds the
  explanation. Do not pre-optimize to per-index counters without measurement.
- **Making it WORSE than today**: the three concrete ways (returning empty
  instead of `None`, deleting the known-gap doc early, an unbounded drain
  wait) are each guarded above — treat all three as release-blocking review
  gates on every slice.
