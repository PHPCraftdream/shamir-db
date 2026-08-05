# Brief — R0-A: registry generation watermark + full DDL admission coverage (#1006 + #1012)

## Context

S.H.A.M.I.R. Database, `crates/shamir-engine` + `crates/shamir-index`. Part of the
release-blocker execution map
(`docs/dev-artifacts/roadmap/2026-08-05-release-blocker-execution-map.md` §R0-A) —
read that section first, it explains why these two tasks are one architectural
move, not two.

Two independent readonly reviews
(`docs/dev-artifacts/research/2026-08-05-new-wave-readonly-review*.md`) found: (1)
`IndexRegistry`'s generation-watermark accounting is unsound (NP-1/#1006), and (2)
the per-table DDL admission mutex does not cover every DDL entry point (NP-5/#1012).
This brief gives you the concrete design that closes BOTH by construction, rather
than patching #1006 in isolation (which the execution map explicitly warns against
— see below for why).

## Part 1 — the confirmed defect (#1006)

`crates/shamir-index/src/registry.rs:64-97` — `IndexRegistry` has TWO counters:

- `insert_ticket: AtomicU64` — `fetch_add`'d once per `insert()` call (`:164`) to
  produce a per-entry tag guaranteed distinct across concurrent inserts.
- `generation: AtomicU64` — the PUBLISHED watermark commit-time re-derivation gates
  on (`crates/shamir-engine/src/tx/pre_commit.rs:825`,
  `if reg.generation() == stage_gen { continue }`); bumped via `fetch_max(my_gen)`
  on insert (`:198`) and `fetch_add(1)` on remove (`:275`).

Read the doc comments at `registry.rs:78-81` and `:92-94` — **both assert
invariants that do not hold**:

- `:92-94` claims `insert_ticket`'s absolute values "never need to match
  `generation`'s". False: `backends_newer_than(threshold_gen)` (`:237-248`)
  compares `entry.gen > threshold_gen` where `entry.gen` IS the ticket value — so
  the ticket and the published generation share one comparison space whether the
  comment admits it or not.
- `:78-81` claims observing `generation() == N` guarantees every entry tagged
  `<= N` is visible in `by_id`. False under out-of-order publish: insert A reserves
  ticket 1 and stalls before `insert_async`; insert B reserves ticket 2 and
  publishes fully (`generation` becomes 2 via `fetch_max`); a reader observing
  `generation() == 2` proceeds as if A (tag 1, `<= 2`) is visible — it is not yet.

**Deterministic failure (no concurrency needed):** `CREATE A` (ticket 1, publish →
generation 1) → `DROP A` (`fetch_add` → generation 2, `insert_ticket` untouched at
1) → tx stages at generation 2 (A already gone, correctly empty plan) → `CREATE B`
(ticket 2, `fetch_max(2)` is a no-op since generation is already 2) → commit sees
`generation() == stage_gen (2)`, skips re-derivation entirely — **B's posting is
never written for a row committed after B existed.**

## Part 2 — why #1006 cannot be fixed in isolation (#1012's role)

Renaming/merging the two counters into one (e.g. `generation.fetch_add(1)` used
directly as both the publish bump AND the per-entry tag) fixes the **sequential**
scenario above but does **not** fix the **out-of-order publish** scenario, because
"reserve a tag" and "actually insert into `by_id`" remain two separate steps that
can still interleave across two concurrent callers.

**The actual fix is serialization, not a smarter counter.** Verify this yourself
before proceeding — read `crates/shamir-engine/src/table/table_manager.rs:937-965`
(`begin_write_barrier`). It already:

1. Acquires `self.ddl_admission` (`Arc<tokio::sync::Mutex<()>>`, per-table) FIRST
   (`:953`) — this already exists and already serializes concurrent DDL that
   shares a write-barrier bit.
2. Raises the intent bit, drains in-flight writers, then takes
   `unique_write_lock`.
3. Returns a `WriteBarrierGuard` that must be held for the ENTIRE
   snapshot/backfill/register/persist body — confirm by reading how
   `create_index_v2` (`table_manager_index_mgmt.rs:24-90` roughly, follow
   `.begin_write_barrier(INDEX2_CREATE)` at `:86`), `create_index` (regular,
   `:596-603`), and `create_unique_index` (`:678-690`) all hold the guard across
   their full backfill+register+persist sequence.

