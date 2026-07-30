# F-69 (#896) — collapse the 6-atomic writer-barrier predicate into one atomic

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The bug

`TableManager::needs_write_barrier()`
(`crates/shamir-engine/src/table/table_manager.rs`, currently around
lines 820-836) is:

```rust
pub(crate) fn needs_write_barrier(&self) -> bool {
    self.index_manager.has_unique_indexes()
        || self.index2_create_barrier.load(Ordering::SeqCst)
        || self.schema_activation_barrier.load(Ordering::SeqCst)
        || self.regular_index_create_barrier.load(Ordering::SeqCst)
        || self.unique_index_create_barrier.load(Ordering::SeqCst)
        || self.sorted_index_create_barrier.load(Ordering::SeqCst)
}
```

Six independent atomics read one after another — NOT a single atomic
snapshot. Worse: the FIRST, short-circuiting operand,
`IndexManager::has_unique_indexes()`
(`crates/shamir-index/src/legacy/index_manager.rs:191-193`), loads its
backing `AtomicBool` with `Ordering::Relaxed`, not `SeqCst` like its five
siblings.

**Confirmed by reading the code this session**: `has_indexes_unique` flips
to `true` inside `create_unique_index_locked`
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`, called from
`create_unique_index` after it acquires `unique_write_lock` and drains —
"the actual drain happens inside `create_unique_index_locked`", per that
function's own comment). A writer that loads `has_unique_indexes()` as
`false` a moment before this flip, takes the lock-free fast path for its
ENTIRE validate→write→index sequence with no further check. If that
writer's `WriterDrainBarrier::enter_writer` bump (an entirely separate
atomic, `active`) is reordered relative to its `has_unique_indexes` read —
which `Relaxed` permits — the concurrent `create_unique_index_locked`'s
`drain()` call can observe `active == 0` and proceed, believing no
fast-path writer is in flight, while this writer is still mid-write. The
result: a duplicate value slips past a unique constraint that just went
live. Silent data corruption, not just a theoretical race.

## Why F-56 didn't already close this

F-56 (#882, commit `7fde958e`) proved a `SeqCst` total-order argument for
EXACTLY ONE flag+counter pair: `schema_activation_barrier` and
`WriterDrainBarrier::active` (see the module doc at the top of
`crates/shamir-engine/src/table/writer_drain_barrier.rs` — read it in
full before touching anything, the proof's own reasoning must be extended,
not contradicted). That proof's scope was the SPECIFIC pair it was applied
to; it never covered `has_unique_indexes`, which uses a completely
different backing atomic (`IndexManager::has_indexes_unique`, a different
crate even — `shamir-index`, not `shamir-engine`) and was never wired into
`WriterDrainBarrier`'s drain-set accounting at all in the same rigorous
way. The bug survived because the proof's scope was assumed to cover
"the write barrier" as a whole, when it actually covered one flag out of
six.

## The fix

Collapse ALL SIX conditions into ONE atomic word — a packed bitfield in a
single `AtomicU64` (or `AtomicU8`/`AtomicU32` if 6 bits + headroom fits
and you have a principled reason to prefer a narrower word; justify the
choice, don't just default to U64). This:

1. **Eliminates the torn read entirely.** One atomic load can never be
   torn regardless of ordering — the compound six-flag OR becomes a
   single memory operation.
2. **Makes F-56's SeqCst proof cover the WHOLE predicate**, not 5/6 of
   it, once every setter of a former individual flag instead sets its bit
   in the packed word (still `SeqCst`, to preserve the existing proof's
   total-order argument — do not weaken any individual bit's ordering
   while merging).
3. **Removes five cache lines from the writer hot path** (a real
   secondary win review #1 separately flagged as a P1 cost concern for
   F-56 — one atomic load instead of six is strictly better for the
   uncontended fast path every non-DDL writer takes).

Design questions to resolve as part of this task, not left to guesswork:

- Does `has_unique_indexes`'s bit need to move location (from
  `shamir-index`'s `IndexManager` into `shamir-engine`'s `TableManager`,
  or vice versa) to live in the SAME atomic as the other five? Read both
  structs' actual field layout and ownership before deciding — a
  cross-crate `Arc<AtomicU64>` shared between `IndexManager` and
  `TableManager` is one legitimate shape; moving the source-of-truth flag
  entirely into `TableManager` and having `IndexManager` notify it on
  index-count transitions is another. Pick the one that keeps
  `IndexManager`'s and `TableManager`'s existing responsibilities clean —
  don't let one struct reach into the other's private fields.
- Should `WriterDrainBarrier::active` (the writer count, a *separate*
  `AtomicUsize` today) be folded into the SAME packed word as the six
  gate bits, or stay a distinct atomic? Folding buys a further reduction
  in atomics-per-check but raises the bit-packing budget (need enough
  bits for the realistic max concurrent-writer count on one table — check
  what bounds exist today, e.g. connection/session limits, or justify an
  assumed ceiling) and complicates `enter_writer`'s RMW (a `fetch_add` on
  a sub-field of a packed word needs care not to corrupt neighboring
  bits — a raw `fetch_add` on a shared word is unsafe for packed
  bitfields; you'd need `fetch_update` with a CAS loop, changing the
  cost profile documented in `writer_drain_barrier.rs`'s "Cost" section).
  If you fold the counter in, update that section's cost analysis
  honestly (CAS-loop is not "one locked instruction" the way today's
  `fetch_add` is). If you DON'T fold it in, that's an acceptable,
  simpler scope for this task — just don't mix "6 gate bits in one atomic"
  with "wait, and I also silently changed the counter's semantics" without
  saying so.
- Do NOT simply upgrade `has_unique_indexes`'s load to `SeqCst` and stop.
  That fixes ONE operand's ordering but leaves a six-atomic non-atomic
  compound read — the predicate as a whole is still not a consistent
  snapshot. The task is explicit that this half-fix is rejected.

## Definition of done

- `needs_write_barrier()` (or its successor) performs exactly ONE atomic
  load to evaluate the compound condition.
- Every setter that used to flip one of the six original flags now sets
  the corresponding bit in the packed word, still under an ordering that
  preserves F-56's total-order argument (SeqCst, unless you have a
  rigorous alternative proof — don't downgrade without one).
- `writer_drain_barrier.rs`'s module-doc proof is updated to state
  explicitly that it now covers the FULL six-condition predicate (or
  seven, if the active counter got folded in too), not just
  `schema_activation_barrier`. Future readers must not have to re-derive
  what you already proved.
- A new test demonstrates the closed race: a table concurrently gains a
  live unique index WHILE a fast-path writer is in flight, and the
  writer cannot admit a duplicate. Use this codebase's existing
  deterministic pause-seam convention (grep for `TEST_POST_BARRIER_PRE_WRITE_HOOK`
  or similar `TEST_*` hooks already used for this exact class of test in
  prior F-5x/F-6x work — follow that pattern, no `sleep`-based timing).
- `cargo fmt -p shamir-engine -p shamir-index -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx --full`
  green (loom-based memory-model tests, if any exist for this barrier,
  must also still pass — check `crates/shamir-engine`'s loom test target
  referenced by F-56).

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
