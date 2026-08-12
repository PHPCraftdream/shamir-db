# #1098 HIGH — unique-guard/generation read-order race can silently skip the durable unique check for a mid-tx-created unique index

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — confirmed by the orchestrator's own investigation before writing this brief

`crates/shamir-index/src/base_index/index_manager_unique.rs`'s
`create_unique_index_from_records` (the `CREATE UNIQUE INDEX` publish path)
runs, in this exact order (lines 646-651):
```rust
self.indexes_unique.add_index(index_def);
self.bump_generation();                          // P0-2 (#958): gen gate
self.write_barrier_flags.set(UNIQUE_INDEX_EXISTS); // F-69 (#896)
```
`bump_generation()` (`index_manager.rs:1350-1352`, `Ordering::Acquire`/
`Release` on `self.generation: AtomicU64`) and `write_barrier_flags.set(...)`
(a SEPARATE atomic word, `index_manager.rs:1341-1342`'s
`has_unique_indexes()` reads `write_barrier_flags.is_set(UNIQUE_INDEX_EXISTS)`)
are TWO INDEPENDENT atomics with no combined ordering guarantee between
them — a reader can observe the NEW generation while still observing the
OLD (unset) flag, because the writer publishes gen first and the flag second.

`crates/shamir-engine/src/table/table_manager_tx_ops.rs`'s
`update_tx_bytes` (the ONE call site with the CORRECT order) captures
`base_index_gen = self.index_manager.generation()` **BEFORE** its
`has_unique_indexes()`-gated unique validation / `UniqueGuard` recording
(see lines ~1153-1197: gen capture at 1156-1158, THEN the `if
self.index_manager.has_unique_indexes() { ... }` block at 1180). This is
the ordering `P0-2 (#958)`'s own doc comment requires: capturing gen
FIRST means that if the LATER flag/def read observes the writer's
already-published state, the captured `stage_gen` is provably from
BEFORE that publish — so `stage_gen < mgr.generation()` at commit time
correctly triggers `pre_commit.rs`'s `rederive_base_index_ops_post_stage`
re-derivation (which ALSO records fresh `UniqueGuard`s for defs that
didn't exist at stage time, per that function's own doc).

**Four other call sites have the ordering BACKWARDS** — confirmed by
direct reading, not inference:

1. `insert_tx` (`table_manager_tx_ops.rs:496-599`): `has_unique_indexes()`
   check + `validate_unique_for_create_with_released` at lines 528-534,
   `unique_keys_for`/`record_unique_guard` loop at 540-546, THEN gen
   capture (`index2_gen`/`sorted_gen`/`base_index_gen`) at lines 565-568.
2. `update_tx` (`table_manager_tx_ops.rs:1007-1130`): `has_unique_indexes()`
   check + validation at lines 1043-1060, `unique_keys_for`/
   `record_unique_guard` loop at 1066-1072, THEN gen capture at
   lines 1080-1083.
3. `insert_tx_many` (`table_manager_tx_ops.rs:625-804`): batch validation
   at lines 641-663, `UniqueGuard` recording loop at 693-704, THEN gen
   capture at lines 713-716.
4. `insert_tx_many_bytes` (`table_manager_tx_ops.rs:824-992`): batch
   validation at lines 851-874, `UniqueGuard` recording loop at 897-909,
   THEN gen capture at lines 917-920.

### Why the backwards order is a real bug (traced end to end, not hypothetical)

Race window: a `CREATE UNIQUE INDEX` is between its `bump_generation()`
and `write_barrier_flags.set(UNIQUE_INDEX_EXISTS)` calls, concurrently
with one of the four call sites above staging an `INSERT`/`UPDATE`.

1. The reader's `has_unique_indexes()` check (and the `unique_keys_for`
   call inside the guard-recording loop, which internally re-checks the
   SAME flag — see `index_manager_unique.rs:260-263`) lands BEFORE the
   writer's flag-set — sees `false` — so **zero validation runs and zero
   `UniqueGuard` is recorded** for the new index.
2. The reader's LATER gen capture lands AFTER the writer's (earlier)
   `bump_generation()` — captures the ALREADY-BUMPED value as `stage_gen`.
