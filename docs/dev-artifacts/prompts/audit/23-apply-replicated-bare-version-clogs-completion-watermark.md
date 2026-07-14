Task: MEDIUM concurrency — `apply_replicated` (follower-side replication
apply) allocates a raw MVCC version via the un-guarded
`assign_next_version()` and NEVER marks it in the completion tracker on
the success path (only on the failure path, via
`mark_durable_aborted`) — so a Deferred/never-`5a`-published version on
a busy follower permanently clogs the completion watermark's
contiguous-prefix advancement, until a restart re-seeds the floor
(audit finding A12, `docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-engine/src/tx/apply_replicated.rs`, `apply_replicated`
  (~line 119-294, confirm current lines):
  - Line ~138: `let local_version = gate.assign_next_version();` — the
    BARE, un-guarded allocator (`RepoTxGate::assign_next_version`,
    `crates/shamir-tx/src/repo_tx_gate.rs:439-441`). This only bumps
    the raw `version_counter` atomic — it does NOT register the
    version with the completion tracker (`gate.completion()`) in any
    state (`Pending`/`Materialized`/`Aborted`).
  - Line ~252 (failure path only): `gate.mark_durable_aborted(local_version);`
    — on FAILURE, the version IS marked (presumably `Aborted` in the
    completion tracker, advancing its watermark past the hole).
  - Line ~265 (success path): `gate.mark_durable(local_version);` —
    marks the DURABLE watermark, but per the audit, this does NOT also
    mark the version `Materialized` in the **completion tracker**
    (`gate.completion()`) the way `VersionGuard::commit()` does. Confirm
    by reading `mark_durable`'s implementation
    (`crates/shamir-tx/src/repo_tx_gate.rs`, search for `fn
    mark_durable` — NOT `mark_durable_aborted`) whether it touches
    `self.completion` at all, or only `self.durable_completion`/similar
    — these may be TWO SEPARATE trackers (a "durable" tracker and a
    "completion"/visibility tracker), and the audit's concern is
    specifically about the **completion/visibility** tracker's
    contiguous-watermark-advancement never being told about this
    version on the success path.
  - Contrast with `RepoTxGate::assign_next_version_guarded()`
    (`repo_tx_gate.rs:453-461`): returns a `VersionGuard` whose `Drop`
    marks the version `Aborted` (advancing past it) UNLESS
    `VersionGuard::commit()` was called first (which marks it
    `Materialized`) — the doc comment states this exists precisely so
    "the compiler thus enforces that every allocated version is
    terminally marked exactly once." `apply_replicated` bypasses this
    guard entirely by calling the bare `assign_next_version()` instead.

## Why this is MEDIUM

**Concrete interleaving from the audit:**
1. A follower applies a replicated event: `local_version = N` is
   allocated via the bare `assign_next_version()` — NOT registered in
   the completion tracker in any state yet.
2. `apply_committed_ops` succeeds (all table batches apply cleanly),
   so `any_failed` stays `None` — the function proceeds down the
   SUCCESS path.
3. `gate.mark_durable(local_version)` runs (line ~265) — but per the
   audit, this does NOT mark the completion tracker's
   `Materialized`/terminal state for `N`. The completion tracker's
   `try_advance`/contiguous-watermark logic (used elsewhere to compute
   `durable_watermark()`/visibility floors that OTHER machinery, e.g.
   the drainer or `advance_last_committed`, depends on) has NO entry
   for `N` at all — it's neither `Pending`, `Materialized`, nor
   `Aborted` in that tracker's bookkeeping.
4. **If this follower is ALSO the WAL-based commit path's user for
   OTHER, local (non-replicated) transactions**: some later LOCAL tx
   commits at version `M > N`. Its `VersionGuard::commit()` → `mark(M,
   Materialized)` runs correctly. But the completion tracker's
   contiguous-prefix advancement logic needs EVERY version up to and
   including its target to be terminally marked before it can advance
   past it (that's the whole point of a contiguous watermark — a gap
   at `N` blocks advancement past `N` even though `M > N` is already
   marked). **`try_advance` gets stuck at `N`'s gap forever** — `N` is
   durably written to history (mark_durable ran) and is fully
   materialized in the actual data, but the trackers's watermark
   thinks `N` is still "unknown"/unterminated, so
   `advance_last_committed`/the visibility floor NEVER moves past `N`.
5. **Concrete user-visible symptom, per the audit's distinction between
   the "5a" (data-phase-published) case and the "Deferred" case**:
   - If Phase 5a (some analogous data-publish confirmation) already
     ran for this version through some OTHER path, the visible floor
     might still limp forward via `publish_committed_max` (a different
     mechanism from the completion tracker) — a partial mitigation.
   - BUT for a tx/version that is genuinely **Deferred** (5a never ran
     — e.g. `apply_replicated`'s inline `apply_committed_ops` IS the
     ONLY publish mechanism here, there's no separate 5a phase calling
     `publish_committed_max` independently) — the version's visibility
     depends ENTIRELY on the completion tracker eventually advancing
     past it, which now never happens. **The replicated data sits
     durably in history but is invisible to readers relying on the
     completion-tracker-derived floor until a RESTART re-seeds the gate
     (which re-derives the floor from durable markers, sidestepping the
     stuck in-memory tracker).**

## Fix

Per the audit's fix sketch: **use `assign_next_version_guarded()`
instead of the bare `assign_next_version()`, and route both the
success and failure paths through the guard's `commit()`/drop
semantics** so the completion tracker is ALWAYS terminally marked
exactly once, matching the compiler-enforced pattern the rest of the
codebase already uses for local commits.

Concretely, in `apply_replicated`:
1. Replace `let local_version = gate.assign_next_version();` with
   `let version_guard = gate.assign_next_version_guarded(); let
   local_version = version_guard.version();` (check the exact accessor
   name on `VersionGuard` — may be a public field or method; use
   whatever the existing guard type exposes, consistent with how the
   main commit path already reads `version_guard.version()` /
   equivalent elsewhere in the codebase — grep `commit.rs` for existing
   usage patterns to match style).
2. On the FAILURE path (~line 240-254, where `any_failed` is `Some`):
   simply DROP `version_guard` without calling `.commit()` — per the
   guard's documented Drop behavior, this automatically marks the
   version `Aborted` and advances the watermark past it, which is
   EXACTLY what the current `gate.mark_durable_aborted(local_version)`
   call intends. Confirm whether `mark_durable_aborted` does something
   ADDITIONAL beyond what the guard's Drop already provides (e.g. also
   touches a separate "durable" tracker distinct from "completion") —
   if so, you may need to keep an explicit call to whatever the
   NON-completion-tracker part of that function does, while letting the
   guard's Drop handle the completion-tracker part. Read
   `mark_durable_aborted`'s full implementation before deciding whether
   it can be fully replaced by the guard's Drop or whether parts of it
   must be retained alongside the guard.
