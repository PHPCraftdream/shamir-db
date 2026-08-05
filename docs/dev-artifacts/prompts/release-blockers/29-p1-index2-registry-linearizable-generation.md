# Brief — IndexRegistry::insert generation tagging must be linearizable

Task: #992 in the session TaskList. Found by the post-campaign `@oh` review
of #957-971 (2026-08-04/05). Read this brief in full — a design decision
has already been made (below); do not re-derive or second-guess it, just
implement it precisely.

## The bug — confirmed by direct trace, not just the review's claim

`IndexRegistry::insert` (`crates/shamir-index/src/registry.rs:96-166`)
computes its per-entry generation tag as:

```rust
let my_gen = self.generation.load(Ordering::Acquire) + 1;
// ... publish to by_id / by_name ...
self.generation.fetch_max(my_gen, Ordering::Release);
```

Two CONCURRENT inserts that both read `generation() == G` both compute
`my_gen = G+1` — this is a classic read-then-write race (not an atomic
reservation). Concrete failure trace, verified against the actual
consumer (`pre_commit.rs:825`, `if reg.generation() == stage_gen { continue;
}` — the exact commit-time skip-rederive shortcut):

1. Insert A and insert B both read `generation() == G` (racing before
   either publishes).
2. A publishes (`by_id`/`by_name`), then `fetch_max(G+1)` → generation
   becomes `G+1`.
3. A tx's stage-time snapshot reads `generation()` at this exact moment →
   captures `stage_gen = G+1`.
4. B publishes (`by_id`/`by_name`) AFTER the tx's stage snapshot — B's
   backend is now live but the tx's stage-time plan has no ops for it.
5. B's own `fetch_max(G+1)` is a no-op (generation is already `G+1`).
6. At commit, `pre_commit.rs:825` compares `reg.generation() == stage_gen`
   → `G+1 == G+1` → **the rederive is skipped entirely** (not just
   filtered — `backends_newer_than` is never even called), so the tx
   commits with zero ops for B's backend — the exact "guaranteed miss"
   class #958/#987 exist to prevent, reintroduced through a different
   mechanism.

**Currently unreachable in production**: #957's per-table `ddl_admission`
mutex serializes every index2-create on a given table, so two concurrent
`insert()` calls on the SAME registry cannot happen today. This is a
fragile coupling to an unrelated subsystem's implementation detail — if
#957's admission mutex is ever narrowed/removed/reworked, this becomes a
live, silent data-integrity bug with no test currently guarding it.

## Decision (made by the orchestrator — implement this, do not pick the
## doc-only alternative the task description also offered)

Fix the root cause: make `IndexRegistry::insert`'s generation tagging
genuinely linearizable, independent of any external caller's serialization.
**Do not** just document the #957 coupling — this is the same failure
class that already caused one real, shipped corruption bug this campaign
(#987); leaving a second instance as a documented landmine is not
acceptable for a release-track fix.

### The exact design

Add a SEPARATE ticket counter, decoupled from the publicly-observable
"published watermark" (`generation`):

```rust
pub struct IndexRegistry {
    by_id: scc::HashMap<u32, BackendEntry, THasher>,
    by_name: scc::HashMap<u64, u32, THasher>,
    next_id: AtomicU32,
    generation: AtomicU64,
    /// P1 (#992): monotonic ticket counter for `insert()`'s per-entry
    /// generation tag — decoupled from `generation` (the PUBLISHED
    /// watermark readers observe via `generation()`). `fetch_add` on this
    /// counter is atomic, so two concurrent `insert()` calls are guaranteed
    /// distinct tickets regardless of interleaving — closing the race where
    /// `generation.load() + 1` let two concurrent inserts compute the SAME
    /// tag. `generation` itself is still only advanced (via `fetch_max`)
    /// AFTER the corresponding entry is published — preserving the
    /// Release/Acquire happens-before invariant P0-2 (#958 2b) established
    /// (a reader observing `generation() == N` is guaranteed every entry
    /// tagged `<= N` is already visible in `by_id`).
    insert_ticket: AtomicU64,
}
```

In `insert()`, replace:
```rust
let my_gen = self.generation.load(Ordering::Acquire) + 1;
```
with:
```rust
let my_gen = self.insert_ticket.fetch_add(1, Ordering::Relaxed) + 1;
```
(`Relaxed` is sufficient — `insert_ticket` has no cross-thread
happens-before obligation of its own; the ORDERING guarantee the rest of
the system depends on is still carried entirely by `generation`'s existing
`fetch_max(..., Release)` / `load(..., Acquire)` pair, unchanged below this
line.) Everything else in `insert()` (publish, then
`self.generation.fetch_max(my_gen, Ordering::Release)`) stays EXACTLY as
it is — only the computation of `my_gen` changes.

**Why this is correct**: `AtomicU64::fetch_add` is a true fetch-and-add —
two concurrent callers are guaranteed to receive DISTINCT return values
(one gets `n`, the other gets `n+1`, never the same value twice), so
`my_gen` is now unique per insert regardless of scheduling. The existing
publish-then-`fetch_max(Release)` step is untouched, so the original P0-2
(2b) invariant (generation only becomes visible to readers AFTER the
corresponding entry is durably published) is fully preserved — this fix
only changes HOW `my_gen` is computed, not when `generation` becomes
observable.

**Do NOT** initialize `insert_ticket` from `generation`'s current value or
try to keep them in lockstep numerically — they are independent counters
serving different purposes (ticket = uniqueness source; `generation` =
published watermark the rederive gate reads). Starting `insert_ticket` at
0 in `IndexRegistry::new()` is correct; its absolute values never need to
match `generation`'s.

**Also check `remove_by_id`** (`registry.rs:225-246`): it already uses
`self.generation.fetch_add(1, Ordering::AcqRel)` directly (a true
fetch-and-add, not a load-then-compute) — this is ALREADY linearizable
and needs NO change. Confirm this yourself by re-reading it; do not modify
it.

## Update the doc comment

The existing doc comment on `insert()` (lines 101-129) explains the OLD
non-linearizable scheme's "harmlessness" argument (a tag that ends up
slightly low just means a slightly wider `backends_newer_than` filter).
That argument is not what actually made the old code safe (the review
found the `current_gen == stage_gen` skip-shortcut bypasses that filter
entirely) — rewrite the comment to describe the NEW ticket-based scheme
accurately: `insert_ticket.fetch_add` guarantees distinct tags for any two
concurrent inserts, which combined with the unchanged
publish-then-`fetch_max(Release)` ordering closes the gap. Keep the
Release/Acquire happens-before explanation (still accurate and still the
load-bearing part) but replace the "harmless because idempotent ops" framing
with the real fix.

