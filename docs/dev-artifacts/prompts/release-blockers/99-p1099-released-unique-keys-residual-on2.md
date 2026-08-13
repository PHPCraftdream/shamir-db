# #1108 MEDIUM — #1099 only fixed half its own O(N^2): released_unique_keys_in_tx is still a per-row full walk

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a final adversarial review of the whole session's accumulated work
(commit range `b556913f..HEAD`). Code-derived, confirmed by direct reading
before this brief was written.

`#1099`'s own task text described the problem as "released/touched walks
recomputed per row" (plural — two walks). Only `touched_records_in_tx` was
replaced with an O(1) probe (an inline `is_record_touched` closure doing
`tx.write_set.get(&table_token).is_some_and(|s| s.staged_op(rid.as_bytes())
.is_some())`). `released_unique_keys_in_tx`
(`crates/shamir-engine/src/table/table_manager_tx_ops.rs:88`) is still a
FULL walk of `tx.index_write_set` — confirmed by direct reading, it iterates
`&tx.index_write_set` unconditionally to build `live`/`released` maps from
scratch, every time it's called.

It's called per row from `update_tx_bytes` at two branches (`:1247` map-lens,
`:1316` tree-fallback), both gated only by `self.index_manager
.has_unique_indexes()` (the gate `#1099` already added for the OTHER walk) —
NOT by anything that would skip it when `tx.index_write_set` is non-empty.
`update_tx_bytes` itself is called PER ROW on the wire transactional-UPDATE
path (`write_exec.rs`'s row loop).

Since `tx.index_write_set` grows by at least 2 ops per row whenever the
unique column actually changes value (a `RemovePosting` for the old value +
a `SetPosting` for the new one, per row), the running sum of these per-row
full walks is still `Theta(N^2)` for the MOST NATURAL workload for a table
with a unique index — an UPDATE batch that changes the unique column on
every row. `#1099` fixed the OTHER O(N) walk (`touched_records_in_tx`) for
this exact workload but left this one in place.

## Why the existing bench doesn't show it

`crates/shamir-engine/benches/p1099_touched_probe.rs` is honest in its own
doc, but its workload happens to leave `index_write_set` empty across rows
(the table has only a unique index, and the UPDATE leaves the unique column
unchanged) — so `released_unique_keys_in_tx` is called on an empty/near-empty
set every row (O(0)-ish) and the bench cannot observe the residual walk at
all. Its doc comment currently understates the residual's scope too
("workload that DOES release-and-reclaim on every row") — the residual
actually reproduces for ANY workload where `index_write_set` accumulates ops
across rows in the same tx (regular, sorted, or FTS index ops staged
alongside the unique ones all count — not just unique release-and-reclaim
specifically), since the walk is unconditional over the WHOLE `index_write_set`
regardless of which family the ops belong to.

## Fix

Apply the same O(1)-probe technique `#1099` already used for
`touched_records_in_tx` to `released_unique_keys_in_tx` — replace the
full-walk-per-call with an incrementally-maintained piece of `TxContext`
state (a set/counter updated at each `index_write_set` mutation site, not
recomputed from scratch on every read), OR another O(1)/O(log N) technique
that avoids re-walking the whole write set per row. Read
`released_unique_keys_in_tx`'s current doc comment in full first — it
explicitly documents why it must track `live`/`released` in the SAME way
`pre_commit.rs`'s Step 1 does (owner-aware `RemovePosting` handling, per a
prior `#1097` follow-up fix) — any replacement must preserve that exact
semantics, not just the O(1) shape.

If a genuinely incremental `TxContext`-level redesign is too large for this
task's scope, that's an acceptable outcome IF you say so explicitly and
propose the smallest correct step toward it (e.g., caching the walk's
result and invalidating only on the next `index_write_set` mutation, so
repeated calls within the same "no new ops since last call" window are O(1)
even if a single walk is still O(N) when the set does grow) — do not ship a
silently-still-O(N²) "fix" without flagging the residual gap explicitly in
your final report.

## Required bench

Extend or add a bench (`bench_scale_tool::Harness`, NOT Criterion per
`CLAUDE.md`) that DOES exercise a workload where `tx.index_write_set` grows
across rows in the same transaction — e.g. a transactional UPDATE batch that
changes the unique-indexed column's value on EVERY row (the natural
worst-case for this function). Demonstrate the O(N^2) behavior BEFORE your
fix (temporarily revert just your fix and re-run the same bench binary for
genuine "before" numbers, the same technique `#1099`'s own bench used) and
the improved complexity AFTER. Also correct
`p1099_touched_probe.rs`'s doc comment to accurately describe what it does
and does NOT cover (it should no longer claim to be exhaustive over "the
released/touched walks" if only one of the two is fixed, or should be
updated once both are fixed here).

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench p1099_touched_probe
```

All must pass clean. Report real before/after bench numbers for the new
per-row-growth workload, and real gate pass/fail counts.
