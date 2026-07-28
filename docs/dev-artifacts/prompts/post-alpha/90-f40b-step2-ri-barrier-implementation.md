# Brief for F-40b Step 2 (#856, P2) — full RI barrier implementation for explicit-Snapshot FK parent mutations

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-40b Step 1 (`docs/dev-artifacts/research/f40b-ri-barrier-spike.md`,
commit `bef9701e`) settled the design and proved the mechanism with a
single recording site + a single guard-widening site + a deterministic
race test. **Read that memo in full first** — it is the source of truth
for both settled decisions and the exact remaining touch points (§5).

This step extends the prototype to the FULL production implementation:
every FK reverse-check scan records into the barrier, and every
commit-pipeline guard that validates predicates also checks it.

**Read these three files in full before writing anything**, in this
order:

1. `crates/shamir-tx/src/tx_context.rs` — the barrier API already exists:
   `ri_barrier_tokens: Mutex<TFxSet<u64>>`, `record_ri_barrier(&self,
   table_token: u64)`, `ri_barrier_tokens_is_empty(&self) -> bool`,
   `append_ri_barrier_deps(&self, deps: &mut Vec<PredicateDep>)`. Do NOT
   change this API's shape — Step 1 already settled it (flat
   `TFxSet<u64>`, `PredicateDep::TableScan { table_token }`). You are
   only calling it from new sites.
2. `crates/shamir-engine/src/query/batch/fk_restrict.rs` —
   `child_has_reference` (~line 292-373) already calls
   `tx.record_ri_barrier(table.table_token())` at function entry, before
   the index fast-path. This is the reference pattern for every other
   recording site below.
3. `crates/shamir-engine/src/tx/pre_commit.rs` —
   `pre_commit_locked_validate`'s Phase 2-bis (~line 460-480) already
   merges `predicate_set` deps with `ri_barrier_tokens` deps via
   `append_ri_barrier_deps` and calls `gate.predicate_conflicts_batch`
   unchanged. This is the reference pattern for the `pre_commit_locked`
   widening below.

## What to add — 2 remaining recording sites

### 1. `crates/shamir-engine/src/query/batch/fk_actions.rs`

Two child-table scan entry points, both currently unconditional
`child_table.list_stream_tx(Some(tx), batch_size)` calls that record
nothing into the barrier today:

