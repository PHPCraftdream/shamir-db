# Brief — F-3: route DROP TABLE CASCADE and doctor::repair() through ddl_admission

## Context

S.H.A.M.I.R. Database, `crates/shamir-db` + `crates/shamir-engine`. An
adversarial review of the R0 correctness-freeze wave
(`docs/dev-artifacts/research/2026-08-06-r0-wave-adversarial-review.md`
§F-3) found that `DROP TABLE ... CASCADE` and `doctor::repair()` mutate
index registries directly, bypassing the `TableManager::ddl_admission`
guarantee that R0-A's registry-counter merge (commit `125b7981`) declared
as its SOLE justification for safety. Tracked as task #1030.

## Prerequisite — already landed, reuse it

R0-A (`125b7981`) extended `begin_write_barrier` coverage to
`drop_sorted_index`/`drop_index2`/both RENAME branches and merged
`IndexRegistry`'s two counters into one, safe SPECIFICALLY because
`IndexRegistry::insert`'s doc comment (`crates/shamir-index/src/registry.rs`,
search `# Precondition`) states: "this method is check-then-act ... safe
from a concurrent second `insert()` on the SAME table ONLY because ... every
registry-mutating DDL op ... takes [`ddl_admission`] for its entire critical
section". The two sites below are NOT covered by that guarantee.

## The defect (verify current line numbers, they may have shifted)

### 1. `DROP TABLE ... CASCADE`

`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` — search for
`if op.cascade` (currently around `:204-245`). Mutates all four index
families DIRECTLY via manager-level calls, never through the barrier-taking
`TableManager` wrapper methods:

```rust
if op.cascade {
    if let Some(db) = self.shamir.get_db(&self.db_name) {
        if let Ok(table) = db.get_table(&op.repo, &op.drop_table).await {
            for id in regular_ids { let _ = table.index_manager_ref().drop_index(id).await; }
            for id in unique_ids  { let _ = table.index_manager_ref().drop_unique_index(id).await; }
            for id in sorted_ids  { let _ = table.sorted_indexes().drop_index(id).await; }
            for b in &backends    { let _ = table.index2_registry().remove_by_id(b.descriptor().id).await; }
        }
    }
}
```

None of `index_manager_ref().drop_index/drop_unique_index`,
`sorted_indexes().drop_index`, or `index2_registry().remove_by_id` acquire
`ddl_admission` — they are the raw manager primitives `TableManager`'s own
`drop_index`/`drop_unique_index`/`drop_sorted_index`/`drop_index2` wrap
WITH the barrier (confirm by reading those `TableManager` methods — they
call `begin_write_barrier` then the exact same manager primitive).

### 2. `doctor::repair()`

`crates/shamir-engine/src/table/doctor.rs` — search for the self-heal loop
(currently around `:501-537`). Same pattern: direct
`index_manager_ref().drop_index`/`drop_unique_index`,
`sorted_indexes().drop_index`, `sorted_indexes().register(def.clone())` —
none under admission.

### Concrete failure scenario (registry-level, verify by tracing, matches
NP-1's original class exactly)

`DROP TABLE t CASCADE` running concurrently with `CREATE INDEX ... ON t`
(any family). Say `generation == N` on the relevant registry. The CREATE
(under admission via `begin_write_barrier`) computes `my_gen = N + 1` and
publishes. In the SAME window, cascade's direct `remove_by_id`/`drop_index`
call does its own `generation.fetch_add`/equivalent WITHOUT admission,
landing at `N + 1` too (two independent mutations racing to the same next
value, un-serialized). Net effect: the tx-plan reconcile's generation gate
can miss one of the two changes — the exact class of bug R0-A's whole
counter-merge fix exists to prevent, reopened here because this code path
was never routed through it.

Secondary effect: cascade's direct drops also bypass the write-barrier bit
entirely, reopening the "non-tx writer with a pre-retire snapshot writes a
posting after sweep" window (P0-3b class) specifically for the DROP TABLE
path.

## The fix

Wrap each of the two call sites in ONE `begin_write_barrier` acquisition
covering its ENTIRE block of index mutations — not four separate
acquisitions (that would serialize against itself and add nothing; one
scope, held for the whole cascade/repair block, is correct and matches how
`create_index_v2` etc. already hold their guard for their full critical
section).

- `begin_write_barrier` is `pub` (`crates/shamir-engine/src/table/table_manager.rs:979`)
  and returns `(WriteBarrierGuard, OwnedMutexGuard<()>)` — hold both for the
  duration of the block (`let (_barrier, _uwl_guard) = table.begin_write_barrier(bit).await;`
  then do the drops, guard drops at end of scope).
