# F-71 (#898) — fix AsOf epoch initialization (restart / CREATE / RENAME)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The bug — three independent ways the AsOf seek gate opens when it shouldn't

The AsOf sorted-index cursor-seek fast path is gated by:
`last_mutation_version(idx) <= pinned_version ⟹ safe to seek`. A LOW
epoch value opens the gate (permits the fast path). Three vectors make the
epoch wrongly low, each independently confirmed by reading the code:

1. **Restart.** `SortedIndexManager::load()`
   (`crates/shamir-index/src/legacy/sorted_index_manager.rs`) hydrates only
   `self.indexes` (definitions) — never the `last_mutation_version` map. A
   missing entry reads as `0`. After ANY restart, every index reads epoch
   `0`, so `0 <= pinned` holds for every pinned version, and an arbitrarily
   old AsOf query wrongly takes the fast path against an index that may
   reflect much newer state.
2. **CREATE INDEX.** The sorted backfill calls
   `on_record_created(&id, &record, 0)` — literal version `0`
   (`crates/shamir-engine/src/table/table_manager_sorted_index.rs:134`,
   confirmed this line number may have shifted — find the actual call by
   grepping for it). A freshly built index gets epoch `0` even though it
   mirrors current state at, say, v100 — an `AsOf(10)` query then wrongly
   takes the fast path and silently omits a row deleted between v10 and
   the build.
3. **RENAME INDEX.** `rename_definition` does not carry the old name's
   epoch to the new `name_interned` — a rename resets the gate to `0`.

**Why this is OUR regression, not inherited.** Pre-F-67 (#893, commit
`e7a8c707`), the counter was manager-WIDE, so any table with prior
mutation history already had it sitting high — creating or renaming an
index left it high, so the gate stayed correctly CLOSED. F-67 gave each
index its OWN epoch starting at `0`, which is exactly what re-opens the
gate for vectors 2 and 3. Vector 1 (restart) predates F-67 (goes back to
F-53b/F-58) and means the seek-safety proof was never sound across a
process restart in the first place.

## The fix — epoch must express AGE OF CONTENT, not "mutations this process observed"

Both independent readonly reviews converge on this direction:

- **On index transition to Ready** (end of a successful backfill), set its
  epoch to at least the table's current `last_committed_version` —
  **never** `0`. The index's content reflects everything up to that
  version; anything older is provably safe for the fast path, but nothing
  younger is.
- **On open/restart**, seed every loaded index with a conservative floor.
  Two shapes are viable — pick and justify one:
  (a) persist `ready_at_version`/`last_mutation_version` in the index
  descriptor itself (durable, survives restart with the EXACT right
  value), or (b) if persisting per-index state is out of scope for this
  task's size, seed every loaded index at open time with the
  table's/repo's own open-time watermark (conservative — closes the gate
  more often than strictly necessary, but never incorrectly opens it).
  Prefer (a) if it fits cleanly; do not ship (b) silently if (a) was
  actually the intended design — say which you chose and why.
- **On rename**, carry the epoch from the old key to the new
  `name_interned` — a rename must not reset the gate.
- Consider whether the gate should require BOTH `pinned >= ready_at_version`
  AND "no newer/in-progress mutation" as two separate, explicit conditions
  rather than one collapsed comparison — clearer to reason about and to
  test each vector independently.

## Definition of done

- A red-then-green (or at minimum a failing-then-passing) test for **EACH
  of the three vectors independently** — restart, CREATE INDEX, RENAME
  INDEX — not just one. The task's own history shows testing only the
  scope-narrowing property (F-67's own verification) missed exactly this
  class of bug; do not repeat that mistake by testing only one vector and
  calling it done.
  - Restart: build an index with mutation history, close/reopen (or the
    in-process equivalent this codebase uses for "restart" tests — grep
    for existing restart-test patterns in `shamir-index`/`shamir-engine`),
    then issue an AsOf query pinned to an OLD version and confirm it does
    NOT silently use the stale fast path (either declines to seek, or
    seeks correctly because the epoch was properly restored — whichever
    your fix produces).
  - CREATE INDEX: on a table with prior mutation history (rows
    inserted/updated/deleted before the index existed), create the index,
    then issue an AsOf query pinned to a version BEFORE the create and
    confirm correctness (no silently-omitted row).
  - RENAME INDEX: mutate, rename, then AsOf-query pinned to before the
    rename and confirm the epoch (and thus correctness) survived the
    rename.
  - Also cover: empty table, non-empty table, and index-create concurrent
    with an existing cursor pin (per the task's own test list).
- `cargo fmt -p shamir-index -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-index -p shamir-engine --full` green.
- If you land on the conservative-floor-only fallback for restart (option
  b above) instead of true persistence (option a), say so explicitly in
  the commit message and justify why — this is a real trade-off (safety
  margin vs. seek-fast-path availability after restart), not a silent
  simplification.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
