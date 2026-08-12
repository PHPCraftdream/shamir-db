# #1099 PERF — replace `touched_records_in_tx`'s upfront set build with an on-demand probe

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

⛔ This brief is running in an isolated git worktree, in parallel with
another agent working on task #1102 in a DIFFERENT worktree on the SAME
underlying repo. You cannot see each other's changes until the
orchestrator merges both back to `master` after independent verification.
**Stay strictly inside the files this brief names**:
`crates/shamir-engine/src/table/table_manager_tx_ops.rs` and
`crates/shamir-engine/src/tx/pre_commit.rs`. Task #1102 works entirely
inside `crates/shamir-index/` — if your investigation somehow leads you to
want to touch anything under `crates/shamir-index/`, STOP and report it in
your final summary instead of editing it.

## Background

`update_tx_bytes` (`table_manager_tx_ops.rs`) is called PER ROW on the
wire transactional-UPDATE path (`write_exec.rs`'s matched-row loop) — row
`i`'s call re-walks `tx.index_write_set`/`tx.write_set[table]` from
scratch, an O(N) cost per row, making an N-row transactional UPDATE O(N²)
overall on any table WITH a unique index (the `has_unique_indexes()` gate
added in an earlier fix only helps tables WITHOUT one — the common case
this whole area of code exists FOR is tables that DO have one).

Measured previously (scratch bench, since reverted, by a reviewing
agent): N=400→1600 rows, 68ms→827ms (12.16× — quadratic) with the walks
live, vs. 21ms→88ms (4.19×, near-linear) with both walks stubbed out.

**This brief scopes ONLY the "touched" half of the fix** — the
`released` half (`released_unique_keys_in_tx`) needs a bigger structural
change (incremental `TxContext` state) that is explicitly OUT OF SCOPE
here; do not attempt it. Read `crates/shamir-engine/src/table/table_manager_tx_ops.rs`'s
`touched_records_in_tx` function and its call sites (`insert_tx`,
`insert_tx_many`, `insert_tx_many_bytes`, `update_tx`, both branches of
`update_tx_bytes`) yourself before starting — do not work from this
summary alone.

## The fix

`crates/shamir-engine/src/tx/pre_commit.rs`'s Step 2 (the commit-time
durable check) ALREADY answers the identical question —
"is this durable owner a record THIS tx itself touched?" — with an O(1)
ON-DEMAND probe, done ONLY at the point an actual durable conflict is
found (search for `staged_op` in that file — the exact line reads
something like:
```rust
tx.write_set
    .get(&g.table_token)
    .is_some_and(|s| s.staged_op(existing_id.as_bytes()).is_some())
```
), rather than materializing a whole `TFxSet<[u8;16]>` of every touched
record BEFORE checking anything. `touched_records_in_tx` should be
replaced by the SAME on-demand pattern: only call
`tx.write_set.get(&table_token)?.staged_op(candidate_id.as_bytes())` at
the exact point a caller has ALREADY found a conflicting durable owner
(via `check_unique_key`/`info_store.get` returning `Some`), which is rare
per-row — not build a full set of every touched record before any check
even runs.

Concretely: `touched_records_in_tx(tx, table_token) ->
TFxSet<[u8; 16]>` should be replaced by a narrower helper (or inlined at
each call site) with a signature closer to `fn is_record_touched_in_tx(tx:
&TxContext, table_token: u64, candidate_id: &RecordId) -> bool`, calling
`tx.write_set.get(&table_token).is_some_and(|s|
s.staged_op(candidate_id.as_bytes()).is_some())` directly — an O(1) probe,
not an O(N) set build. Every current call site of `touched_records_in_tx`
passes its result into `validate_unique_for_create_with_released`/
`validate_unique_for_update_with_released` (in
`crates/shamir-index/src/base_index/index_manager_unique.rs`) as a
`&TFxSet<[u8;16]>` parameter — **you will need to change that function
signature too**, from accepting a pre-built set to accepting something
that can answer "is this specific candidate id touched?" on demand (e.g.
change the parameter to a closure `impl Fn(&RecordId) -> bool`, or thread
`tx`/`table_token` through directly so the validator can call the O(1)
probe itself at the exact point it discovers a durable conflict — mirror
whichever shape keeps `shamir-index` from needing to depend on
`shamir-tx`'s `TxContext` type directly if it doesn't already; check the
existing dependency direction between these two crates before choosing,
since `shamir-engine` depends on both `shamir-tx` and `shamir-index` but
they may not depend on each other).

**Do NOT touch `released_unique_keys_in_tx`** — its O(N)-per-call shape
is a separate, larger fix (incremental `TxContext` state) explicitly
deferred to its own task. Leave it exactly as-is; only replace the
`touched_records_in_tx` half.

## Benchmark

Write a NEW bench (or adapt a scratch reproduction into a real one) using
`bench_scale_tool::Harness` (NOT Criterion — see this repo's `CLAUDE.md`),
in `crates/shamir-engine/benches/`, mirroring an existing bench file's
structure in that directory (check what's already there and copy the
established shape). Reproduce the O(N²) shape FIRST — confirm you can see
superlinear scaling (N=400→1600 rows, transactional UPDATE on a table
WITH a unique index, measuring wall time) — THEN apply the fix and
confirm the SAME bench shows near-linear scaling afterward. Report both
sets of numbers in your final summary. This is the same "measure before
and after" discipline `#1092`'s benchmark work in this same session used
— do not skip the "before" measurement even though you know the fix is
coming; it is the proof the fix actually helped, not just a plausible
guess.

## Tests

The existing unique-index tx test suites
(`crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check.rs`,
`p1097_remove_posting_owner.rs`) already exercise the `touched`
semantics functionally — run them to confirm your refactor preserves
identical BEHAVIOR (not just wins on perf). Add a new regression test if
you find any behavioral edge case the on-demand-probe refactor could get
wrong that isn't already covered (e.g. a record touched via `Remove` not
just `Set` — `staged_op` should already cover both, per its existing
doc, but verify this explicitly with a test rather than assuming).

## Gate

```
cargo fmt -p shamir-engine -p shamir-index -- --check
cargo clippy -p shamir-engine -p shamir-index --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index --full
```
All three must pass clean, PLUS the new/adapted bench must show the
claimed near-linear improvement — include the actual numbers in your
final summary, not just "it's faster now".