- Confirm `IndexManager::drop_index`/`drop_unique_index` and
  `SortedIndexManager::drop_index`/`register` do NOT themselves try to
  acquire `ddl_admission` or `unique_write_lock` internally (they shouldn't —
  they're the raw primitives the `TableManager` wrappers call INTO after
  already holding the barrier) — verify this before wrapping, to rule out
  a re-entrant deadlock on the non-reentrant `tokio::sync::Mutex`.
- **Which bit to raise**: this touches all four families in one block. Check
  `crate::index::write_barrier_flags` for the existing bit constants
  (`REGULAR_INDEX_CREATE`, `UNIQUE_INDEX_CREATE`, `SORTED_INDEX_CREATE`,
  `INDEX2_CREATE`). Since the goal here is primarily REGISTRY-MUTATION
  SERIALIZATION (the `ddl_admission` mutex itself, which is the SAME single
  mutex regardless of which bit is passed to `begin_write_barrier` — see
  `begin_write_barrier`'s own doc, Step 0 acquires `ddl_admission` before
  touching any bit), a single `begin_write_barrier` call with any ONE
  existing bit is sufficient to get the admission serialization — the bit
  itself only affects which writers see the slow path via
  `needs_write_barrier(bit)`. Given DROP TABLE CASCADE is about to delete
  the whole table immediately after (writers targeting it will fail once
  `remove_table` runs regardless), and given `doctor::repair()`'s self-heal
  is already an existing, accepted maintenance operation — prefer NOT
  inventing a new bit; pick whichever single existing bit reads most
  naturally for "a DDL-shaped table-wide index mutation is in flight" and
  say which one you chose and why in your report. If you find a concrete
  reason multiple bits must be raised (e.g. a writer path checks bits
  independently and DROP TABLE genuinely needs to block writers on all
  four family paths simultaneously), raise multiple bits by holding
  multiple `begin_write_barrier` guards in the same scope rather than
  picking one arbitrarily — but justify it, don't default to it.
- **Do NOT fix the `let _ = ...` error-swallowing** in the same lines (both
  cascade and repair discard errors from these drop calls) — that changes
  user-visible failure semantics for DROP TABLE and is explicitly out of
  scope for this brief (noted by the review as a follow-on, not part of
  F-3's admission-bypass fix).

## Tests

- A test proving `DROP TABLE ... CASCADE` acquires `ddl_admission` for its
  index-mutation block — mirror the pattern R0-A's
  `drop_sorted_index_acquires_write_barrier`/`drop_index2_acquires_write_barrier`
  tests already use (hold `unique_write_lock` externally, spawn the cascade
  drop, assert it blocks until released — see
  `crates/shamir-engine/src/table/tests/r0a_registry_watermark_admission_tests.rs`
  for the exact shape to mirror).
- Same for `doctor::repair()`'s self-heal block.
- If you can construct a deterministic test reproducing the concrete
  registry-generation race scenario above (concurrent CASCADE + CREATE),
  write it — but the admission-acquisition proof tests above are the
  primary requirement; don't block on constructing the harder race test if
  it needs more scaffolding than the brief anticipated.

## Constraints

- Follow `CLAUDE.md` conventions (tests under existing `tests/` directories,
  no inline `#[cfg(test)] mod tests {}`).
- Gate: `cargo fmt -p shamir-db -p shamir-engine`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-db -p shamir-engine --full`, and
  `./scripts/test.sh @oracle` must all be clean.
- Do NOT touch F-4 (#1029, already fixed) or F-5..F-8 (#1031, separate
  task). Do NOT fix the `let _ =` error-swallowing (see above — explicitly
  deferred).
- This should be a contained fix (~15-30 lines plus tests per the review's
  own estimate). If it grows much larger, stop and report why.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `DROP TABLE ... CASCADE`'s index-mutation block holds `ddl_admission`
      (via `begin_write_barrier`) for its entire duration.
- [ ] `doctor::repair()`'s self-heal block does the same.
- [ ] No re-entrant deadlock introduced (verified: the raw manager
      primitives called inside the barrier don't themselves try to acquire
      admission/unique_write_lock).
- [ ] Admission-acquisition proof tests for both sites, confirmed to fail
      (not block) against the pre-fix code.
- [ ] `let _ = ...` error-swallowing left untouched (explicitly out of
      scope).
- [ ] fmt/clippy/tests green (report exact commands and pass/fail).
