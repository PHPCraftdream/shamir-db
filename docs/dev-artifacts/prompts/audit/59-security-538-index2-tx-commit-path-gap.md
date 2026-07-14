Task #538 — close the index2 CREATE INDEX lost-write race on the tx-commit
path. Follow-up from #534's `@fl` adversarial review, which found the
`index2_create_barrier` fix landed in #534 only covers the non-tx write
path (`table_manager_crud.rs`'s `insert`/`insert_many_returning_version`/
`delete_returning_version`/`set` — used by replication-apply and tests),
NOT the tx-commit path every real client DML statement actually goes
through.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Read first — full accounting of what #534 closed and didn't

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs::backfill_index2_backend`'s
doc comment and `crates/shamir-engine/src/table/table_manager.rs::needs_write_barrier`'s
doc comment are the source of truth for exactly what #534 left open. Read
both before starting — do not re-derive from scratch.

## The gap, in two independent parts (confirmed by the orchestrator's own
## trace — re-verify line numbers, code may have shifted)

Every real client DML statement runs through a tx (implicit or interactive):
`execute_insert_tx`/`execute_update_tx`/`execute_delete_tx`/`execute_set_tx`
in `crates/shamir-engine/src/table/write_exec.rs`, which STAGE via
`insert_tx_many_bytes`/`update_tx_bytes`/`delete_tx` in
`table_manager_tx_ops.rs` (multiple `self.index2_registry.all_backends().await`
call sites — currently around lines 58, 84, 111, 138, 159, 184, 211, 518,
686, 890, grep to confirm), then MATERIALIZE later through the commit
pipeline (`crates/shamir-engine/src/tx/pre_commit.rs`'s Phase 2.5 prelock,
then `commit_phases.rs`'s Phase 5a/5c).

**Part A — the commit-time prelock doesn't cover index2-only tables.**
`pre_commit.rs`'s Phase 2.5 (~line 266-297) builds `unique_tokens` ONLY from
`tx.unique_guards` (tables with staged unique-key claims) and acquires each
one's `unique_write_lock`. An index2-only table (fts/functional/vector, no
legacy unique index) contributes NOTHING to `unique_guards`, so this tx's
Phase 5a (row materialization) can freely interleave with a concurrent
`create_index_v2`'s backfill on that table — the exact lost-write window
#534 targeted, just unreachable through this path.

**Part B — even if Part A is fixed, the ops-PLAN is stale.** Index2 write
ops (which postings to write) are planned at STAGE time against an
`all_backends()` snapshot — this can happen well before commit (especially
for a multi-statement interactive tx), and definitely before Phase 2.5's
prelock is ever acquired. Concretely: if a tx STAGES before a
`create_index_v2` starts, then COMMITS after that create has already
finished and registered the new backend, this tx's physical row write
(Phase 5a) happens strictly after the backfill already ran — so the
backfill can never have seen this row (it wasn't in the data store yet)
— AND the tx's ops-plan (captured at stage time) has no ops for the new
backend, so Phase 5c won't write a posting for it either. This is a
GUARANTEED miss (not just a rare race), and it exists independently of
Part A — fixing Part A alone (serializing commit against the barrier)
does NOT fix it, because the plan itself was already stale before the
lock was ever consulted.

## What a correct fix needs (investigate both parts, they're independent)

**Part A fix:** extend Phase 2.5's prelock predicate to ALSO acquire
`unique_write_lock` for any table whose `index2_create_barrier` is up
(`TableManager::needs_write_barrier()`-equivalent check), not just tables
in `tx.unique_guards`. This mirrors the non-tx fix from #534 and uses the
SAME lock, so it composes with the existing unique-table serialization
(sorted-token-order ABBA prevention already in place — verify your change
preserves that ordering guarantee if you add more tokens to the same sort+
dedup+lock sequence).

**Part B fix (the harder one — investigate before committing to an
approach):** the index2 ops-plan must reflect the backend set that will
actually be live when the row commits, not a stale stage-time snapshot.
Options to weigh:
1. **Re-snapshot `all_backends()` at COMMIT time** (inside Phase 5a/5c,
   under the Part-A lock, immediately before applying index ops) instead
   of trusting the stage-time plan. This is likely the cleanest fix but
   touches the commit pipeline's phase structure — review how deeply
   `commit_phases.rs` depends on the STAGED plan shape (WAL entry
   construction order, interner delta sequencing) before assuming this is
   a drop-in change.
2. **Block staging itself** on the barrier (gate the `all_backends()`
   calls in `table_manager_tx_ops.rs` on the same lock/barrier check used
   in Part A) so a tx can never stage against a stale snapshot while a
   create is in flight. This closes the "stage-during-create" sub-case but
   NOT the "stage-before-create-starts, commit-after-create-finishes"
   sub-case described above (since staging happened before the barrier
   ever went up) — evaluate honestly whether this is a meaningful
   improvement or a false sense of security, and say so in your report
   either way.
3. Some combination, or a different mechanism entirely if your
   investigation finds a cleaner seam — this brief is not prescribing the
   final design, it is scoping the problem and the constraint (no new
   deadlock, no correctness regression on the existing unique-index path).

**Whatever you choose, verify no new deadlock**: `create_index_v2` already
holds `unique_write_lock` for its full backfill duration; the commit
pipeline waiting on the SAME lock is the accepted trade-off (matches
`create_unique_index`'s own precedent). Check there is no path where a
commit already holds some OTHER lock that `create_index_v2` (directly or
transitively) also needs — trace this explicitly, don't just assert it.

## Explicit permission to scope down

This is a genuinely hard problem (the task's own text — read the full
picture above before assuming a quick fix exists). If, after real
investigation, Part B's clean fix (option 1) proves too risky to land
safely within this task's scope (e.g. it would require restructuring
`commit_phases.rs` in ways that risk the WAL/interner sequencing
invariants), it is FINE to:
- Fully close Part A (the commit-time prelock gap) — a real, valuable
  improvement on its own.
- For Part B, either implement option 2 as a partial mitigation (clearly
  documenting what it does and doesn't close, matching the honest-partial-
  fix style used in #534's own doc comments), or leave it fully open and
  file a properly scoped follow-up task with the same rigor #534's
  followup (this very task) got.
- Do NOT force a risky rewrite of the commit pipeline to chase a
  complete fix. An honest partial fix + a well-scoped follow-up is the
  established, preferred outcome in this campaign over an overclaimed or
  under-tested "complete" fix.

## Test requirement

A tx-path sibling of #534's `insert_during_index2_create_is_not_lost` test
(`crates/shamir-engine/src/table/tests/index2_create_barrier_tests.rs`) —
drive a concurrent tx INSERT/UPDATE/DELETE (via `execute_insert_tx` et al.,
NOT the non-tx `TableManager::insert`) into the exact backfill→register
window using the same `create_index2_backfill_hook` pause mechanism, and
prove the row is NOT lost for whatever scope you actually close. Verify
this test genuinely FAILS against today's code (the gap this task exists
to close) before you start fixing, and again after applying just Part A
(to confirm Part B's guaranteed-miss case, if you scope down to Part A
only, is HONESTLY documented as still open rather than silently assumed
fixed).

## Test scope

```
./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Part A: exact change, confirmed no ABBA/deadlock regression
  > Part B: which option chosen (or none — scoped to Part A only), and
    an honest accounting of what remains open if not fully closed
  > New tx-path regression test: confirmed RED before the fix, GREEN after
    (for whatever scope was actually closed)
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx: pass/fail
```

Given the commit-pipeline surface this touches (WAL/interner/phase
sequencing — high blast radius), this MUST go through an adversarial
review pass before committing, same discipline as #534/#537 this session.
If that review finds a genuine bug, the orchestrator fixes it directly
(never re-delegates), re-verifies, and sends the fix through a second
review pass before committing.
