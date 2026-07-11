Task #539 — replace `MemBufferStore`'s boolean `dirty_nonempty: AtomicBool`
sentinel with an `AtomicUsize` cardinality mirror, closing the transient
reader-visible false-negative window (and its tombstone-poisoning
consequence) that #535's two rounds of check-then-clear patches narrowed
but structurally cannot close.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Read first — source of truth

`crates/shamir-storage/src/storage_membuffer.rs`'s `drain_once` has a
"Residual" comment block (search for "THIRD `@fl` adversarial pass" or
similar wording) documenting exactly what #535's rounds 1+2 closed and what
they left open. Read it before starting — do not re-derive from scratch.
Task #539 in the TaskList (`TaskGet 539`) has the full narrative if the
comment has drifted.

## What #535 already closed (do not re-litigate — fixed, tested, verified)

Round 1: `drain_once`'s clear sequence is check-then-store(false)-then-
re-check-and-restore-if-non-empty.

Round 2: every writer (`insert`/`set`/`remove`/`insert_many`/`set_many`/
`remove_many`) republishes `dirty_nonempty.store(true, Release)`
immediately after its `dirty.insert()`, not just before.

Both are regression-tested: `crates/shamir-storage/src/tests/
storage_membuffer_tests.rs::clear_race_535` and
`::batch_insert_republish_535`.

## The remaining gap this task must close

A boolean flag with a separate check-then-store sequence can never be
linearizable with the thing it's tracking — each additional "verify again"
round only narrows the transient window, it cannot close it. Concrete
interleaving requiring genuine OS-level thread preemption (not just an
async yield):

1. Drainer: `is_empty()` check → `true`.
2. **Preempted.**
3. Writer (another thread): `dirty.insert(K)`; round-2 republish
   `store(true)`; `cache.insert(K, Live)`; returns — K is a fully ACKed
   write.
4. moka evicts K from cache (dirty is now the ONLY place K lives).
5. Drainer resumes: `store(false)`. **Preempted again**, between this
   store and the re-check just below it.
6. A concurrent reader: `get(K)` → cache miss → `dirty_nonempty`
   Acquire-load sees `false` → skips the dirty probe → falls through to
   `inner` → `NotFound` for a fully-ACKed write. Worse: `get()`'s
   NotFound path caches a `Slot::Tombstone` for K, masking K on every
   SUBSEQUENT `get(K)` via the cache-HIT path (ahead of the
   `dirty_nonempty` check entirely) until that tombstone entry itself is
   evicted or overwritten — outliving the sentinel's own self-healing
   re-check.

## The fix (CLAUDE.md-aligned — this is the prescribed O(1)-cardinality
## pattern used elsewhere in this codebase)

Replace `dirty_nonempty: AtomicBool` with an `AtomicUsize` cardinality
mirror (same pattern as `Drainer::window_depth`, `VersionedOverlay::count`
— grep both for the idiom used in this codebase):

- **Increment** the counter when `dirty.insert()` returns `None` (the key
  was genuinely NEW to `dirty`, not an overwrite) — for `insert`/
  `insert_many` this is always new (fresh `RecordId`); for `set`/
  `set_many`/`remove`/`remove_many` check the `DashMap::insert` return
  value and only increment on `None`.
- **Decrement** the counter on every SUCCESSFUL `remove_if` — in
  `drain_once` AND in `transact`'s dirty-cleanup, if that path also
  removes entries (grep `dirty.remove` / `dirty.remove_if` across the
  file to find every removal site, do not rely on this brief's list being
  exhaustive).
- **Readers** gate on `counter.load(Acquire) > 0` instead of a boolean.

This eliminates the check-then-clear shape entirely: the counter can
never be observably zero while an entry it accounted for still exists,
because the decrement happens AT THE SAME LOGICAL POINT the entry is
actually removed (inside the same `remove_if` success branch), not as a
separate later "is it empty now" re-derivation. No verify-after-clear
loop is needed — the multi-paragraph rounds-1+2 comment block in
`drain_once` collapses into a much simpler invariant. Update/replace that
comment block to describe the new invariant instead of the old
check-then-clear reasoning (do not leave stale reasoning describing a
mechanism that no longer exists).

**Verify no double-count / under-count**: audit every `dirty.insert()`
call site in the file (not just the ones named above) so the increment
condition (`insert()` returning `None`) is applied uniformly, and every
removal site so the decrement is applied uniformly. An off-by-one here
(e.g. decrementing on a `remove_if` that didn't actually find anything,
or incrementing on an overwrite) reintroduces exactly the kind of
drift-from-reality bug this redesign exists to eliminate structurally —
trace it explicitly, don't assume.

## Separately: the tombstone-poisoning half (investigate, decide, document)

Consider whether `get()`'s NotFound → `Slot::Tombstone` cache-fill needs
its own guard independent of this fix (e.g. only cache a negative result
if a re-check confirms the key is still absent from `dirty` at
cache-fill time, or don't cache negatives for keys that were ever dirty
within some recency window). This codebase's cache is explicitly "purely
a read accelerator" (see `build_cache`'s doc comment) so some staleness
may already be an accepted trade-off — if, after investigating, you
conclude it's an accepted trade-off, say so explicitly in your report and
leave a doc comment recording that decision. If you find a cheap,
low-risk guard that closes it without touching the cache's broader
contract, implement it. Do NOT force a large cache-invalidation redesign
to chase this — the counter fix above is the actually-required scope of
this task; the tombstone half is investigate-and-decide, not
mandatory-implement.

## Test requirement

A dedicated deterministic regression test for the narrow original
residual (flag-false-while-entry-present window) is expected to be very
hard or impossible with current seams (would need a hook between
`store(false)` and the re-check, plus a racing reader) — a bonus, not a
hard requirement. What IS required:

- Port the existing `clear_race_535` and `batch_insert_republish_535`
  tests to the new counter-based implementation — they test end-to-end
  visibility, not the internal flag mechanism directly, so they should
  need minimal changes. Confirm both still pass GREEN against the new
  implementation.
- If you implement a tombstone-poisoning guard, add a regression test
  proving it (confirm RED without the guard, GREEN with it).

## Test scope

```
./scripts/test.sh -p shamir-storage
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-storage
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Counter redesign: exact increment/decrement sites, confirmed no
    double-count/under-count across every dirty.insert/remove site
  > Ported clear_race_535 / batch_insert_republish_535: pass/fail
  > Tombstone-poisoning half: guard implemented / investigated-and-
    accepted-as-documented-tradeoff (state which, and why)
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-storage: pass/fail
```

Given this touches the same correctness-critical concurrency surface as
#535 (three adversarial passes already applied to this file this
campaign), this MUST go through an adversarial review pass before
committing. If that review finds a genuine bug, the orchestrator fixes
it directly (never re-delegates), re-verifies, and sends the fix through
a second review pass before committing.
