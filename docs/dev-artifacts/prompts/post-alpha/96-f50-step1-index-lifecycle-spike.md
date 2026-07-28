# Brief for F-50 Step 1 (#869, P0, spike) — online index build lifecycle design

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is a **timeboxed design spike**, mirroring this session's own
established precedent for genuinely complex concurrency/lifecycle design
questions (F-40b's Step 1→Step 2 split, and F-48's own follow-up F-48b).
**Read `docs/dev-artifacts/research/f40b-ri-barrier-spike.md` first** as
the template for this spike's shape, rigor, and deliverable format.

The 2026-07-28 readonly review
(`docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`, §3
P0-5) found that online index creation (`create_index_v2`) still allows
transactions to guaranteed-miss a newly-created index. **This is NOT a
rare race — the codebase's OWN doc comment already names it a "GUARANTEED
miss (not a rare race)".** Read
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s
`backfill_index2_backend` doc comment (~line 346-414) in full FIRST — it
precisely describes the mechanism (§"Part B — still OPEN") this spike
must settle a design for.

## What's already been closed this session (do not re-investigate)

- **F-48/F-48b** landed a reusable `WriterDrainBarrier`
  (`crates/shamir-engine/src/table/writer_drain_barrier.rs`) and wired it
  into BOTH the non-tx writer methods (`table_manager_crud.rs`) AND the
  tx-commit prelock (`pre_commit.rs`'s Phase 2.5). Since
  `needs_write_barrier()` is barrier-agnostic (ORs
  `schema_activation_barrier` and `index2_create_barrier`), this
  **incidentally closes item 3** from `backfill_index2_backend`'s doc
  ("Check-then-act, not a drain") for `index2_create_barrier` too — BUT
  ONLY once `create_index_v2` itself calls `self.drain_writers().await`
  after raising `index2_create_barrier` (mirroring EXACTLY how
  `admin_schema.rs`'s `SchemaActivationBarrierGuard::raise` now calls it
  for the schema-activation barrier — see that code, commit `c2cb6e13`).
  **This one-line wiring (`drain_writers()` call in `create_index_v2`,
  right after `Index2CreateBarrierGuard::set` at
  `table_manager_index_mgmt.rs:82`) is a cheap, mechanical, already-solved
  piece — settle whether to include it in this spike's prototype or defer
  it explicitly to Step 2's implementation list (your call, but it's
  small enough to just do here if convenient).**

## The genuinely open problem: Part B (stale staged ops-plan)

**Already confirmed by direct code reading this session — do not
re-derive, build on this:**

`table_manager_tx_ops.rs`'s `plan_insert_ops`/`plan_update_ops`/
`plan_delete_ops` (lines ~51-66, ~151-167, ~201-219) each call
`self.index2_registry.all_backends().await` ONCE, at STAGE time (inside
`insert_tx`/`update_tx`/`delete_tx`), producing `Vec<IndexWriteOp>` that
get appended into `tx.index_write_set`. If a NEW index2 backend is
registered (via `create_index_v2`'s `self.index2_registry.insert(backend)`,
`table_manager_index_mgmt.rs:322-325`) AFTER a tx already took its
`all_backends()` snapshot but BEFORE that tx commits, the tx's
`index_write_set` permanently has zero ops for the new backend.

**Critical confirmed finding: this cannot be fixed by simply re-deriving
ops later in the commit pipeline (e.g. right before Phase 5c apply).**
`crates/shamir-engine/src/tx/commit.rs`'s `wal_ops_from_tx` (line
282-330) directly serializes `tx.index_write_set` into
`WalOpV2::IndexPut`/`IndexDel` entries — the WAL entry's actual durable
content IS the stage-time ops-plan. Since Phase 4 (WAL begin) is the
commit point, and recovery replays the WAL entry's own logged ops (not a
freshly-re-derived plan), any fix that re-derives ops AFTER Phase 4 would
create a genuine crash-safety hole: a crash after WAL-write but before
Phase 5c apply would recover using the STALE (WAL-logged) ops, silently
reintroducing the exact miss this fix is supposed to close. **A correct
fix must re-derive the ops-plan BEFORE `wal_ops_from_tx` runs** — i.e.
somewhere in Phase 1-3 of the pipeline (`pre_commit.rs`'s
`pre_commit_prelock`/`pre_commit_locked`/`pre_commit_locked_validate`,
all of which run before `commit.rs`'s Phase 4 `wal.begin_grouped` call),
against the LIVE `all_backends()` snapshot at THAT (much later, much
closer to actual commit) point.

## What to settle

### 1. Where and how to re-derive the ops-plan

Investigate: at what point in the pre-Phase-4 pipeline is
re-deriving index2 ops actually FEASIBLE? The re-derivation needs each
staged record's actual final bytes (available via `tx.write_set`) and
must call each LIVE backend's `plan_insert_tx`/`plan_update_tx`/
`plan_delete_tx` fresh. Read `pre_commit.rs`'s `pre_commit_prelock` (all
phases) to determine the right insertion point — likely near Phase 2.5
(where `tx.write_set`'s tables are already being iterated for the
barrier check) or a NEW phase between existing ones. Consider: does this
need to happen for EVERY tx (even ones that never touch an index2-relevant
table), or can it be gated cheaply (e.g. only if `index2_registry`'s
generation/version has changed since the tx started staging — investigate
whether `IndexRegistry` already has a generation counter to check
cheaply, or would need one added)?

### 2. Lifecycle state — at least Building/Ready per backend