3. At commit, `pre_commit.rs`'s gate `mgr.generation() == stage_gen`
   (see `rederive_base_index_ops_post_stage`, ~line 1493) finds them
   EQUAL (nothing advanced since this tx's own stage_gen, because that
   capture already included the bump) — **re-derivation is skipped
   entirely**, so no fresh `UniqueGuard` is recorded even at commit time
   either.
4. `pre_commit.rs`'s Step 2 (the durable check) only iterates
   `tx.unique_guards` — with none recorded for this key, **it is never
   durable-checked**.
5. But the reader's OWN index-op planning (`plan_base_index_insert_ops`
   et al., which runs even later, after the writer has very likely
   finished publishing by then) DOES see the new unique index (the def
   is now visible in `indexes_unique.iter()`) and emits a `SetPosting`
   for it — which Phase 5c writes unconditionally, **overwriting
   whatever posting is already there with zero prior validation**.

Net effect: a genuinely racing `CREATE UNIQUE INDEX` (concurrent with an
in-flight `INSERT`/`UPDATE` tx) can silently admit a duplicate under the
brand-new unique index — the exact class of corruption `P0-2 (#958)`
was written to close, reopened at 4 of 5 call sites by an inverted read
order at each.

Pre-existing since `P0-2 (#958)` (NOT introduced or worsened by `#1096`/
`#1097`'s work) — out of both of those tasks' scope.

## The fix

At each of the 4 backwards call sites, move the generation-capture block
(`let token = self.table_token(); let index2_gen = ...; let sorted_gen =
...; let base_index_gen = self.index_manager.generation();`) to run
**BEFORE** the `has_unique_indexes()`-gated validation/guard-recording
block — matching `update_tx_bytes`'s already-correct structure exactly.
This is a **pure reordering** — do not change what either block DOES,
only WHEN it runs relative to the other. Do not touch `update_tx_bytes`
itself (already correct) or any other function.

Concretely, for each of the 4 sites:

### `insert_tx` (~line 496)

Current order: pessimistic lock → `has_unique_indexes()` validation block
→ `unique_keys_for`/guard-recording loop → `bytes = value.to_bytes()` →
gen capture → index planning.

New order: pessimistic lock → **gen capture** → `has_unique_indexes()`
validation block → `unique_keys_for`/guard-recording loop → `bytes =
value.to_bytes()` → index planning. (The `token`/`index2_gen`/
`sorted_gen`/`base_index_gen` `let` bindings simply move up above the
`if self.index_manager.has_unique_indexes() { ... }` block at line 528;
everything else keeps its relative order. `let token = self.table_token();`
already appears once later at line 565 in the original — after the move
there is only ONE `token` binding, used both by the moved gen-capture
block's needs and by the later `tx.note_*_stage_gen(token, ...)` calls at
the bottom of the function; do not create a duplicate binding.)

### `update_tx` (~line 1007)

Current order: `read_one_tx` → pessimistic lock → `has_unique_indexes()`
validation block → `unique_keys_for`/guard-recording loop → `bytes =
value.to_bytes()` → gen capture → index planning.

New order: `read_one_tx` → pessimistic lock → **gen capture** →
`has_unique_indexes()` validation block → `unique_keys_for`/
guard-recording loop → `bytes = value.to_bytes()` → index planning.

### `insert_tx_many` (~line 625)

Current order: batch validation block (has_unique_indexes-gated) → id
generation → pessimistic locks → `UniqueGuard` recording loop
(has_unique_indexes-gated) → gen capture → index2/base_index planning.

New order: **gen capture** → batch validation block → id generation →
pessimistic locks → `UniqueGuard` recording loop → index2/base_index
planning. (Gen capture no longer needs anything computed by the
validation/id-generation steps, so moving it all the way to the top,
right after the empty-input early return, is correct and simplest — but
placing it anywhere before the FIRST `has_unique_indexes()` check at
line 641 satisfies the fix; prefer moving it to immediately after the
`if values.is_empty() { return Ok(Vec::new()); }` guard for clarity,
matching how early `update_tx_bytes` captures it relative to its own
first unique-related read.)

### `insert_tx_many_bytes` (~line 824)

Same shape as `insert_tx_many`: move the gen-capture block from its
current position (~line 917-920) to before the first
`has_unique_indexes()` check (~line 851) — placing it right after the
`views` construction (~line 840) is the natural spot, matching
`insert_tx_many`'s fix.

## Why this closes the race

With gen captured FIRST at every site: if the writer's publish sequence
overlaps this reader at all, EITHER (a) gen is captured before
`bump_generation()` runs — the reader's `stage_gen` is strictly older
than the eventual `mgr.generation()`, so the commit-time gate correctly
detects "something changed since stage" and re-derives (recording a
fresh guard, closing the gap) — OR (b) gen is captured after
`bump_generation()` AND the reader's LATER flag/def reads also see the
fully-published state (both gen bump and flag set already happened) — in
which case the reader correctly sees and validates against the new
index from the start, consistent behavior, no gap. The backwards order
was the only way to get gen captured post-bump while flag/def reads still
see pre-flag-set — this fix removes that possibility structurally.

## Tests to add

New file `crates/shamir-engine/src/table/tests/p1098_gen_read_order_tests.rs`
(or `crates/shamir-engine/src/tx/tests/` if that fits this repo's existing
convention better for tx-race tests — check how `f69_write_barrier_single_atomic_tests.rs`
and `f70_lock_order_inversion_tests.rs` are organized and match that), wired
into the parent `mod.rs` per this repo's test-organization convention.

The race itself is timing-dependent and hard to hit via real concurrency
in a deterministic test. Prefer a **deterministic reproduction using a
test-only pause seam**, mirroring this codebase's established pattern
(see `pre_commit.rs`'s `TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK` /
`fire_post_prelock_pre_materialize_test_hook` for the exact shape: a
`#[cfg(test)]` `OnceLock<Arc<Hook>>`, zero-cost when unset, that parks a
task at a specific point so a test can interleave a concurrent write
deterministically) OR, if simpler and equally convincing, a direct
**unit-level test of the read-ordering invariant itself**: stage an
`insert_tx`/`update_tx`/`insert_tx_many`/`insert_tx_many_bytes` call,
verify (via whatever internal introspection is reasonable, e.g. checking
`tx.base_index_stage_gens` after the call vs. the manager's live
`generation()`) that the captured stage_gen reflects the state as of
BEFORE the validation/guard step ran, not after.

If a pause-seam approach is used, the minimal one-thread-at-a-time
reproduction: 1) begin a tx, 2) drive `insert_tx` (or one of the other 3
sites) via a controllable pause point positioned between where the OLD
code's flag-read happened and where gen is now captured (post-fix, this
window is BEFORE both reads, so the seam should sit right after the gen
capture and before the validation block) 3) from the test thread (not
inside the paused tx), perform a real `CREATE UNIQUE INDEX ... FROM
RECORDS` that lands entirely within that pause window (bump_generation +
flag set both complete) 4) resume the paused tx, let it finish staging
and commit 5) assert the commit is EITHER rejected (if it tried to
insert a value colliding with something already indexed under the new
unique index) OR, for the "no collision" case, assert a `UniqueGuard`
WAS recorded / the value IS validated — proving the tx did NOT blindly
skip the new constraint. Write at least one test per affected call site
(4 tests) exercising a genuine collision that MUST be caught, matching
this task's own failure-scenario description above. If the pause-seam
plumbing is more work than a single session should absorb for all 4
sites, prioritize `insert_tx` and `insert_tx_many_bytes` (the latter is
what `execute_insert_tx`/`execute_set_tx` actually call on the wire —
see other comments in this file emphasizing that path's real-world
weight) and clearly note in the PR/commit which sites got a full
concurrency-level test vs. a lighter unit-level ordering check.

Run mutation testing on your own fix before considering it done: revert
one call site's reordering back to the original (broken) order, confirm
the corresponding new test fails, restore it, confirm it passes again.

## Gate

Before finishing:
```
cargo fmt -p shamir-index -p shamir-engine -- --check
cargo clippy -p shamir-index -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-index -p shamir-engine --full
```
All three must pass clean.

Do not touch anything not described above — this is a pure read-ordering
fix at 4 specific call sites plus new tests. No incidental refactors, no
touching `update_tx_bytes` (already correct), no changes to
`create_unique_index_from_records`'s publish order (that order is
correct — generation-then-flag is the intended contract, per its own
inline comments; the bug is entirely on the READ side).
