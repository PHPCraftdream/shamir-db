# Brief for F-46 (#857, P0) — RI barrier mutual commit-serialization for concurrent FK-child writers

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

The 2026-07-28 readonly review
(`docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`, §3
P0-1) found a genuine release-blocking gap in F-40b's RI barrier
mechanism (the explicit-Snapshot FK TOCTOU fix landed in commits
`bef9701e`/`51375746` earlier this session). **Read that review's P0-1
section in full first**, then read
`docs/dev-artifacts/research/f40b-ri-barrier-spike.md` (the original
design memo) for the barrier's intended shape.

**The gap, verified by direct code reading (not just trusting the
review):**

`commit_tx_lockfree` (`crates/shamir-engine/src/tx/commit.rs:748-755`)
takes `gate.commit_lock()` when:

```rust
let _serializable_guard = if tx.isolation == IsolationLevel::Serializable
    || !tx.cas_set.is_empty()
    || !tx.ri_barrier_tokens_is_empty()
{
    Some(gate.commit_lock().await)
} else {
    None
};
```

`footprint_tokens` (`crates/shamir-tx/src/tx_context.rs:226`, populated via
`require_footprint_for` at `:584-587`, called from
`require_footprint_if_fk_child` in `query_runner.rs:302-363` at every
insert/update/set into an FK-child table — see the 5 call sites at
`query_runner.rs:1214`, `:1267`, `:1352`, `:1433`, `:1688`) is **absent**
from this condition. A plain Snapshot writer into an FK-child table only
gets `footprint_tokens` populated — it never takes `commit_lock`.

**The race this allows** (verified against the barrier's own commit
sequence in `commit.rs:699-810`, specifically `pre_commit_locked_validate`
at line ~758 running Phase 2-bis BEFORE `record_commit_writes` at line
~810):

1. Parent Snapshot tx scans the FK-child table (records an RI barrier
   token via `fk_restrict.rs`/`fk_actions.rs`/`fk_on_update.rs`'s recording
   sites).
2. Parent enters commit, takes `commit_lock` (because
   `ri_barrier_tokens` is non-empty).
3. Parent runs `predicate_conflicts_batch` inside `pre_commit_locked_validate`
   — no conflicting footprint exists yet.
4. A concurrent Snapshot writer into the SAME child table has only
   `footprint_tokens` set (not `ri_barrier_tokens`), so it does NOT take
   `commit_lock`. It proceeds through its own commit: WAL write,
   `record_commit_writes` (publishing its footprint), and materialize —
   all without contending with the parent's lock.
5. If step 4 lands strictly between the parent's step 3 check and the
   parent's own `record_commit_writes`/publish, the parent's RESTRICT/
   CASCADE/SET NULL/ON UPDATE decision was made against a child-table
   state that a concurrent writer subsequently changed — and neither side
   detects it, because the child writer's commit_lock-free path never
   re-checked against the parent's activity either.

The existing tests (`fk_ri_barrier_tests.rs:136-150`, `:253-286`,
`fk_race_closure_tests.rs:193-213`) inject the concurrent writer BEFORE
parent commit begins (at the after-scan/before-parent-commit seam via the
`resolve_repo`-ordinal injection), so they only prove a **backward-looking
recheck** (writer-already-fully-committed-before-parent-validates) — never
a **concurrent publish during** the parent's own commit-lock window. This
brief's job is to close that second case.

## What to do

### 1. Adversarial red test FIRST (prove the gap on the CURRENT code)

Before touching any production code, write a deterministic race test that
fails on the current implementation. It needs a pause seam that fires the
concurrent child writer **AFTER** the parent's `predicate_conflicts_batch`
check has run and **BEFORE** the parent's own publish
(`record_commit_writes`/materialize) — not at the `resolve_repo` ordinal
seam the existing tests use (that seam fires too early for this
scenario).

Investigate the commit pipeline for an existing injection seam that can
fire mid-commit (after Phase 2-bis, before publish) — check whether
`RepoTxGate`/`commit_lock`/`pre_commit_locked_validate`'s call site in
`commit.rs` already has a test-only hook, or whether you need to add a
narrow one (e.g. a `#[cfg(test)]` callback invoked at a specific point in
`commit_tx_lockfree`, mirroring how other race tests in this codebase
inject at precise points — read `fk_reverse_cache_race_tests.rs` and
`storage_mirrored_tests.rs`'s fault-injection patterns for the established
style of "narrow, cfg-gated, documented" test seams in this repo).

The test should prove: with an explicit-Snapshot (or implicit) parent
DELETE/UPDATE against an FK-child table with a RESTRICT/CASCADE/SET
NULL/ON UPDATE action, and a concurrent Snapshot writer inserting a new
child row timed to land inside the parent's commit-lock window (after the
barrier's own predicate check, before the parent's publish) — the CURRENT
code allows both to commit, silently producing an inconsistent FK
decision (e.g. RESTRICT allows the delete despite the concurrent insert,
or CASCADE/SET NULL misses the concurrently-inserted row). Confirm this
test FAILS on the unmodified code before proceeding — this is the "red"
in red-green.