**Confirmed gaps in admission coverage** (verify each by reading the code, line
numbers may have drifted slightly since the review):

- `crates/shamir-engine/src/table/table_manager_sorted_index.rs:297-304` —
  `drop_sorted_index` calls `self.sorted_indexes.drop_index(...)` directly, no
  `begin_write_barrier` call anywhere near it.
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:877` —
  `drop_index2` — check whether it holds `begin_write_barrier` across its
  tombstone→retire→sweep sequence; the review found it does not.
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1428` —
  `rename_index` — `begin_write_barrier(UNIQUE_INDEX_CREATE)` appears only in
  the unique branch (`:1602`); check whether the sorted and index2 rename
  branches hold an equivalent barrier for their FULL duration (tombstone,
  physical rekey/rename, metadata persist).

**Once every CREATE/DROP/RENAME for all four families holds `ddl_admission` (via
`begin_write_barrier` or an equivalent guard) for its ENTIRE critical section —
including the actual `IndexRegistry::insert()`/`remove_by_id()` call and the
generation bump — no two registry mutations can ever be in flight at the same
time for a given table.** That eliminates the out-of-order-publish race
structurally, not by cleverer atomics. This is why the execution map groups #1006
and #1012 as one move: #1012's admission-coverage fix IS #1006's concurrency fix,
once you also route the registry mutation itself through the same critical
section.

## Part 3 — what to actually implement

