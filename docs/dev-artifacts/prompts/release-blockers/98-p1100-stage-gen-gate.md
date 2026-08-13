# #1107 MEDIUM — #1100's "zero-cost on common path" gate never fires: per-row MVCC read + O(N^2) scans on every commit

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a final adversarial review of the whole session's accumulated work
(commit range `b556913f..HEAD`). Confirmed by direct code reading before
this brief was written (line numbers below are current as of `#1106`'s fix,
already merged — re-read the function before editing, it may have shifted
further).

File: `crates/shamir-engine/src/tx/pre_commit.rs`,
`rederive_stale_value_ops_post_stage` (added by `#1100`, fixed further by
`#1106` — this brief does NOT change either prior fix's logic, only adds a
real gate around calling it). Currently gated at `:1875-1877` on
`tx.base_index_stage_gens.is_empty()`.

## The bug

`note_base_index_stage_gen` (which populates `tx.base_index_stage_gens`) is
called **unconditionally** by all six tx staging entry points
(`table_manager_tx_ops.rs:634, 844, 1037, 1190, 1370, 1469` — confirmed by
direct reading, e.g. `:634` sits AFTER the `if self.has_any_index() { ... }`
block that gates the actual index planning, not inside it). So
`tx.base_index_stage_gens` is non-empty for essentially every transaction
that stages ANY row on ANY table — including tables with ZERO indexes. The
"zero-cost on the common path" gate this function's own doc comment claims
(mirroring the sibling `rederive_base_index_ops_post_stage`'s real gate — see
below) never actually short-circuits in practice.

Contrast with `rederive_base_index_ops_post_stage` (`:1630-1653`), which has
the SAME `tx.base_index_stage_gens.is_empty()` outer check, but ALSO a real
per-table gate inside the loop: `if mgr.generation() == stage_gen { continue;
}` — one atomic `Acquire` load, skipping all per-record work when nothing
about the table's index DEFINITIONS changed since staging. This is why that
sibling function genuinely is close to zero-cost on the common path, while
`rederive_stale_value_ops_post_stage` is not.

## Why a straight copy of that gate does NOT work here

`rederive_base_index_ops_post_stage` detects DEFINITION changes (an index
created/dropped between stage and commit) — a table-level index-manager
`generation()` counter is the right cheap signal for that.
`rederive_stale_value_ops_post_stage` detects VALUE changes (a concurrent
transaction modified a SPECIFIC RECORD's indexed fields between this tx's
snapshot and commit) — a table-level generation counter cannot tell you
whether any given record's value changed; that's an inherently
finer-grained, per-record condition. Do NOT copy `mgr.generation()` here
without checking it actually captures the right invariant — it does not, by
construction (creating/dropping an index bumps it; an ordinary row UPDATE by
a concurrent tx does not).

## Consequences on the commit-time critical path (today, unconditionally)

Inside `pre_commit_prelock`, while `uwl_guards` are held, for basically every
commit that stages any row:
- one `tbl.read_one_tx_bytes(rid, None)` MVCC lookup PER staged row (the
  DELETE and UPDATE branches each do this) — e.g. a 10,000-row batch insert
  does 10,000 extra reads that all return `None` (inserts have no prior
  value, so this hits the `if let Some(...)` path's `else` every time)
- for genuine UPDATE/DELETE rows: `InnerValue::from_bytes` decode + a full
  `plan_record_updated{,_unique}`/`plan_record_deleted{,_unique}` re-plan
  per row
- `staged_removals_by_rid` rebuilt INSIDE the per-op loop → `O(N *
  |index_write_set|)`
- the owner/kind-aware dedup scan (`#1106`'s fix) is a linear scan of
  `index_write_set` per re-planned op → `O(N^2)` overall

This is notable because `#1099` was opened specifically to remove an
`O(N^2)` from this exact area (`pre_commit`/`update_tx_bytes`); `#1100`
landed two new `O(N^2)`/per-row-I/O patterns here, apparently without a
working gate ever being verified to actually fire.

## What to do

1. **Find (or add) a genuinely cheap, correct signal for "could ANY staged
   row on this table possibly be stale relative to durable state".** Start
   by investigating whether the codebase already has a per-table or
   per-repo cheap "has anything committed since version V" primitive —
   `TxContext.snapshot_version` (`shamir-tx/src/tx_context.rs:99`) and
   `RepoTxGate::version()` (`shamir-tx/src/repo_tx_gate.rs:229`) are worth
   reading in full as a starting point; there may be an existing MVCC
   watermark or per-table commit counter suited to this. The invariant you
   need: if nothing has committed against this table (or, if only a
   repo-global counter is cheaply available, against the WHOLE repo) since
   this tx's `snapshot_version`, then NO staged row's snapshot value can be
   stale, and the entire per-row loop is provably safe to skip for that
   table.
2. **If a per-table signal exists or can be added cheaply (O(1), one atomic
   load, no new lock), use it** as an early-exit per table inside the loop
   (same shape as `rederive_base_index_ops_post_stage`'s `continue`).
3. **If only a coarser repo-global signal is cheaply available**, that's an
   acceptable, honestly-scoped improvement (it will gate less often — any
   concurrent commit anywhere in the repo defeats the fast path, not just on
   this table — but it's still correct and still a real win for the common
   case of a quiet repo). State this tradeoff explicitly in your report if
   that's the path taken.
4. **If no sufficiently cheap correct signal exists at all**, say so
   plainly, do not force a fix that risks correctness for the sake of
   "closing the task" — a documented, explicit "no cheap gate is safe here,
   here's why" is an acceptable outcome, but only after a real investigation
   (not a first-attempt bailout).
5. Whatever gate you land on, it MUST NOT cause `#1100`'s or `#1106`'s
   regression tests to regress — those tests exercise the exact "something
   DID change concurrently" case this function must still catch. Re-run
   `p1100_stale_snapshot_delete_posting.rs` in full (all 6 tests, including
   the 2 `#1106` added) after your change and confirm all still pass.

## Required benchmark

Add a benchmark (`bench_scale_tool::Harness`, NOT Criterion per `CLAUDE.md`)
demonstrating the gate actually short-circuits the per-row MVCC read +
re-plan work on the COMMON path (no concurrent change to any staged row) —
an A/B comparison similar in spirit to the existing `p1099_touched_probe.rs`
(temporarily revert just your gate and re-run the same bench binary for
genuine "before" numbers, not a synthetic estimate). Report real
before/after numbers.

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench <your new/extended bench>
```

All must pass clean. Report: what signal did you land on (per-table,
repo-global, or "none found, here's why")? Real before/after bench numbers.
Confirm all 6 `p1100_stale_snapshot_delete_posting.rs` tests still pass
(the ones proving the gate does NOT skip genuine staleness).