### 2. Minimal fix

Widen the `commit_tx_lockfree` commit-lock condition
(`commit.rs:748-755`) to also take the lock when `!tx.footprint_tokens.is_empty()`:

```rust
let _serializable_guard = if tx.isolation == IsolationLevel::Serializable
    || !tx.cas_set.is_empty()
    || !tx.ri_barrier_tokens_is_empty()
    || !tx.footprint_tokens.is_empty()
{
    Some(gate.commit_lock().await)
} else {
    None
};
```

This makes every FK-child writer (which always gets `footprint_tokens`
populated per `require_footprint_if_fk_child`'s unconditional call sites)
participate in the SAME validate→publish serialization the RI barrier
side already uses — closing the gap symmetrically. Check whether the
SAME widening is needed at the two other commit-pipeline sites F-40b
already touched (`pre_commit.rs`'s `pre_commit_locked` legacy path, and
`commit_tx_inner_legacy_async`'s own lock acquisition if it has an
analogous conditional) — read those call sites and decide; if
`commit_tx_inner_legacy_async` already unconditionally takes the lock
(check this — F-40b's memo noted this path "already always runs under
commit_lock"), it needs no change.

Make the red test from step 1 pass. Then verify it: all 4 FK actions
(RESTRICT/CASCADE/SET NULL/ON UPDATE), both commit orders (parent-first
and child-first), implicit AND explicit Snapshot paths, and at least a
sanity check that AsyncIndex visibility and crash-recovery paths are
unaffected (they likely already always serialize — confirm, don't
assume).

### 3. Measure concurrency impact — MANDATORY, do not skip

This fix means EVERY write into ANY FK-child table now takes the
process-wide `commit_lock` — a potentially significant concurrency
regression on write-heavy child tables (e.g. a table referenced by many
FKs, or under high write concurrency). Before declaring this done:

- Run (or write, if none exists) a benchmark that measures concurrent
  write throughput into an FK-child table before vs after this change —
  use `bench_scale_tool::Harness` per this repo's mandatory bench
  convention (see `crates/shamir-engine/benches/tx_pipeline.rs` for the
  template; do NOT reach for Criterion APIs).
- Report the actual numbers (throughput/latency before vs after) in your
  final summary — do not just assert "some regression is expected".
- If the regression is severe (your judgment call, but flag anything that
  looks like more than a modest single-digit-percent throughput hit under
  concurrent FK-child writes), say so explicitly and note that P1-1's
  follow-up (narrowing the barrier's dependency from full-`TableScan` to a
  tighter key/range predicate, and/or a per-repo/per-relation RI epoch
  instead of the global `commit_lock`) becomes higher priority — this is
  already tracked, do not attempt that narrowing in THIS task, just report
  the numbers that justify (or don't) escalating it.

### 4. Update documentation

`docs/guide-docs/KNOWN_LIMITATIONS.md`'s FK entry currently (as of commit
`51375746`) claims the explicit-Snapshot gap is "CLOSED via the RI
barrier" — this is the exact overclaim the review's P1-4 flags (tracked
separately as task F-51, do NOT do the full truthfulness sweep here). For
THIS task, only update the specific sentence describing the forward-race
mechanism you just fixed: once your fix lands and is verified, the
"CLOSED" claim becomes accurate for this specific gap — but do not touch
unrelated parts of that entry (F-51 owns the rest of the sweep). If your
fix does NOT fully close it (e.g. you scope down to a narrower guarantee),
say so precisely instead of leaving the overclaim in place.

## Constraints

- Do not implement the P1-1 dependency-narrowing (key/range predicate
  instead of full `TableScan`) or the per-repo RI epoch alternative in
  this task — out of scope, tracked separately. Report the measured
  concurrency impact so that follow-up can be prioritized correctly.
- Do not touch `FkReverseCache` internals, `group_commit.rs`'s dead
  `run_leader` path (tracked separately as F-54), or the
  `std::sync::Mutex` question for `ri_barrier_tokens` (also noted in
  F-46's task description as a secondary concern, not this task's focus
  unless it's trivially bundled with the fix you're making).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-tx -- --check` and
  `cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings`
  must be clean.
- Benches: `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench <name>`
  if you add/run one — isolated target dir per this repo's convention.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine -p shamir-tx --full
```

When done, give your final summary as plain text: the red test's proof
(what it demonstrated failing, then passing), the exact fix applied and
why, all commit-pipeline sites checked/changed, the concurrency
benchmark's actual before/after numbers and your assessment of severity,
what (if anything) was changed in `KNOWN_LIMITATIONS.md` and why, full
test run output, and confirmation fmt/clippy are clean.