Design a minimal PERSISTED lifecycle state per index2 backend (at least
`Building` → `Ready`, richer if genuinely needed — e.g. the review's
suggested `Building → CatchingUp → Ready`). Investigate
`crates/shamir-engine/src/index2/`'s existing metadata persistence
(`crate::index2::persistence::save_index2_metadata`,
`IndexDescriptor`/`IndexRegistry`) to determine the least invasive way to
add a state field. The planner (wherever index2 backends are consulted
for query planning — investigate `read_exec.rs`/`read_index_scan.rs` or
equivalent) must only use `Ready` backends; a `Building` backend is
invisible to the planner even though it exists in the registry (so
concurrent reads are unaffected by an in-progress build).

### 3. Sorted-index's own "cancel-safe: NO" residual

`table_manager_sorted_index.rs`'s `create_sorted_index`/
`create_sorted_index_with_include` (read the doc at line ~7-16) registers
the definition BEFORE the backfill completes — the SAME register-before-
ready hazard, for a DIFFERENT (legacy, not index2) index mechanism.
Determine whether the SAME lifecycle-state approach applies here too, or
whether it needs its own (simpler, since sorted indexes don't have the
tx.index_write_set staleness problem — investigate whether they do; the
doc doesn't mention it, confirm) fix.

### 4. Scope boundary — what belongs in Step 2 vs. a further Step 3

This session's own precedent (F-48 → F-48b) shows a clean split is often
right: F-48 handled what was mechanically reachable in one delegation,
F-48b handled a deeper, separately-scoped follow-up. Decide: does closing
Part B (ops-plan re-derivation) + the lifecycle state + planner
Ready-gating fit in ONE Step 2 implementation, or does crash/restart
continuation (a build interrupted by a crash — does it resume, restart
from scratch, or leave a permanently-`Building` orphan?) and DDL
cancellation (a `DROP` or explicit cancel mid-build not leaving a
partially-queryable index) need their OWN Step 3? State your reasoning;
there is no mandated answer, but do NOT try to force everything into one
implementation step if the investigation shows real design complexity in
the crash/cancellation cases that deserves its own focused pass.

## What to prototype

Prove the mechanism works for ONE simple, concrete case: a single index2
backend (pick `functional` or `fts`, whichever seems simplest once you're
reading the code), single-row insert.

1. **Adversarial red test FIRST**: a deterministic test proving the
   CURRENT "guaranteed miss" — an explicit tx stages an insert (takes its
   `all_backends()` snapshot with the new backend NOT yet registered),
   THEN a concurrent `create_index_v2` completes (backfill + register),
   THEN the tx commits. Prove the newly-created index has NO posting for
   the tx's row (the guaranteed miss) on the CURRENT code. This does NOT
   need the pause-seam infrastructure other tasks this session used (the
   race is deterministic by construction — stage, then create-index,
   then commit, in that literal order — no interleaving injection
   needed).
2. **Apply your settled fix** for this one case and make the test pass:
   the tx's commit must either (a) genuinely include a posting for the
   new backend (ops re-derived against the live registry before WAL
   Phase 4), or (b) if you determine synchronous re-derivation is
   infeasible for some concrete reason found during investigation, some
   other mechanism that closes the miss (state your reasoning precisely
   if you land on something other than (a)).
3. Verify the existing `index2_create_barrier_tests.rs` suite (including
   `stage_and_commit_inside_window_still_misses_new_index_part_b_open` —
   the EXISTING test that honestly reproduces this exact residual, cited
   in the doc comment at line 411) — your fix should make THIS EXISTING
   TEST'S NAME A LIE if it now passes where it used to document a known
   miss; investigate whether to update/rename it once your fix closes
   what it was proving open, or whether your prototype's narrower scope
   doesn't yet cover what that specific test exercises (state which).

## Deliverable

A decision memo at `docs/dev-artifacts/research/f50-index-lifecycle-spike.md`
(mirroring `f40b-ri-barrier-spike.md`'s structure): the settled design for
ops-plan re-derivation timing/mechanism, the lifecycle-state shape,
whether the sorted-index residual shares the fix, the Step 2 vs. Step 3
scope decision with reasoning, the prototype's proof (red→green, actual
test output), and a PRECISE implementation plan for Step 2 (and Step 3 if
you decide one is needed) with exact touch points — every file/function
that needs the ops-plan re-derivation applied for the OTHER backend kinds
(vector's HNSW staging via `tx.staged_vectors` may need special handling —
investigate and note), the sorted-index fix, the planner-gating change,
and whatever crash/cancellation scope you assign to which step.

## Constraints

- Timebox this — it's a spike. If the ops-plan re-derivation proves
  substantially harder to prototype correctly than expected, STOP,
  document the difficulty and what you learned in the memo, and let
  Step 2 handle the harder mechanics with the design questions still
  settled from your investigation.
- Do NOT implement the fix for all four backend kinds, the sorted-index
  fix, or crash/restart/cancellation in this spike — prototype ONE case
  only, per "What to prototype" above.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean if any prototype code is committed.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- index2_create_barrier
./scripts/test.sh -p shamir-engine --full
```

When done, give your final summary as plain text: your settled design
for ops-plan re-derivation (where/how, and why the alternatives were
rejected), the lifecycle-state design, the sorted-index residual's
relationship to this fix, the Step 2/Step 3 scope decision and reasoning,
the prototype's red→green proof with actual test output, the memo's
implementation plan, and confirmation fmt/clippy are clean if you
committed prototype code.
