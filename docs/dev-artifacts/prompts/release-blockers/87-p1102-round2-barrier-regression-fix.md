# #1102 round 2 — undo the write-barrier-predicate regression the round-1 fix introduced

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`#1102` round 1 (commits `ea54fbad`, `fed51288`, `6cb066fb`, already on
`master`) fixed a real ordering race in the regular (non-unique) hash
index's `has_indexes` flag by folding it into `write_barrier_flags.rs`'s
packed `Arc<AtomicU16>` word as a new `REGULAR_INDEX_EXISTS` bit (bit 6),
and reordering the 4 CREATE-index sites in
`crates/shamir-index/src/base_index/index_manager.rs` to set that flag
before calling `self.bump_generation()`.

An adversarial `@oh` review of that fix (already run, findings below) found
the packed-word approach itself introduced a **real HIGH-severity
regression**, and that the underlying memory-model reasoning for even
needing the fold was wrong. Read the full round-1 diff yourself first —
`git show ea54fbad` and `git log --oneline -5` — before starting.

## Finding 1 (HIGH, must fix): the fold silently disables the lock-free write fast path for any table with a regular index

`write_barrier_flags.rs`'s `any_set()` — `self.bits.load(SeqCst) != 0` — IS
the write-barrier predicate:
`crates/shamir-engine/src/table/table_manager.rs:1393`'s
`needs_write_barrier()` is exactly `self.write_barrier_flags.any_set()`.
`TableManager` and `IndexManager` share the SAME `Arc<AtomicU16>`.

