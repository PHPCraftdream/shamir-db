# Brief for F-28 Step 3 (#830, P1) — timeboxed spike: decide the cross-transaction race-closure mechanism

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## This is a RESEARCH/SPIKE task, not a normal fix+test task

The deliverable is a **decision memo** (a committed markdown doc) plus
whatever throwaway experiments you needed to reach a confident
recommendation. Production code is NOT expected to land from this task —
Step 5 (#832) does the real implementation, informed by your memo.

## Context

F-28 Step 2 (#829, landed) closed the **in-transaction** read-your-own-
writes gap in the reverse-FK checks (RESTRICT/CASCADE/SET NULL/ON
UPDATE). What remains open is the **cross-transaction** race documented
in `fk_restrict.rs`'s doc comment: a genuinely CONCURRENT other
transaction's write (e.g. a new child-row insert) landing between this
transaction's FK check/plan and its own commit.

Two candidate mechanisms were identified by an earlier investigation
(`@oh`, this session — full detail below is a condensed version of that
investigation; re-derive/re-verify anything you rely on rather than
taking it purely on faith, since this spike's job is to validate the
recommendation with real evidence):

### S3-C — lock-free, Serializable isolation + footprint fix (tentatively recommended)

Upgrade the implicit FK-relevant delete/update tx from `Snapshot` to
`Serializable` isolation (targeted — only when the table is schema-
flagged as an FK parent with a non-`NoAction` action, not globally). The
tx-aware probes from Step 2 (`list_stream_tx`) ALREADY record
`PredicateDep`s automatically when `tx.isolation == Serializable`
(`crates/shamir-engine/src/table/table_manager_streaming.rs` ~line
235-258). The existing Phase 2-bis phantom-conflict validation
(`crates/shamir-engine/src/tx/pre_commit.rs` ~line 460) then aborts the
commit if a real race occurred.

**Blocker to verify and prototype a fix for**: `build_footprint_from_tx`
(`crates/shamir-tx/src/repo_tx_gate.rs` ~line 948-956) returns an EMPTY
footprint for any tx whose `isolation != Serializable` — meaning a
concurrent SNAPSHOT-isolation child-table write (the common case today,
e.g. a plain autocommit insert into the child table) publishes NOTHING,
so even a Serializable FK-parent-delete's phantom-conflict check has
nothing to conflict against. Verify this by reading the function, then
prototype a fix: a `TxContext.footprint_tokens: TFxSet<u64>` populated at
stage time (when the table is flagged as an FK-child-with-action),
consulted by `build_footprint_from_tx` to widen publication beyond
"all tables iff Serializable" to "iff Serializable OR token is in
footprint_tokens".

**A second prerequisite to verify**: an apparent footprint/publish
ordering inconsistency on the AsyncIndex commit path — `materialize.rs`'s
own doc comment (~line 40-46) states Phase 6-bis (`record_commit_writes`)
must run BEFORE Phase 6 publish, but `commit.rs` (~line 618-619) appears
to call `version_guard.commit()` (publish) THEN `record_commit_writes`.
If this is real, a concurrent validator scanning `commit_write_log` could
observe a published version whose footprint isn't logged yet — a missed
conflict. Confirm whether this is genuinely a bug (read both files
carefully, trace an actual commit through both the sync and AsyncIndex
paths) or whether you're misreading something — this is exactly the kind
of "verify, don't assume" step this spike exists for.

### S3-A — per-table barrier lock (fallback if S3-C's abort rate or footprint-ordering fix proves too invasive)

A new per-table `fk_barrier: Arc<tokio::sync::RwLock<()>>` (sibling of the
existing `unique_write_lock`, `crates/shamir-engine/src/table/
table_manager.rs` ~line 49/526). Parent-side delete/CASCADE/SET NULL takes
`write()` on every affected CHILD table's barrier (sorted table-token
order, INCLUDING the parent's own table if it is itself an FK child — must
be write-mode once in the sorted set to avoid self-deadlock, tokio locks
are not reentrant), held through commit. Child-side writes to a flagged
FK-child table take `read()` for their own stage→commit window. Guards
must be adopted by `TxContext` and released after `materialize`/
`post_publish_cleanup` (mirroring existing `uwl_guards` plumbing in
`pre_commit.rs`/`materialize.rs`) since an explicit-tx batch's guard must
live until the BATCH commits, not just the individual op.

## What to actually do

1. **On a throwaway worktree/branch** (this agent run may already be
   isolated in its own worktree — check your environment; if not, use a
   scratch branch and be scrupulous about not touching the main working
   tree's tracked files outside your own scratch commits), prototype S3-C's
   three pieces: the `footprint_tokens` widening, the footprint/publish
   ordering fix (if confirmed real), and the targeted Serializable upgrade
   for FK-relevant implicit deletes.
2. **Build a deterministic concurrent-race test harness** mirroring
   `GateBarrierResolver` (`crates/shamir-engine/src/query/batch/tests/
   executor_tests/ssi_tests.rs` ~line 20-67) — a `TableResolver` whose
   `resolve()` call, on a specific counted invocation, runs a COMPLETE
   competing `execute_batch` to commitment inline. This injects a
   concurrent committed write at an exact program point with zero timing
   dependence (no sleeps). Use it to test:
   - A parent delete under RESTRICT racing a concurrent child insert — the
     invariant is `parent_deleted XOR child_still_references_it` (never
     both false, i.e. never "parent gone AND a live child reference
     exists").
   - The SAME scenario under CASCADE — invariant: no orphan (every
     surviving child's FK value resolves to a live parent, OR the child
     itself was cascaded).
   - A quiescent-DB abort-rate check: with NO concurrent writer, an FK
     parent delete under your S3-C prototype must NOT spuriously abort
     (measure this — a false-positive conflict on the common single-writer
     case would be a real regression, not just a theoretical concern).
3. **Measure and decide.** Does the race actually get caught? What's the
   abort rate on a quiescent DB (must be ~zero)? Is the footprint-ordering
   fix (if needed) contained to a small, reviewable diff, or does it ripple
   further than expected? Based on the evidence, recommend S3-C or fall
   back to recommending S3-A.
4. **Write the decision memo**: `docs/dev-artifacts/research/f28-s3-mechanism-decision.md`.
   Include: what you verified (with file/line citations), what you
   prototyped, your test harness's actual results (pass/fail, timing,
   abort rates), and a clear recommendation for Step 5 with a rough shape
   of what Step 5's implementation brief should cover. If you found either
   prerequisite (footprint-ordering, footprint-token widening) is itself
   nontrivial, say so explicitly — Step 5's brief will need to account for
   it as its own sub-piece.

## What NOT to do

- Do NOT implement S0 (#831, the reverse-FK cache/flags) — that's a
  separate task, ordered AFTER this spike specifically because this
  spike determines which flags are actually needed.
- Do NOT land production code in the main working tree as this task's
  final state UNLESS you and the orchestrator explicitly agree the
  prototype should become the basis for Step 5 (coordinate this in your
  final summary — state clearly whether you're leaving prototype code in
  place or have reverted it, and why).
- Do NOT treat "all tests green" as this task's completion bar the way
  other tasks in this campaign work — the completion bar here is "a clear,
  evidence-based decision memo, committed."

## Verification the orchestrator will run

Since this is a spike, the orchestrator will primarily review your
decision memo for reasoning quality and re-run your race-harness tests
(if you left them in a runnable state) rather than running a blanket
fmt/clippy/full-suite gate on speculative prototype code. Tell the
orchestrator explicitly, in your final summary, exactly which commands to
run to reproduce your findings.