## Required tests

Add to `crates/shamir-index/src/tests/` (find the existing test file for
`registry.rs` — search for `IndexRegistry` test module; if none exists
dedicated to it, check where `backends_newer_than`/`insert`/`remove_by_id`
are currently tested and extend that file) — a real concurrency test
proving the fix:

1. **Direct regression for the exact race**: spawn two concurrent
   `tokio::spawn` tasks each calling `registry.insert(...)` with a
   distinct backend, synchronized via a `tokio::sync::Barrier` (or a
   oneshot-channel rendezvous) so BOTH tasks are guaranteed to compute
   their `my_gen` value before EITHER publishes — this requires a small
   test-only seam (an async pause point) inside `insert()`, OR — simpler —
   if the ticket-uniqueness property is what matters (not the exact
   old-vs-new interleaving), skip the seam and just assert the INVARIANT
   directly: call `insert()` from N concurrent tasks
   (`futures::future::join_all` over spawned tasks), then read back each
   inserted entry's `gen` (you may need a small crate-internal test-only
   accessor, or read it indirectly via `backends_newer_than(threshold)`'s
   behavior for each threshold in the returned range) and assert every
   `gen` value is DISTINCT — proving two concurrent inserts can never tag
   identically. Prefer this simpler property-based test over building a new
   pause-hook seam, unless you find the seam is trivial to add and clearly
   more convincing — your call, but justify it in the report.
2. **Regression for the specific commit-time skip bug**: a test at the
   `pre_commit.rs`/`TxContext` level (or, if that's too heavy a lift for
   this task's scope, a targeted `shamir-index` test asserting
   `generation()` strictly increases by exactly the number of successful
   concurrent inserts — i.e. N concurrent inserts advance `generation()`
   by exactly N, never fewer) is an acceptable substitute if a full
   tx-level integration test is disproportionate. Use your judgment on
   which level proves the fix most directly without over-scoping.

## Scope discipline

- Do NOT touch `remove_by_id` — already linearizable, confirmed above.
- Do NOT touch `backends_newer_than`, `set_state`, `get_by_id`,
  `get_by_name`, or any other `IndexRegistry` method.
- Do NOT touch the base_index/sorted family's OWN generation counters
  (`IndexManager`/`SortedIndexManager`) — this task is scoped to
  `IndexRegistry` (the index2 family) only, which is where the review
  found this specific issue.
- Do NOT touch `pre_commit.rs`'s consumer logic (`reg.generation() ==
  stage_gen` check) — the fix is entirely inside `IndexRegistry::insert`;
  the consumer's existing shortcut becomes CORRECT once the tag source is
  linearizable.

## Gate (MANDATORY)

```
cargo fmt -p shamir-index -- --check
cargo clippy -p shamir-index --all-targets -- -D warnings
./scripts/test.sh -p shamir-index --full
```
Also run the engine suite once, since `pre_commit.rs`'s consumer logic
depends on this invariant being correct:
```
./scripts/test.sh -p shamir-engine --full
```

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Show the exact diff of `insert()`'s `my_gen` computation and the new
`insert_ticket` field. Explain which test approach you chose (the
property-based N-concurrent-inserts-distinct-tags test, or a full
seam-based race reproduction) and why. Give exact gate command output for
both `shamir-index` and `shamir-engine`.
