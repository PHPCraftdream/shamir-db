# #1102 HIGH — regular hash index publishes `has_indexes` after `bump_generation()`, and the read side has no happens-before edge at all

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

⛔ This brief is running in an isolated git worktree, in parallel with
another agent working on task #1099 in a DIFFERENT worktree on the SAME
underlying repo. You cannot see each other's changes until the
orchestrator merges both back to `master` after independent verification.
**Stay strictly inside the files this brief names.** Task #1099 touches
`crates/shamir-engine/src/table/table_manager_tx_ops.rs` (a `PERF` fix to
`released_unique_keys_in_tx`/`touched_records_in_tx`) and
`pre_commit.rs` — if you find yourself needing to touch either of those
files for any reason, STOP and report it in your final summary instead of
editing them; a real conflict there needs the orchestrator's judgment, not
a race between two agents.

## Background — confirmed by the orchestrator's own investigation before writing this brief

`crates/shamir-index/src/base_index/index_manager.rs` publishes the
regular (non-unique) hash index's existence flag in the WRONG order at 4
sites — `bump_generation()` runs BEFORE `self.has_indexes.store(true,
Ordering::Release)`:

```
:1421-1422   (create_index_from_stream)
:1602-1603   (a second regular-index create path)
:1792-1793   (a third — check each site's enclosing fn name yourself)
:1935-1936   (a fourth, no P0-2 comment on this one — same shape regardless)
```

This is the EXACT structural bug `#1098` (already closed, both rounds
verified sound by adversarial review) fixed for `UNIQUE_INDEX_EXISTS` —
except **weaker**: `has_indexes` is written with `Ordering::Release` but
read with `Ordering::Relaxed` (`index_manager.rs:1327`,
`pub fn has_indexes(&self) -> bool { self.has_indexes.load(Ordering::Relaxed) }`).
A `Release` store paired with a `Relaxed` load establishes **no
happens-before edge whatsoever** — unlike `#1098`'s unique flag, which
already used `SeqCst` on both the store and the load (via the shared
`write_barrier_flags` packed word), so `#1098`'s fix only needed the
WRITER's publish order corrected. Here, correcting the writer's publish
order ALONE is insufficient — the reader ALSO needs an ordering strong
enough to observe that store once it observes a generation bump that
happened after it.

### Failure scenario (already traced by the orchestrator, reproduces the identical class of corruption `P0-2 (#958)` was written to prevent)

1. Table `T` has zero indexes. A tx calls `insert_tx`, which captures
   `base_index_gen` via an `Acquire` load
   (`crates/shamir-engine/src/table/table_manager_tx_ops.rs:591` — this is
   already fixed to run BEFORE the `has_any_index()` check, per `#1098`'s
   own fix pattern for the SAME call site).
2. Concurrently, `CREATE INDEX idx ON T(f)` runs `add_index();
   bump_generation();` — its `has_indexes.store(true, Release)` (the NEXT
   line) has not yet become visible to the tx.
3. The tx's `has_any_index()` read
   (`table_manager_tx_ops.rs:640`, which calls
   `index_manager.has_indexes()` among others) observes stale `false` →
   skips index planning entirely for this insert, staging ZERO ops for the
   new index.
4. At commit, `pre_commit.rs`'s gate `mgr.generation() == stage_gen` finds
   them EQUAL (the tx's own gen capture already reflected the bump) →
   base_index rederive is skipped.
5. The `CREATE INDEX`'s own backfill snapshot predates the (then
   uncommitted) row, so it never saw it either.

Result: the row commits with **no posting** under `idx`, permanently.
`lookup_by_index` silently omits it — no error, anywhere, ever. This is
pre-existing since `P0-2 (#958)`; NOT introduced by `#1096`/`#1097`/`#1098`.

The mirror-image `drop_index` site (`index_manager.rs`, search for
`has_indexes.store(false` — same wrong-order shape) is assessed benign by
the orchestrator's own analysis (a stale-true `has_indexes` read during a
DROP just means a definition-already-removed check finds nothing to do —
not a correctness issue). **Do not "fix" the DROP site's order unless you
find a concrete reason it's unsafe — leave it alone if your own
investigation confirms the orchestrator's benign assessment**, to keep
this change minimal and focused on the real hazard.

## The established precedent this codebase already uses for exactly this class of gate

`crates/shamir-index/src/base_index/write_barrier_flags.rs` is a packed
`Arc<AtomicU8>` word, ALL bit-set/clear/load operations `SeqCst`,
specifically designed for this class of "DDL existence/intent" flag —
`UNIQUE_INDEX_EXISTS` (bit 0) already lives there and is read via
`is_set()`/`any_set()`, both `SeqCst`. The module's own doc (`F-69,
#896`) explains exactly why: a single `SeqCst` load can never be torn,
and merging flags into one packed byte doesn't introduce a CAS loop (a
`fetch_or`/`fetch_and` bitwise RMW is still one locked instruction).

**Investigate whether folding `has_indexes` into this SAME packed word
(as a new bit) is the right fix, before choosing an alternative.**
Arguments for folding, which the orchestrator's own pre-investigation
found compelling but did NOT fully commit to — verify and decide
yourself:

