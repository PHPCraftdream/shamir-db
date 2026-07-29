# Brief for F-58 (#884, P0) — close the TOCTOU race between the AsOf high-water gate and the index-seek scan

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-4) found a genuine TOCTOU race in the AsOf cursor index-seek
fast path (F-53b). The orchestrator independently traced the exact
mechanism below — read it, don't take it on faith, but the analysis is
correct and the fix approach is deliberately narrow and low-risk.

### The mechanism

`read_as_of` (`crates/shamir-engine/src/table/read_temporal.rs:94-119`)
gates entry to the seek fast path with:

```rust
if self.sorted_indexes().last_mutation_version() <= version {
    if let Some(result) = self.read_as_of_keyset_seek(...).await? { ... }
}
```

This is a **single check performed once, before** dispatching into
`read_as_of_keyset_seek` (`crates/shamir-engine/src/table/read_asof_seek.rs:70-219`),
which then runs a **multi-iteration `loop`** (lines 99-161) — each
iteration does an async `lookup_range_first_k_page` call and a batched
`get_at_many` await, potentially spanning real wall-clock time across
several `.await` points, especially when many candidates need to be
skipped (INSERT-after-pin exclusions) before `limit` genuine matches are
found.

**The gate is checked at entry, never re-checked after the scan
completes.** A concurrent UPDATE (to the indexed field) or DELETE landing
*during* that loop can remove or move a posting that the walk has not yet
reached (or already passed). The existing per-candidate classifier
(`concurrent_modified`, lines 139-152) can only flag a candidate that the
walk **actually observes** with a bumped version — it has no way to
detect a posting that vanished from the observed range entirely. Per the
review: *"UPDATE может удалить old posting и переместить row за текущий
scan range; DELETE полностью удаляет posting; отсутствующий candidate
никогда не попадёт в `version_of`/`get_at`, поэтому `concurrent_modified`
не увеличится; page может тихо пропустить row, который существовал в
pinned snapshot"* — a silent omission with zero defence, not the
already-handled "MODIFY-after-pin, caught by the classifier" case.

### Why the fix is narrow (verified, not assumed)

The orchestrator checked `last_mutation_version`'s actual definition
(`crates/shamir-index/src/legacy/sorted_index_manager.rs:195-213`):

```rust
pub fn last_mutation_version(&self) -> u64 {
    self.last_mutation_version.load(Ordering::Acquire)
}
pub fn note_mutation_at_version(&self, version: u64) {
    self.last_mutation_version.fetch_max(version, Ordering::AcqRel);
}
```

