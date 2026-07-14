# Execution Plan: Oracle review remediation — WAL group-fsync + cleanup

> **Superseded in part** by `docs/dev-artifacts/perf/durability-model.md` (Реализация B — file WAL): the WAL-layer group-fsync over the KV store + watch\<u64\> generation design here was replaced by a file-backed WalSegment + per-entry-waiter WalGroupCommit.

Follow-up to the Version Oracle (`version-oracle-execution-plan.md`,
fully landed) addressing the post-implementation review. The review's
six P2c/P3a findings + eight P3b findings collapse onto **one root
cause: batching lived at the wrong layer.** P2c killed the serializing
leader for lock-freedom and threw out WAL fsync-batching with it.

The keystone fix lowers batching to where it belongs — the WAL sink —
as a **lock-free group-fsync**. One move heals the regression (#1),
makes the dead orchestration deletable (#3/#4/#5/#6), and evaporates
P3a's intra-batch accumulator. The rest are independent test-quality
and hygiene fixes.

Discipline (unchanged): sub-agents never commit/push (return diff,
parent reviews). Tests ONLY via `./scripts/test.sh` (NEVER raw
`cargo test`). Gate = fmt + clippy --workspace --all-targets + the
relevant `./scripts/test.sh` scope. Structural phases cut small,
Stage-A-style. Data integrity > performance — STOP on uncertainty.

Agent-type legend: `research` (read-only/bench, no behavior change),
`structural` (high risk, ao46l), `mechanical` (asl).

---

## Dependency graph

```
G0 (durable-backend concurrent-commit bench) ─┐ research, baseline
                                              ▼
G1 (WAL group-fsync primitive) ───────────────┤ structural KEYSTONE
   G1a scaffold → G1b wire RepoWalManager → G1c switch commit_tx_lockfree
                                              │ heals #1
                                              ▼
G2 (delete dead orchestration) ───────────────┤ structural
   run_leader / group_commit.rs / PendingCommit / conflicts_with / P3a
   accumulator; resolve AsyncIndex fate        │ resolves #3/#4/#5/#6

G3 (crash-injection completeness) ── independent, structural ── P3b#3
G4 (stress-test quality) ─────────── independent, mechanical ── P3b#1/#2/#7/#8
G5 (record-ordering rationale doc) ─ fold into G1c ─────────── review#2
G6 (nextest version pin) ─────────── independent, mechanical ── P3b#5
```

`G3`/`G4`/`G6` are independent of the G1→G2 spine and may run in
parallel. `G4`'s gap-semantics decision (#7) should be settled before
or alongside `G3` (both touch recovery/watermark invariants).

---

## G0 — Durable-backend concurrent-commit bench (research, baseline)

- **Agent:** research · **Prereq:** none · **[parallel-ok]**
- **Why:** #1 is invisible on the in-memory backend (fsync is free —
  that's why the Oracle benches showed only wins). To prove WAL
  group-fsync helps we need a per-fsync backend baseline. Measure-first,
  like P0.
- **Files:** `crates/shamir-engine/benches/` (new bench or extend
  tx_pipeline), a durable backend factory (redb with
  `Durability::Immediate`, or persy/nebari — whichever exposes
  fsync-per-commit cleanly; check `crates/shamir-storage`).
- **Task:** a bench `durable_concurrent_commit` that, on a durable
  backend, runs N concurrent committers (N ∈ {1, 8, 32}) against
  (a) the SAME table and (b) DISJOINT tables; report wall-clock and, if
  the backend exposes it, fsync count per batch. Capture the current
  per-tx-`wal.begin` numbers as the baseline group-fsync will be
  measured against.
- **Constraints:** bench-only; isolated bench target dir per CLAUDE.md;
  BENCH default is quick — fine for relative numbers.
- **Done:** baseline table {same,disjoint} × {n_1,n_8,n_32} → µs +
  fsync-count. Verdict: is the same-table regression real and
  measurable on this backend? (If a durable backend can't be benched
  cheaply, document that and proceed on the theoretical regression —
  the fsync arithmetic is not in doubt.)
- **Report:** baseline numbers + go/no-go for G1. <300 words.

---

## G1 — WAL group-fsync primitive (structural, KEYSTONE)

Lock-free at append, batched at fsync. Classical group commit, done at
the WAL layer instead of the commit orchestration. **Level-triggered
signalling** (lesson from the CancellationToken fix): use
`tokio::sync::watch<u64>` for the flushed-generation counter — a waiter
reads `if flushed >= my_gen { done } else { watch.changed().await }`,
race-free across the subscribe-window because watch holds the latest
value.

### Mechanism (target shape)
```
GroupFsync (per RepoWalManager):
  pending:   lock-free queue of (encoded_entry_bytes, gen)   // SegQueue / scc
  cur_gen:   AtomicU64                                        // generation a new append joins
  flushed:   watch::Sender<u64>                              // last fully-flushed generation
  flushing:  AtomicBool                                      // leader election

append_and_await(entry):
  gen = cur_gen.load()
  pending.push((encode(entry), gen))
  if flushing.compare_exchange(false, true).is_ok():
      // I am the fsync leader for this window.
      batch = drain(pending)                 // take everyone waiting
      store.set_many(batch) ; store.flush()  // ONE fsync for all
      flushed.send(max_gen_in_batch)
      cur_gen.fetch_add(1)
      flushing.store(false)
      // if more arrived while flushing, re-elect (loop or re-CAS)
  else:
      // follower: wait until my generation is durable
      let mut rx = flushed.subscribe()
      while *rx.borrow() < gen { rx.changed().await }
```

### G1a — scaffold (additive, `#[allow(dead_code)]`)
- **Agent:** structural · **Prereq:** G0 (go)
- **Files:** new `crates/shamir-tx/src/group_fsync.rs`;
  `crates/shamir-tx/src/lib.rs` (mod).
- **Task:** the `GroupFsync` type + `append_and_await` per the shape
  above, lock-free (SegQueue/scc + atomics + watch — NO std::Mutex on
  the hot path; ideology pillar 1). Unit tests: single append flushes;
  N concurrent appends → exactly one (or few) flush generations, all
  appends observe durable; follower wakes when leader bumps generation;
  re-election when an append lands mid-flush.
- **Constraints:** additive; not wired to commit. Gate ONCE at end.
- **Done:** type + tests green; commit path untouched.

### G1b — wire into RepoWalManager
- **Agent:** structural · **Prereq:** G1a
- **Files:** `crates/shamir-tx/src/repo_wal_manager.rs`.
- **Task:** `RepoWalManager::begin_grouped(entry) -> DbResult<()>` that
  routes through `GroupFsync` (append + await durable). Keep `begin`
  and `begin_many` for now (callers migrate in G1c / G2). Verify the
  durable bytes written are byte-identical to today's `begin` path
  (same WAL key namespace, same WalEntryV2 encoding).
- **Done:** begin_grouped exists; round-trip test proves identical
  on-disk bytes; gate green.

### G1c — switch commit_tx_lockfree to begin_grouped
- **Agent:** structural · **Prereq:** G1b
- **Files:** `crates/shamir-engine/src/tx/commit.rs` (commit_tx_lockfree
  ~line 452). Also fold in **review #2 rationale comment** (G5): document
  that `record_commit_writes` runs BEFORE materialize/publish on
  purpose — `predicate_conflicts` gates the SSI window on
  `commit_version <= last_committed`, so an early footprint is invisible
  to SSI until publish advances `last_committed`; recording before
  publish is REQUIRED so no concurrent tx misses a just-published
  footprint (missed conflict = serializability hole, strictly worse than
  a false abort). Do NOT reorder — the comment prevents a future "fix".
- **Task:** replace `wal.begin(validated.wal_entry)` with
  `wal.begin_grouped(...)`. Concurrent committers now share one fsync,
  lock-free. Re-run G0's durable bench → confirm same-table fsync count
  drops to ~1/window and the #1 regression is healed.
- **Constraints:** crash-safety preserved — `begin_grouped` returns only
  after the entry is durable (the commit point). `maybe_crash("phase4")`
  semantics unchanged. Tests: `./scripts/test.sh @oracle` + `@e2e`.
- **Done:** lockfree path batches fsync; G0 bench shows the win; all
  oracle + e2e green; review #2 documented.
- **Report (G1 overall):** mechanism, durable-bench before/after,
  tests. <600 words.

---

## G2 — Delete the dead orchestration (structural)

- **Agent:** structural · **Prereq:** G1c
- **Why:** with batching at the WAL, `run_leader` / `group_commit.rs` /
  `PendingCommit` / `conflicts_with` / the P3a `batch_footprints`
  accumulator are no longer the batching mechanism — and the standard
  path already bypasses them. Resolves #3, #4, #5, #6 by subtraction.
- **Files:** `crates/shamir-engine/src/tx/group_commit.rs` (delete or
  gut), `commit.rs` (the AsyncIndex dispatch + `commit_tx_inner_legacy_async`),
  `crates/shamir-tx/src/{repo_tx_gate.rs (pending queue),
  tx_context.rs (write_set_keys/conflicts_with), pending_commit.rs}`.
- **Task:**
  1. **Resolve AsyncIndex fate first** (read commit.rs dispatch +
     `commit_tx_inner_legacy_async`). Decide: can AsyncIndex commits use
     `commit_tx_lockfree` + `begin_grouped` like the standard path? If
     YES → delete the legacy path AND `group_commit.rs` entirely. If NO
     (AsyncIndex has a genuine out-of-band ordering need) → document it
     precisely, and reduce `group_commit.rs` to the minimum AsyncIndex
     needs (it can still adopt `begin_grouped` for its fsync). STOP and
     report if AsyncIndex's needs are unclear — don't guess.
  2. Delete now-unused: `PendingCommit`, `enqueue_pending`/`drain_pending`,
     `compute_write_set_keys`, `conflicts_with` + `write_set_keys`
     (Stage C `2876ec7`), the P3a accumulator + `record_conflicts`
     export (keep `record_conflicts` only if AsyncIndex path retains the
     accumulator).
  3. Remove `begin_many` if no caller remains (G1's group-fsync
     supersedes it), or keep if AsyncIndex still batches via it.
- **Constraints:** pure subtraction must not change observable behavior
  of the standard path. Every deletion verified by `cargo check
  --workspace` + `./scripts/test.sh @oracle @e2e`. If a deletion
  cascades into >8 files of churn, STOP and report scope.
- **Done:** dead orchestration removed (LOC count reported); AsyncIndex
  fate documented; oracle + e2e green.
- **Report:** what was deleted (LOC), AsyncIndex decision, tests. <500w.

---

## G3 — Crash-injection completeness (structural) — P3b#3

- **Agent:** structural · **Prereq:** G1c (path stable) · **[parallel-ok with G4/G6]**
- **Why:** the dangerous crash points — (c) mid-materialize, (d)
  after-materialize/before-mark — are untested. The seam already exists
  (`maybe_crash("phase4")` at commit.rs:461).
- **Files:** `crates/shamir-engine/src/tx/materialize.rs` (add
  `maybe_crash("phase5-mid")` between data and index writes, and
  `maybe_crash("phase6")` after materialize before the
  mark(Materialized)/publish), `crates/shamir-engine/src/tx/tests/oracle_stress_tests.rs`.
- **Task:** add the two seams; add recovery tests that inject a crash at
  (c) and (d), then run recovery and assert convergence:
  - partial-materialize (c): the version is either fully replayed from
    WAL or fully absent — never half; watermark + data consistent.
  - after-materialize/before-mark (d): recovery re-marks the version
    Materialized from the durable WAL entry; watermark reaches it; no
    orphaned version.
- **Constraints:** the `maybe_crash` seams must be zero-cost when not
  armed (cfg/test-only or an atomic check that's a no-op in prod —
  match the existing `maybe_crash("phase4")` implementation exactly).
- **Done:** (c) and (d) covered with passing recovery-convergence tests;
  `./scripts/test.sh @oracle` green; non-flaky over 20 runs.
- **Report:** seams added, scenarios, flakiness check. <400 words.

---

## G4 — Stress-test quality (mechanical) — P3b#1/#2/#7/#8

- **Agent:** mechanical · **Prereq:** none · **[parallel-ok]**
- **Files:** `crates/shamir-engine/src/tx/tests/oracle_stress_tests.rs`,
  `.config/nextest.toml` (if raising iteration counts for a flake-hunt
  profile).
- **Task:**
  1. **#1** `oracle_stress_same_table_serializable_conflict_resolution`:
     replace `AlwaysConflictProvider` (constant 999_999) with a provider
     returning the REAL committed version of an earlier tx, so the test
     exercises an actual A-writes/B-reads SSI conflict — not just the
     abort path. Assert the INVARIANT (no torn state; at least one
     commits; conflicting ones abort), not `conflicts == 20`.
  2. **#2** `oracle_stress_abort_does_not_stall_watermark`: add a
     `tokio::sync::Barrier` so all txs start together; assert the
     invariant `aborts + successes == TOTAL` + watermark advances past
     all consumed versions — NOT exact `aborts == 20`. First VERIFY
     whether the count is actually scheduling-dependent (if each tx's
     conflict decision is a pure function of its own seed and the txs
     write disjoint keys, the count IS deterministic — then document
     that and keep a count assert; if it depends on inter-tx visibility,
     switch to the invariant).
  3. **#7** `oracle_stress_recovery_gap_in_versions`: **decide and pin
     the gap semantics.** A version assigned but never made durable
     (the gap) — does recovery mark it Aborted so the watermark advances
     past it (→ wm reaches 5), or does it stall the contiguous prefix
     (→ wm == 2, and 4/5 stay pending forever)? Settle this with the
     recovery design, then `assert_eq!` the correct value with a comment
     explaining why. A permanently-stalled watermark (4/5 stuck) would
     be a liveness bug — if that's the current behavior, it's a real
     finding: report it.
  4. **#8** add a `flake-hunt` nextest profile or a documented
     `for i in $(seq 1 200)` loop for the concurrency tests; record the
     methodology in a comment, not just "10/10".
- **Constraints:** tests stay deterministic; assert invariants over
  exact counts wherever scheduling can vary. Gate ONCE.
- **Done:** all four addressed; gap semantics pinned; `@oracle` green
  over the flake-hunt count.
- **Report:** per-finding what changed; gap-semantics decision; flake
  methodology. <500 words.

---

## G6 — nextest version pin (mechanical) — P3b#5

- **Agent:** mechanical · **Prereq:** none · **[parallel-ok]**
- **Files:** a tooling note (e.g. `.config/nextest.toml` header comment
  + a line in CLAUDE.md §🧪 or a `rust-toolchain`/CI manifest).
- **Task:** document that the cargo-runner guard depends on `cargo
  nextest` setting `$NEXTEST` in the test-process environment, and pin
  the expected nextest version (the guard fails CLOSED if the var is
  renamed — annoying, not dangerous, but pin + comment makes the
  dependency explicit). If CI installs nextest, pin the version there.
- **Done:** dependency documented + version pinned; no behavior change.
- **Report:** <150 words.

---

## Sequencing summary

| Phase | Type | Risk | Prereq | Heals |
|---|---|---|---|---|
| G0 | research | low | — | baseline for #1 |
| G1a | structural | med | G0 | — |
| G1b | structural | med | G1a | — |
| G1c | structural | high | G1b | #1, #2(doc) |
| G2 | structural | med | G1c | #3,#4,#5,#6 |
| G3 | structural | med | G1c | P3b#3 |
| G4 | mechanical | low | — | P3b#1/#2/#7/#8 |
| G6 | mechanical | low | — | P3b#5 |

Spine: `G0 → G1a → G1b → G1c → G2`. Parallel anytime: `G3` (after
G1c), `G4`, `G6`.

## The beauty restated

The review's cluster is one symptom — batching at the wrong layer.
Lower it to the WAL as a lock-free group-fsync (level-triggered via
`watch`, echoing the CancellationToken lesson), and the regression
heals while ~400 lines of orchestration become deletable and P3a's
accumulator evaporates. The remaining items are independent test-honesty
fixes. Subtraction over addition.