- It reuses the ALREADY-PROVEN `SeqCst` total-order guarantee this exact
  gate exists to provide, rather than introducing a new, narrower
  Acquire/Release reasoning that only this one flag would rely on.
- `has_any_index()` (`table_manager.rs:1339-1344`) already reads
  `self.index_manager.has_unique_indexes()` — a packed-word `SeqCst`
  check — right next to `self.index_manager.has_indexes()` — a
  standalone `Relaxed` `AtomicBool` check. These two checks currently
  have DIFFERENT, inconsistent ordering strength for the exact same
  "does this table have an index of this kind" question. Folding
  `has_indexes` into the packed word makes both checks consistent.
- The orchestrator confirmed the actual production (non-test) read
  fan-out for `has_indexes()` is small and contained: internal call
  sites at `index_manager.rs:2372,2440,2496,2598` (all
  `if !self.has_indexes() { ... }` early-returns inside `IndexManager`
  itself) plus the external accessor at `table_manager.rs:1341`. This is
  a tractable blast radius for a field-shape change (removing the
  standalone `has_indexes: Arc<AtomicBool>` field, adding a
  `REGULAR_INDEX_EXISTS` bit constant to `write_barrier_flags.rs`,
  updating `has_indexes()`'s body to call `self.write_barrier_flags.is_set(REGULAR_INDEX_EXISTS)`,
  updating the 4 create sites' `.store(true, Release)` → `self.write_barrier_flags.set(REGULAR_INDEX_EXISTS)`
  moved BEFORE `bump_generation()`, same shape as `#1098`'s
  `create_unique_index_from_records` fix).

**If your own investigation finds a reason folding is wrong or
riskier than it looks** (e.g. a subtlety in how `IndexManager::clone()`
shares state, or a test that specifically asserts `has_indexes`'s
standalone-`AtomicBool` identity/independence from the unique flag), fall
back to the narrower fix instead: swap the 4 create sites' publish order
(flag before gen-bump, exactly mirroring `#1098`'s
`create_unique_index_from_records` swap) AND upgrade the read side from
`Ordering::Relaxed` to `Ordering::Acquire`
(`index_manager.rs:1327`) — write out the FULL happens-before argument
for why `Release`-store + `Acquire`-load + writer-publishes-flag-before-gen
closes the race, mirroring the exact proof shape `#1098`'s fix comment in
`table_manager_tx_ops.rs`'s `insert_tx` already contains, adapted to this
flag pair. **Do not ship a bare ordering change without writing out this
argument as an inline comment** — an unjustified `Ordering` choice on a
newly-touched concurrency primitive is exactly the mistake this whole
task lineage (`#1096`→`#1097`→`#1098`) has repeatedly found and had to
correct via adversarial review; get it right the first time by reasoning
it through explicitly, in writing, before compiling.

Whichever approach you choose, state your choice and reasoning clearly in
your final summary so the orchestrator's review can independently verify
the SPECIFIC argument you relied on, not just that tests pass.

## Tests to add

Mirror `#1098`'s exact pattern:
`crates/shamir-index/src/base_index/tests/index_manager_tests/p1102_regular_index_gen_order_tests.rs`
(wired into that directory's `mod.rs`), containing:

1. A deterministic pause-seam reproduction (mirroring `#1098`'s
   `PostFlagSetPreGenBumpHook`/`p1098_writer_order_tests.rs`): park a
   `CREATE INDEX` call strictly after the (fixed) flag publish and before
   the generation bump (or, if you chose the fold-into-packed-word
   approach, wherever the equivalent "flag now visible, gen not yet
   bumped" window sits in your fixed code), and prove a reader parked
   there, reading generation-then-flag in that order, observes the flag
   already `true` — the SAME proof shape `#1098`'s
   `p1098_writer_flag_before_gen_bump_is_visible_to_a_reader_parked_mid_create`
   test uses; read that test yourself and mirror its structure closely.
2. An end-to-end reproduction of the failure scenario from this brief's
   Background section, using a raw `IndexManager` (mirroring
   `#1098`'s `p1098_writer_order_tests.rs`'s `create_manager()` helper
   pattern) OR the full `TableManager`/`RepoInstance` stack (mirroring
   `#1097`'s test style) — your choice, whichever is more natural given
   which fix approach you took. Must genuinely exercise the race via the
   pause seam (not just assert generation monotonicity — the orchestrator
   has ALREADY caught and rejected one prior batch of vacuous
   "structural" smoke tests this session for exactly this failure mode;
   do not repeat that mistake).

**Mutation-test your own fix before considering it done**: temporarily
revert just the ordering fix (keep the seam in place), confirm the new
test(s) fail, restore the fix, confirm they pass again. Report this in
your final summary — the orchestrator will also independently re-verify
this via its own mutation-testing pass, so do not skip it just because
it's "extra" work; a fix that passed the wrong test is worse than no fix.

## Gate

```
cargo fmt -p shamir-index -- --check
cargo clippy -p shamir-index --all-targets -- -D warnings
./scripts/test.sh -p shamir-index --full
```
All three must pass clean.

Do not touch anything outside `crates/shamir-index/` (this fix is fully
contained within that crate — no `shamir-engine` changes are needed,
since `table_manager.rs`'s `has_any_index()` only ever CALLS
`index_manager.has_indexes()`, it doesn't need its own ordering change).