This is a genuinely correct, single-atomic `Acquire`/`AcqRel` pair (unlike
F-56's bug, which was a *cross-atomic* dependency — this is one atomic,
properly paired). `commit_phases.rs:557-564` confirms the bump happens
**after** the posting has actually landed ("at APPLY time ... ensures an
uncommitted tx can never disable the fast path"). So `last_mutation_version`
is a sound, monotonic, correctly-ordered high-water mark — **the bug is
purely the TIME WINDOW the check covers, not a memory-ordering defect in
the counter itself.** This means the fix does NOT need a new epoch/seqlock
primitive: a symmetric **re-check of the SAME predicate after the scan
completes** closes the window, because any mutation that could have
raced the walk necessarily bumps this same counter before-or-during its
own apply, and the post-check's `Acquire` load is guaranteed to observe
it (or a later value) once that mutation's `AcqRel` `fetch_max` has
happened.

## What to do

1. **Add a post-scan re-check in `read_as_of_keyset_seek`.** After the
   `loop { ... }` (line 161) completes and the `concurrent_modified > 0`
   defence-in-depth check (lines 167-169) has already passed, add:
   ```rust
   if self.sorted_indexes().last_mutation_version() > pinned_version {
       return Ok(None);
   }
   ```
   This is the entry gate's exact predicate, re-applied at exit. Place it
   AFTER the existing `concurrent_modified` check (both are defence
   layers; keep them both, in this order — the cheap one first). If this
   fires, `read_as_of` (the caller) already falls through to the existing
   full-scan tail on `Ok(None)` — confirm this fallback wiring still
   works unchanged, do not modify `read_temporal.rs`'s caller logic beyond
   what's needed.
2. **Confirm (do not assume) the ordering argument holds.** Read
   `note_mutation_at_version`'s callers (`on_record_*` non-tx path,
   `apply_index_batch` at commit Phase 5c) to verify the bump genuinely
   happens after the posting write is durably applied and visible to a
   concurrent reader — the brief's proof above depends on this. If you
   find a caller where the bump could race AHEAD of the actual posting
   write (i.e. the bump-then-apply order is reversed anywhere), that is a
   SEPARATE, more serious bug — stop, document exactly what you found,
   and ask the orchestrator before proceeding (do not silently paper over
   a different bug with this task's fix).
3. **Write the missing test the review explicitly calls for**: "park the
   read **after the gate, before/during the index walk**, then
   UPDATE/DELETE" — this is different from the EXISTING negative tests in
   `f53b_asof_seek_tests.rs`, which all mutate BEFORE the gate check (and
   correctly prove the gate declines up front). You need a test that:
   - Sets up enough rows that `read_as_of_keyset_seek`'s `loop` takes
     MULTIPLE iterations (or install a test-only pause hook — check
     whether `read_asof_seek.rs`/`read_temporal.rs` already have a
     pause/resume test seam analogous to `PostBarrierPreWriteHook`; if
     not, and adding one is small, add one gated `#[cfg(test)]` seam that
     parks between two loop iterations, or between the entry gate check
     and the loop's first iteration — mirror the existing
     `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK` / `PostBarrierPreWriteHook`
     conventions used elsewhere in this codebase for exactly this
     "park mid-operation for a deterministic test" need).
   - While parked, performs a concurrent UPDATE (to the indexed field, so
     the posting moves) or DELETE that would be invisible to the
     in-flight walk's already-collected candidates.
   - Releases the pause and asserts: PRE-FIX, the page silently returns
     without the removed/moved row's replacement being caught (or returns
     a page missing a row that existed in the pinned snapshot) — RED.
     POST-FIX, the post-scan re-check detects the high-water advanced and
     returns `Ok(None)`, so `read_as_of` falls back to the full scan for
     that page, which correctly includes the row — GREEN.
4. **Do not weaken the existing gate or classifier.** This is an ADDITIVE
   defence layer, not a replacement for either the entry gate or the
   `concurrent_modified` per-candidate check.

## What NOT to do

- Do NOT introduce a new epoch/seqlock primitive, a lock, or an RCU
  snapshot of the sorted-index root — the existing `last_mutation_version`
  counter (already correctly `Acquire`/`AcqRel`-ordered) is sufficient for
  a symmetric before/after check. Do not over-engineer this.
- Do NOT touch F-55/F-56/F-57/F-59/F-60/F-61 (other tasks from the same
  review) or `PaginationMode::IndexSeek`'s server-side wiring
  (`cursor_handlers.rs`, F-53b Step 4) — this task is scoped to
  `read_as_of_keyset_seek`'s internal safety, not its callers' cursor
  bookkeeping.
- Do NOT change `Temporal::Latest`'s sibling `read_keyset_seek` path
  (`read_index_scan.rs`) — that path has no AsOf pinned-version gate to
  begin with (it serves the current state directly), so this specific
  race class does not apply there. Confirm this understanding before
  touching anything there; if you find it DOES apply, stop and flag it
  rather than silently expanding scope.
- If, per step 2, you discover the bump-vs-apply ordering is NOT what the
  brief claims, do not invent your own fix for that separate problem —
  report it and stop.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: the new test must fail against the pre-fix code and pass after.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

Plus a personal red-then-green reproduction of the new mid-scan race
test.

When done, give your final summary as plain text: the exact diff (file:line),
confirmation of the bump-vs-apply ordering check from step 2, the new
test and how it deterministically forces the mid-scan window, and
confirmation fmt/clippy/tests are clean.