3. On the SUCCESS path (~line 260-265): call `version_guard.commit()`
   BEFORE or in place of (again, check exactly what `mark_durable` does
   relative to what `commit()` already provides — there may be
   overlap/redundancy, or `mark_durable` may cover a genuinely separate
   "durable" tracker that the guard doesn't touch, in which case BOTH
   calls are needed) the existing `gate.mark_durable(local_version)`
   call. Read `VersionGuard::commit()`'s implementation
   (`crates/shamir-tx/src/repo_tx_gate.rs`, search for `impl
   VersionGuard` and its `commit` method) to understand precisely what
   it marks, and compare against `mark_durable`'s effects, to determine
   whether one, both, or a merged call sequence is needed. Document
   your finding in the report.
4. Ensure the guard variable is genuinely held across every early-exit
   path in the function (the `by_table` loop's `break` on
   `any_failed`, the early `Ok(ApplyOutcome::Skipped)` return for
   already-applied versions — confirm this early return happens
   BEFORE version allocation, so it's not a concern — and the final
   success/failure branches) so its Drop obligation is honored on EVERY
   path out of the function, not just the two explicitly-handled ones
   this brief calls out. Use the compiler (a `VersionGuard` with no
   `Default`/`Clone` and a `Drop` impl will simply do the right thing
   automatically on any early return/panic-unwind, which is the whole
   point of the RAII pattern) rather than manually auditing every path
   by hand — but DO confirm via `cargo build`/`clippy` that no code
   path attempts to use `local_version` in a way that's now awkward
   with the guard in scope (e.g. needing `.version()` repeatedly is
   fine; just check it compiles cleanly).

## TDD requirement

1. **Red**: write `#[tokio::test]`s (check
   `crates/shamir-engine/src/tx/tests/` for existing
   `apply_replicated`-related test modules, e.g.
   `apply_replicated_tests.rs`, and follow established patterns) that:
   - Reproduce the stuck-watermark scenario: apply a replicated event
     successfully via `apply_replicated` (so `local_version = N` is
     allocated and the OLD code's bare `assign_next_version` leaves the
     completion tracker with no entry for `N`), then perform a LOCAL
     commit at a version `M > N` through the normal commit path, and
     assert that the completion tracker's derived watermark/visibility
     floor DOES advance to include `M` (not stuck at `N`'s gap). This
     should FAIL before the fix (watermark stuck below `N`, unable to
     advance past the untracked version) and PASS after (guard properly
     marks `N` `Materialized`, watermark advances cleanly through both
     `N` and `M`).
   - A second test for the FAILURE path: force `apply_committed_ops` to
     fail for a replicated event, and assert the completion tracker
     correctly shows `N` as `Aborted` (or equivalent terminal state)
     and the watermark advances PAST it — proving the guard's Drop
     path works identically to (or better than) the old
     `mark_durable_aborted` call.
   - A regression test confirming the existing "downstream changefeed
     re-emission", "idempotent re-delivery skip", and "persist_markers
     best-effort" behaviors are unchanged.
2. **Green**: apply the fix.
3. Confirm existing `apply_replicated`/replication tests still pass.

## Test scope command

```
./scripts/test.sh -p shamir-engine -- apply_replicated
./scripts/test.sh -p shamir-engine -- replicat
./scripts/test.sh -p shamir-tx
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- What `mark_durable`/`mark_durable_aborted` actually do relative to
  `VersionGuard::commit()`/Drop, and whether the fix ended up
  replacing them entirely, keeping them alongside the guard, or a
  merged sequence — with justification.
- Confirmation the guard is held across every exit path in
  `apply_replicated` (compiler-enforced, but state this explicitly in
  the report).
- The failing-then-passing test evidence for the stuck-watermark
  reproduction (success path) and the failure-path Aborted-marking
  test.
- Confirmation existing replication/apply_replicated tests still pass.
- Full test/gate results (exact commands + pass/fail).
