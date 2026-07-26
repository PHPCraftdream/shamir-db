# Brief for F-28 Step 5 (#832, P1) — implement S3-C (Serializable upgrade + footprint widening)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context — read these two artifacts FIRST, in full

1. `docs/dev-artifacts/research/f28-s3-mechanism-decision.md` — F-28 Step
   3's spike, recommending S3-C over S3-A, with citations for every piece
   below. Its "§4 What Step 5 must still do" section is this brief's
   direct source.
2. `crates/shamir-engine/src/repo/fk_reverse_cache.rs` — F-28 Step 4
   (#831, landed, commit `7496f207`), which built the O(1) role-flag API
   this task consumes: `FkReverseCache::is_fk_parent_with_action(table)`
   and `is_fk_child(table)` (both currently `#[allow(dead_code)]` —
   remove that attribute once you wire them in), reachable via
   `RepoInstance::fk_reverse_cache()`.

Note the spike's prototype code was built in an isolated worktree that
predates F-28 Steps 1/2/4 on `master` (see the memo's "orchestrator note"
at the top) — it was NOT merged. You are re-deriving/re-implementing the
mechanism against CURRENT `master`, using the memo as your design
reference, not as a patch to apply.

## What to build

### 1. `TxContext.footprint_tokens` widening (memo §1.2)

In `crates/shamir-tx/src/tx_context.rs`: add `footprint_tokens:
TFxSet<u64>` and a `require_footprint_for(&mut self, table_token: u64)`
builder method (append to the set).

In `crates/shamir-tx/src/repo_tx_gate.rs`: `build_footprint_from_tx`
currently returns an EMPTY `CommitWriteRecord` for any tx whose
`isolation != Serializable`. Widen the per-table inclusion gate to
`tx.isolation == Serializable OR tx.footprint_tokens.contains(&token)`
instead of gating the WHOLE function on Serializable. Off the common path
(`footprint_tokens` empty, Snapshot) this must be a single `is_empty()`
check — zero-overhead, byte-identical behavior to today for every
existing caller that never touches `footprint_tokens`.

### 2. AsyncIndex footprint/publish ordering fix (memo §1.3)

In `crates/shamir-engine/src/tx/commit.rs`'s `commit_tx_inner_legacy_async`
(the ONLY commit path with this bug — the default `commit_tx_lockfree`
path already orders these correctly): swap the two calls so
`gate.record_commit_writes(...)` runs BEFORE `version_guard.commit()`,
matching `materialize.rs`'s own documented Phase 6-bis-before-Phase-6
ordering. Add a doc comment explaining why (a concurrent Serializable
validator reading `last_committed()` between the two calls must never
see this tx's version published with no footprint recorded for it yet).

### 3. Wire the real trigger: Serializable upgrade + footprint-token calls

This is the part the spike deliberately did NOT do (it hand-set both in
its test harness). Two symmetric hooks, both driven by F-28 Step 4's O(1)
flags:

- **Parent side** (implicit delete-begin time): in
  `crates/shamir-engine/src/query/batch/query_runner.rs`'s `BatchOp::Delete`
  implicit arm (the `None =>` branch that calls
  `repo.begin_implicit_batch_tx(...)`, per F-28 Step 1/#828), check
  `repo.fk_reverse_cache().is_fk_parent_with_action(&table_ref.table)`
  BEFORE calling `begin_implicit_batch_tx` — if `true`, the tx must open
  as `Serializable` instead of the hardcoded `Snapshot`. Investigate
  whether `begin_implicit_batch_tx` needs a new parameter (an
  `IsolationLevel` override) or whether a small sibling method is cleaner
  — check `RepoInstance::begin_tx`'s existing signature (it already takes
  `isolation: IsolationLevel`) to decide the smallest, most consistent
  change. **Only the DELETE path needs this today** (RESTRICT/CASCADE/SET
  NULL only trigger on delete and on-update actions per this codebase's
  FK model) — verify whether `BatchOp::Update`'s implicit arm (which also
  runs `plan_fk_on_update`) needs the SAME upgrade for symmetry (it
  reads/writes the SAME child tables via the same `list_stream_tx`
  mechanism Step 2 wired up) — if the ON UPDATE fan-out has the identical
  cross-tx race exposure, wire it there too; if you determine it doesn't
  (state your reasoning), it's fine to scope this to DELETE only, but
  make that decision explicit and justified, not accidental.
- **Child side** (insert/update staging time): wherever a row is staged
  into a table (the `execute_insert_tx`/`execute_update_tx` staging path
  in `crates/shamir-engine/src/table/`, or wherever is the correct single
  choke point — investigate rather than guessing), check
  `repo.fk_reverse_cache().is_fk_child(table_name)` and, if `true`, call
  `tx.require_footprint_for(table_token)` on the ACTIVE `TxContext`
  BEFORE the write is staged. This must work for BOTH the explicit-tx and
  implicit-tx paths (any Snapshot-isolation writer touching an FK-child
  table needs its footprint published, regardless of which path staged
  it) — this is the mechanism that makes the PARENT's Serializable
  phantom-check actually have something to conflict against.

### 4. Retry policy for the common case

Per the memo: upgrading an FK-relevant implicit delete to Serializable
means it CAN now abort with `CommitError::PhantomConflict` even in cases
that used to always succeed (Snapshot never aborts). A genuine race
SHOULD abort — but investigate whether a bounded internal retry (re-plan
+ re-run the whole implicit-delete attempt, 2-3 attempts) is warranted so
an extremely rare, already-resolved race doesn't surface as a client-
visible `tx_conflict` for what is, from the caller's perspective, a
perfectly ordinary single delete. Check whether this codebase has an
existing retry-wrapper convention for a similar "opportunistic
Serializable upgrade might need a retry" situation (search for existing
CAS-retry loops in `query_runner.rs`/`db_execute.rs`) and match it; if
none exists, keep the retry logic minimal and clearly bounded (no
unbounded loops), and make the final abort (after retries exhausted)
surface as a clear, coded error — do not swallow it silently.