1. **Extend admission coverage (#1012).** Wrap `drop_sorted_index`, `drop_index2`,
   and the sorted/index2 branches of `rename_index` in `begin_write_barrier` with
   the appropriate bit (check `write_barrier_flags` for the existing bit
   constants — `SORTED_INDEX_CREATE`/`INDEX2_CREATE` or equivalent; if a DROP/RENAME
   needs its own bit rather than reusing the CREATE bit, that's a legitimate
   finding — decide and justify in your report, but reusing the existing bit per
   family is the simpler default unless you find a concrete reason two operations
   on the same family must NOT mutually exclude each other). The guard must be
   held across the ENTIRE tombstone → physical mutation → registry mutation →
   metadata persist sequence — not just part of it.

2. **Simplify `IndexRegistry`'s counter model (#1006).** Once (1) guarantees only
   one registry-mutating DDL op is ever in flight per table, `insert_ticket` no
   longer needs to be decoupled from `generation` for concurrency-safety —
   simplify to ONE counter: `insert()`'s per-entry tag and the published watermark
   become the same `generation.fetch_add(1, Release) + 1` value, computed and
   applied atomically as part of the single admission-serialized critical section.
   Before deleting `insert_ticket`, grep every caller of `entry_gen`
   (`#[cfg(test)]`, `registry.rs:117`) and any other reader of the ticket concept
   to confirm nothing outside this file depends on it being independent from
   `generation`.
   - Correct the now-true doc comments at `registry.rs:78-81` and `:92-94` to
     describe the ACTUAL invariant (serialization via the caller's admission
     guard, not a decoupled-ticket scheme) — do not leave stale reasoning in
     place once the code no longer matches it.
   - `remove_by_id` (`:259-280`) keeps its own `fetch_add(1)` on the SAME counter
     (already the case) — verify this is still correct once insert's tag comes
     from the same counter (it should be: both are simple monotonic bumps now).

3. **Consider (but do not silently skip) whether `SortedIndexManager` and the
   base `IndexManager` (regular+unique) have an analogous concern.** They use a
   SINGLE plain `generation: Arc<AtomicU64>` with `fetch_add` (no separate ticket,
   no `backends_newer_than`-style per-entry tag comparison) —
   `crates/shamir-index/src/base_index/sorted_index_manager.rs:111,615,823,924` and
   `crates/shamir-index/src/base_index/index_manager.rs:189,1047-1048`. Confirmed
   during this brief's preparation: these do NOT have the ticket/generation
   desync bug (#1006 is index2-`IndexRegistry`-specific), but their generation
   bumps must ALSO happen only while the caller holds `ddl_admission` for the
   same "no concurrent registry mutation" reasoning to apply to them. Verify (1)
   already covers their CREATE/DROP/RENAME call sites too (regular/unique CREATE
   already do per the grep above; confirm DROP and RENAME for these two families
   are equally covered — the map's #1012 scope includes "regular rename does not
   hold one atomic section across drop+create transition", check that too).

## Part 4 — tests (write these; each must fail against the reverted code)

- `CREATE A → DROP A → stage tx → CREATE B → commit`: B's posting is present
  (registry-level test mirroring the scenario in Part 1, plus an end-to-end
  `TableManager`-level test through the real tx commit path).
- Out-of-order publish: a deterministic-pause test (reuse the existing
  `set_create_index2_backfill_hook`/`BackfillPauseHook` pattern already used
  elsewhere in this crate for parking a CREATE mid-flight) where insert A is
  parked BEFORE `insert_async` completes while insert B (for a DIFFERENT table or
  a scenario you construct) publishes fully — assert a reader never observes
  `generation() == B's tag` while A is still not visible in `by_id`. (If the new
  serialized design makes this scenario structurally impossible to construct as a
  race — i.e., admission genuinely prevents two concurrent inserts on the same
  table — say so explicitly in your report and instead write a test that PROVES
  two DDL ops on the same table cannot both be inside `begin_write_barrier` at
  once, which is the real invariant now doing the work.)
- Admission coverage: a test per newly-covered gap (sorted DROP, index2 DROP,
  sorted RENAME, index2 RENAME) proving a concurrent writer or a second DDL op on
  the same bit correctly blocks/serializes rather than racing — mirror whatever
  test pattern the existing `REGULAR_INDEX_CREATE`/`UNIQUE_INDEX_CREATE` admission
  tests already use (grep for tests referencing `ddl_admission` or
  `begin_write_barrier` for the pattern).
- Invariant test: equal observed `generation()` implies equal planner-visible
  `(id, entry.gen)` set — direct assertion, not just the scenario tests above.

## Constraints

- Follow `CLAUDE.md`: `tokio::sync::Mutex` for guard-across-`.await` bounded
  contention is the sanctioned pattern here (matches the EXISTING
  `ddl_admission`/`unique_write_lock` precedent — do not introduce a new
  synchronization primitive type, reuse what's there).
- Test files go under the crate's existing `tests/` directory convention
  (manifest-only `mod.rs`, no inline `#[cfg(test)] mod tests { ... }`).
- Gate: `cargo fmt -p shamir-index -p shamir-engine`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-index -p shamir-engine --full` must be green.
  Given this touches concurrency-sensitive DDL admission, also run
  `./scripts/test.sh @oracle` (tx/engine scope) if that scope differs from the
  two crates above, to catch any cross-crate regression in commit-path
  re-derivation.
- Do NOT expand scope into R0-B (#1007/#1008 — sorted rename generation bump +
  reconcile/ABA) or R0-C (#1009/#1010 — registry insert atomicity + namespace) —
  those are separate briefs. If you find your admission-coverage fix here
  overlaps with #1009's "atomic two-projection insert" concern (both touch
  `IndexRegistry::insert`), note the overlap in your report but do NOT
  implement #1009's fix here — leave a comment marking what #1009 will still
  need to do (preflight name uniqueness) that is out of this brief's scope.
- Do not touch `crates/shamir-engine/src/table/degraded_index_count.rs` or
  `IndexState` (those belong to the already-completed R0-D, commit `5935b346`).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `drop_sorted_index`, `drop_index2`, and the sorted/index2 branches of
      `rename_index` hold `begin_write_barrier` (or equivalent) across their
      FULL critical section.
- [ ] `IndexRegistry`'s `insert_ticket`/`generation` split is resolved — either
      merged into one counter (recommended) or otherwise proven race-free, with
      the stale doc comments at `:78-81`/`:92-94` corrected to match reality.
- [ ] `CREATE A → DROP A → stage → CREATE B → commit` test passes and is shown to
      fail against the pre-fix code.
- [ ] Out-of-order-publish scenario is either fixed-and-tested or proven
      structurally impossible under the new admission coverage, with that proof
      stated in your report.
- [ ] Admission-coverage tests for each newly-covered DROP/RENAME path.
- [ ] `SortedIndexManager`/base `IndexManager` generation bumps confirmed (not
      just assumed) to happen only under `ddl_admission` for CREATE/DROP/RENAME.
- [ ] fmt/clippy/tests green (report the exact commands and pass/fail).