`REGULAR_INDEX_EXISTS` is a **steady-state existence** flag — set the
moment ANY regular index exists on the table, cleared only when the LAST
one is dropped. Folding it into the barrier word means `any_set()` is now
permanently `true` for the (very common) case of "table has at least one
regular index" — even with zero DDL in flight and zero unique indexes.
That silently forces every single-row write
(`table_manager_crud.rs`'s `insert_returning_version`,
`insert_many_returning_version`, `delete_returning_version`, `set`) and
every tx commit touching such a table
(`pre_commit.rs`'s Phase 2.5 `unique_tokens`/`unique_write_lock` path) onto
the `unique_write_lock`-serialized slow path — contradicting
`needs_write_barrier`'s own documented contract
(`table_manager.rs:1233-1394`), which enumerates exactly five DDL-in-flight
/ unique-index conditions and explicitly says a table with none of them
"keeps the lock-free fast path." A regular (non-unique) hash index
existing was never meant to be one of those five conditions — only its
CREATE-in-flight window (`REGULAR_INDEX_CREATE`, bit 3) was.

**This needs undoing.** Two options — pick whichever you judge cleaner
after reading the code, but you MUST NOT leave `any_set()` (and therefore
`needs_write_barrier()`) permanently `true` for a table with a regular
index:

- **(a) Mask it out of the predicate.** Add
  `pub const BARRIER_BITS: u16 = ALL_BITS & !(REGULAR_INDEX_EXISTS as u16);`
  (non-`cfg(test)` — production code needs it) and change `any_set()` to
  `(self.bits.load(Ordering::SeqCst) & BARRIER_BITS) != 0`. Keeps the
  packed word, keeps `REGULAR_INDEX_EXISTS`'s `SeqCst` ordering, smallest
  diff.
- **(b) Revert the fold entirely.** Restore `has_indexes` as a standalone
  atomic (see Finding 2 below for what ordering it actually needs — you do
  NOT need to re-widen anything or touch `write_barrier_flags.rs` at all
  under this option), keep only the writer-order reorder at the 4 CREATE
  sites (and, per your own judgment per round 1's already-settled Finding
  3 below, the DROP site's analogous reorder — already correct, leave it).

Whichever you pick, add a regression test proving `needs_write_barrier()`
(or `TableManager`'s public equivalent — check what's actually reachable
in a test) stays `false` on a table that has ONLY a regular index and zero
other barrier conditions, immediately after `CREATE INDEX` completes. This
exact gap is why round 1 shipped the regression undetected — every
existing `assert!(!tbl.needs_write_barrier())` in the test suite happened
to run on a table with no regular index yet.

## Finding 2 (informational — explains why option (b) doesn't need SeqCst)

The `@oh` review re-derived the happens-before argument and found the
original brief's claim — "correcting the writer's publish order ALONE is
insufficient, the reader also needs a stronger ordering than `Relaxed`" —
was actually WRONG. Writer: `A: flag.store(_, Release)` sequenced-before
`B: generation.fetch_add(1, AcqRel)`. Reader: `C: generation.load(Acquire)`
sequenced-before `D: has_indexes()`. If `C` reads-from `B`, `B`
synchronizes-with `C`; `A` sequenced-before `B` makes `A` happens-before
`C`, and `C` sequenced-before `D` on the same thread makes `A`
happens-before `D` — so `D` is guaranteed to observe `A`'s write **even as
a plain `Relaxed` load**, purely via the generation's own release/acquire
pair. The `SeqCst` upgrade wasn't the fix; the writer-order swap was.

This matters for whichever option you pick: under (a) the `SeqCst` cost
stays (harmless, already paid); under (b) you can keep `has_indexes`
`Release`-written / `Relaxed`-read exactly as before round 1 and STILL be
correct by this argument, but that correctness depends on every caller of
`has_indexes()` being reached only after a generation-`Acquire` capture on
the same thread — a caller-discipline invariant that's easy to violate
later without anyone noticing. **Prefer upgrading the read side to
`Acquire`** (matching the round-1 brief's own documented fallback option)
so the guarantee holds unconditionally, independent of caller discipline —
write out the happens-before argument as an inline comment either way,
this codebase's established convention for any newly-touched ordering.

## Finding 3 (already resolved, no action needed): the DROP-site reorder is safe

Round 1 also reordered the mirror-image DROP INDEX site
(`remove_index`) to set the flag before `bump_generation()`, which the
original brief had scoped as "leave alone unless you find a concrete
reason it's unsafe." The `@oh` review did a full adversarial trace and
concluded it introduces no new hazard and is a net improvement (closes an
analogous stale-read gap on the DROP side). **Leave this exactly as
merged — do not revert it.**

## Findings 6-10 (LOW/NIT — fix all of these too, they're all in the file(s) you're already touching)

- `write_barrier_flags.rs`'s module doc still says "six gate bits" /
  "the whole byte" / "five-sixths" in several places (lines ~27-28, 62-63,
  81, 94, 162, 255) — stale after round 1's edit added a seventh bit
  (and, if you keep option (a), still needs updating to explain
  `BARRIER_BITS` vs the full word). Also the round-1 `AtomicU16` widening
  was unnecessary (7 bits fit in a `u8` with room to spare; all bit
  constants are still declared `u8`, so the second byte was never
  reachable) — if you take option (b) this is moot (you're reverting the
  widening anyway); if you take option (a), either shrink back to
  `AtomicU8`/keep the packed byte at 1 byte, or fix the "to accommodate
  the seventh bit" doc claim to not be false.
- `index_manager.rs:1410`ish — `write_barrier_flags()`'s doc still says
  "the SAME `Arc<AtomicU8>`" (stale if you kept the `AtomicU16` widening
  under option (a); moot under option (b)).
- `table_manager.rs`'s `write_barrier_flags` field doc and
  `needs_write_barrier`'s doc (~lines 52-110, 1233-1394) still enumerate
  exactly six bits and never mention `REGULAR_INDEX_EXISTS` — needs a
  correct update either way (under (a): explain it's excluded from
  `BARRIER_BITS`; under (b): no change needed, it's gone).
- `write_barrier_flags.rs`'s `WriteBarrierFlags::with_unique_index_exists`
  constructor is now production-dead (only `IndexManager::new` used it,
  and round 1 switched that call site to
  `with_regular_and_unique_index_exists`) — delete it and its test, or
  fold option (a)'s survivor into a single clean constructor. Under option
  (b) this whole question is moot (you're reverting to the pre-round-1
  shape).
- `index_manager.rs`'s new `post_flag_set_pre_gen_bump_hook` field doc
  claims "NOT `#[cfg(test)]`-gated — cross-crate test consumer" — false,
  its only consumer is the in-crate `p1102_regular_index_gen_order_tests.rs`.
  Either gate it `#[cfg(test)]` (matching the `#1098` sibling hook in
  `index_manager_unique.rs` it claims to mirror) or fix the doc to state
  the real reason if you deliberately want it ungated.
- Comment typo at `index_manager.rs` (search `REGULAR_INDEX EXISTS` —
  missing underscore) in one of the 4 CREATE-site comments.
- `index_manager.rs`'s `recover_in_progress_drops` (search for it) still
  does `bump_generation()` THEN the flag `set_to` — the one remaining site
  with the pre-round-1 shape. It's genuinely exempt (open-time,
  single-threaded, before the manager is published/shared) — add a
  one-line comment saying so, don't reorder it blind just to be
  "consistent" with the other sites.

## Tests

Keep the existing `p1102_regular_index_gen_order_tests.rs` pause-seam test
(it's sound, per the `@oh` review — proves the writer-order half). Add:

1. The `needs_write_barrier()`-stays-false regression test from Finding 1.
2. If you choose option (a) (mask), a test that `REGULAR_INDEX_EXISTS`
   still correctly participates in `has_indexes()`/`is_set()` while being
   excluded from `any_set()`'s effective predicate — i.e. `any_set()`
   with ONLY that bit set is `false`, but `is_set(REGULAR_INDEX_EXISTS)`
   is still `true`.

Mutation-test whichever fix you land (temporarily revert, confirm the new
regression test fails, restore, confirm it passes) — this repo's
established discipline for every ordering/predicate fix this session.

## Gate

```
cargo fmt -p shamir-index -p shamir-engine -- --check
cargo clippy -p shamir-index -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-index -p shamir-engine --full
```

All three must pass clean. Report in your final summary: which option
((a) mask or (b) revert) you chose and why, the full gate output, and
confirmation every LOW/NIT item above was addressed or an explicit reason
given for skipping it.