### 5. Close the "never-yet-interned FK field" gap (memo §2.4)

The spike found that `child_has_reference`'s field-id lookup (now, after
F-28 Step 2, via `resolve_field_id_layered`) must NEVER sit in front of
the `list_stream_tx` call in a way that skips the scan entirely when the
field id can't yet be resolved — the SCAN (and its Serializable predicate
recording) must run unconditionally; only the row-level match should be
skipped when the field id is unresolvable. Re-read `fk_restrict.rs`'s
CURRENT (post-Step-2) `child_has_reference` and confirm this is already
true (Step 2's version calls `resolve_field_id_layered` and then, if
`None`, returns `Ok(false)` EARLY — before `list_stream_tx` runs in the
fallback-scan branch. Check whether this is actually the gap the spike
warned about, or whether Step 2's design differs enough that it's already
safe). If it IS still a gap for the SERIALIZABLE case specifically (an
empty/young child table whose FK field was never interned, causing the
scan-and-thus-predicate-recording to be skipped entirely), fix it: ensure
`list_stream_tx(Some(tx), ..)` always runs at least once (to record the
`TableScan` predicate) even when there is no possible row match to find.

## Tests

**MANDATORY, using the deterministic race-harness pattern** (mirror
`GateBarrierResolver`, `crates/shamir-engine/src/query/batch/tests/
executor_tests/ssi_tests.rs` ~line 20-67 — inject a complete concurrent
`execute_batch` at an exact program point, no sleeps):

1. **End-to-end race closure**: a real `execute_batch`-driven test (not
   the spike's low-level harness) — a genuinely concurrent writer
   inserting a new child reference between an FK-parent delete's check
   and its commit must now either be caught (delete aborts with a coded
   conflict) or the delete correctly sees the new reference (RESTRICT
   correctly rejects) — never "delete succeeds AND a dangling/orphaned
   reference exists after both commit."
2. **Quiescent-DB non-regression**: an FK-parent delete with NO
   concurrent writer must NOT spuriously abort — assert this over
   multiple trials (mirror the spike's 50-trial quiescent test).
3. **The retry policy** (if implemented): a test proving a resolved race
   (the retry succeeds on the second attempt) does not surface as an
   error to the caller, and that retry exhaustion (if you can force it)
   surfaces a clear coded error.
4. **The never-interned-field gap** (point 5): a test with a brand-new,
   never-before-written child table (its FK field never interned) racing
   a concurrent insert — confirm the race is still caught (i.e., the scan
   ran and recorded its predicate despite the field being unresolvable at
   scan time).
5. Full regression: existing `fk_restrict_tests.rs`/`fk_actions_tests.rs`/
   `fk_on_update_tests.rs`/`declarative_schema_fk_*_e2e.rs` suites must
   all still pass unchanged in behavior (aside from any genuinely new
   Serializable-abort behavior this task intentionally introduces for the
   race-adjacent cases — do not let this task's changes alter results for
   any test that isn't specifically about the race window).

## Docs

Update the three modules' TOCTOU doc comments (`fk_restrict.rs`,
`fk_actions.rs`, `fk_on_update.rs`) to state the cross-transaction race is
now CLOSED (or precisely describe what residual remains, if your
implementation doesn't fully close every case — be honest, matching this
campaign's standard). Leave the final, comprehensive KNOWN_LIMITATIONS.md
write-up to F-28 Step 6 (#833) — do not duplicate that work here, but DO
make sure your own module-level doc comments are accurate for Step 6 to
summarize from.

## Constraints

- Do NOT implement S3-A (the barrier lock) — S3-C is the decided
  mechanism.
- Do NOT touch F-28 Steps 1/2/4's existing logic beyond what's needed to
  wire in the two new hooks (Serializable upgrade, `require_footprint_for`
  calls) — this is additive, not a rewrite.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-tx -- --check` and
  `cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D
  warnings` must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-tx
./scripts/test.sh @engine
./scripts/test.sh @oracle
./scripts/test.sh @e2e
```