- `plan_cascade_recursive` (~line 397): `let stream =
  child_table.list_stream_tx(Some(tx), batch_size);` — add
  `tx.record_ri_barrier(child_table.table_token());` immediately before
  this line (mirroring `child_has_reference`'s "record before the scan,
  regardless of isolation" placement).
- `plan_cascade_for_ids` (~line 688): the identical
  `child_table.list_stream_tx(Some(tx), batch_size)` call in the
  grandchild-recursion arm — same one-line addition immediately before
  it.

Do **not** add a recording call to `collect_parent_values` in this file
— it scans the PARENT table (rows about to be deleted), not a child
table. The barrier's purpose is catching a concurrent write to a CHILD
table between this tx's scan and its commit; recording the parent scan
would not close that race and would just cost an extra token per commit
for no correctness benefit.

### 2. `crates/shamir-engine/src/query/batch/fk_on_update.rs`

Two sites:

- `child_has_reference` (~line 804-867) — mirrors
  `fk_restrict.rs::child_has_reference` exactly (same index-fast-path /
  scan-fallback shape, same staged-overlay probe). Add
  `tx.record_ri_barrier(table.table_token());` at function entry (before
  the `let interner = table.interner().get().await?;` line), so it fires
  regardless of which sub-path is taken — identical placement rationale
  to the already-fixed `fk_restrict.rs` site.
- `plan_fk_on_update`'s CASCADE/SET-NULL child scan (~line 411): `let
  stream = child_table.list_stream_tx(Some(tx), batch_size);` — add
  `tx.record_ri_barrier(child_table.table_token());` immediately before
  it, same pattern as `fk_actions.rs`.

Do not add a recording call to `collect_parent_values` in this file
either — same reasoning (it scans the parent table).

## What to widen — 2 remaining commit-pipeline guard sites

### 3. `crates/shamir-engine/src/tx/pre_commit.rs` — `pre_commit_locked` (the legacy `AsyncIndex` path)

Around line 628 (grep for `Phase 2-bis (SSI only, Phase C)` inside
`pre_commit_locked` — NOT `pre_commit_locked_validate`, which is already
widened):

```rust
if tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty() {
    let deps = tx.predicate_set.snapshot_deps();
    if let Some(idx) = gate.predicate_conflicts_batch(&deps, tx.snapshot_version) {
        let dep = format!("{:?}", deps[idx]);
        repo.tx_metrics().on_tx_aborted_phantom();
        return Err(TxError::PhantomConflict { dep });
    }
}
```

Widen this **identically** to how `pre_commit_locked_validate`'s Phase
2-bis was already widened in Step 1: also fire when
`!tx.ri_barrier_tokens_is_empty()`, merge `predicate_set` deps with
`ri_barrier_tokens` deps via `append_ri_barrier_deps`, call
`predicate_conflicts_batch` unchanged. Read Step 1's diff of
`pre_commit_locked_validate` (`git show bef9701e -- crates/shamir-engine/src/tx/pre_commit.rs`)
for the exact shape to replicate — this is a second instance of the
identical pattern, not a novel design.

Note this path already always runs under `commit_lock` (per the existing
comment at ~line 640-645), so — unlike `commit_tx_lockfree` — no
additional lock-widening is needed here for this path specifically.

### 4. `crates/shamir-engine/src/tx/group_commit.rs` — inter-batch phantom check (~line 184-202)

```rust
let phantom_conflict = if entry.tx.isolation == IsolationLevel::Serializable
    && !entry.tx.predicate_set.is_empty()
    && !batch_footprints.is_empty()
{
    let mut conflict_dep: Option<String> = None;
    entry.tx.predicate_set.with_iter(|dep| {
        if conflict_dep.is_none() {
            for fp in &batch_footprints {
                if shamir_tx::record_conflicts(fp, dep) {
                    conflict_dep = Some(format!("{:?}", dep));
                    break;
                }
            }
        }
    });
    conflict_dep
} else {
    ...
};
```

This is a **per-dep** check (`predicate_set.with_iter` + `record_conflicts`
per footprint), not the batch form the other two sites use — read the
full surrounding function to understand `batch_footprints`'s shape and
this check's role (it closes the INTRA-batch phantom gap, since the
committed log check inside `pre_commit_locked_validate` only sees
already-committed writes, not sibling transactions still landing in the
same grouped-commit batch) before touching it.

Widen the condition to ALSO run when `!entry.tx.ri_barrier_tokens_is_empty()`
(in addition to, or instead of, the Serializable-and-predicate_set
condition — the barrier tokens must be checked regardless of isolation,
same as everywhere else), and extend the dep iteration to also walk
`ri_barrier_tokens` (build `PredicateDep::TableScan { table_token }`
values via `append_ri_barrier_deps` into a `Vec`, or iterate the tokens
directly if that's simpler given `with_iter`'s callback shape — use
whichever integrates most naturally with the existing per-dep loop,
your judgment call, but the resulting behavior must be: every
`ri_barrier_tokens` entry gets checked against every `batch_footprints`
entry via `record_conflicts`, exactly as `predicate_set` entries already
are).

Note `group_commit.rs:635` already maps `TxError::PhantomConflict` →
`DbError::Conflict(...)` → `"tx_conflict"` (confirmed in Step 1's memo
§1.2 investigation) — no new error-code plumbing needed here either.

## Tests to add

1. **End-to-end explicit-tx race tests for CASCADE, SET NULL, and
   on-update**, mirroring `fk_ri_barrier_spike_tests.rs`'s existing
   RESTRICT race test (`explicit_snapshot_restrict_race_closed_via_ri_barrier`)
   and its quiescent counterpart. For each of the 3 remaining actions,
   at minimum one race test (explicit-Snapshot parent DELETE or UPDATE
   racing a concurrent child write, proving the barrier catches it —
   `PhantomConflict`, never a silent orphan/dangling-reference/missed-fanout)
   plus folding the existing quiescent test's methodology (or extending
   it) to cover the new actions with zero spurious aborts.
   - Rename `fk_ri_barrier_spike_tests.rs` to a permanent name (drop
     "spike" — e.g. `fk_ri_barrier_tests.rs`) since this is no longer a
     throwaway prototype; update the `mod` declaration in
     `crates/shamir-engine/src/query/batch/tests/mod.rs` to match. Add
     the new tests to this renamed file (or fold everything into
     `fk_race_closure_tests.rs` if you judge that's a better fit once
     you've read both files — your call, but pick one and don't
     duplicate the harness).
2. Confirm the full existing FK suite (`./scripts/test.sh -p shamir-engine -- fk`)
   still passes unchanged — zero regressions on the implicit-path race
   closure tests from F-28.

## Documentation to update

**`docs/guide-docs/KNOWN_LIMITATIONS.md`** — the FK entry's "Residual
scope" bullet (re-synced most recently by F-45, commit `34ec0292`) states
the explicit-Snapshot gap is OPEN, scoped via the F-40 memo and this
follow-up task. Once this step's mechanism is fully landed and tested,
rewrite that bullet to record it as **CLOSED for explicit Snapshot via
the RI barrier** — mirroring how the entry already describes the
implicit path's F-28 Step 5 closure. Cite this task's commit(s) and the
two memos (`f40-explicit-snapshot-ri-gap-memo.md`,
`f40b-ri-barrier-spike.md`). Read the current entry in full first (it
has shifted since F-45 touched it) — this is a re-sync to new ground
truth, matching the existing entry's precision level, not a rewrite.

## Explicitly out of scope

- `FkReverseCache` internals (F-35/F-36, already landed) — untouched.
- The client-retry-wrapper question (§1.2 / §5.3 point 7 of the Step 1
  memo) — already settled as "no wrapper, client retries on
  `tx_conflict` if it wants one." Do not add a retry wrapper to
  `interactive_tx.rs`.
- The dep-dedup micro-optimization (Step 1 memo §5.3 point 8) — optional,
  skip unless trivial once you're in the code; do not spend meaningful
  effort on it.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-tx -- --check` and
  `cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings`
  must be clean.
- Keep the diff surgical — this is a mechanical extension of an already-
  settled, already-proven pattern to more sites, not a redesign. If you
  find yourself wanting to change the `TxContext` barrier API's shape or
  the `PhantomConflict`/`"tx_conflict"` error-code decision, STOP and
  document why in your final summary instead of changing settled Step 1
  decisions unilaterally.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine -p shamir-tx --full
```

When done, give your final summary as plain text: every recording site
and guard-widening site actually touched (confirm all 4: 2 recording +
2 guard), what new tests were added and their pass/fail results
(including the race + quiescent numbers for each of the 3 new actions),
whether `KNOWN_LIMITATIONS.md` was updated and how, full test run
output, and confirmation fmt/clippy are clean.
