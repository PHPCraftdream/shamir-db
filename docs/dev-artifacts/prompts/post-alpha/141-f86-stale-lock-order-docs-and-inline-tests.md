# F-86 (#914) — fix stale F-70-contradicting lock-order docs + relocate write_barrier_flags.rs inline tests

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

This is a finding from an `@oh` adversarial review of the F-69..F-81
remediation wave (see `docs/checkpoints/p0-p1-wave-complete.md`, task
#914). Two independent issues in the same general area, unrelated to each
other except by proximity — fix both in this task.

## Part (a) — stale lock-order doc comments

F-70 (#897, commit not needed here — read
`crates/shamir-engine/src/table/writer_drain_barrier.rs`'s "THE canonical
lock-order hierarchy" module doc, and `TableManager::begin_write_barrier`
at `table_manager.rs:763-778`, for the authoritative current order)
established a canonical **drain-then-lock** order for every DDL barrier
acquisition:

```rust
pub async fn begin_write_barrier(&self, bit: u8) -> (...) {
    // Step 1: raise the intent bit FIRST.
    let guard = WriteBarrierGuard::set(self.write_barrier_flags.clone(), bit);
    // Step 2: drain every writer that read the flag as false before step 1.
    self.drain_writers().await;
    // Step 3: ONLY NOW acquire the lock.
    let lock_guard = self.unique_write_lock.clone().lock_owned().await;
    (guard, lock_guard)
}
```

This replaced the OLDER **lock-then-drain** order that F-57 (#883) had
wired into every DDL create path, which F-70 proved genuinely deadlocks
against the tx-commit path's own drain-guard-then-lock shape (see the
`writer_drain_barrier.rs` module doc for the full 3-party deadlock
derivation).

Two doc comments elsewhere in `table_manager.rs` still teach the OLD,
now-dangerous order and were never updated when F-70 landed:

1. **`drain_writers`'s doc** (`table_manager.rs:811-826`, currently reads):
   > "The caller MUST have already (1) raised its intent bit ... so NEW
   > writers take the slow (locked) path, and (2) **hold
   > `unique_write_lock`** so slow-path writers are blocked."

   This states the EXACT OPPOSITE precondition from
   `begin_write_barrier`'s actual Step 2/Step 3 order (drain happens
   BEFORE the lock is acquired, not after). A future caller who read only
   THIS doc and followed it literally (raise bit → acquire lock → THEN
   call `drain_writers`) would reintroduce the F-70 deadlock shape. Correct
   this to state the ACTUAL precondition/order, cross-referencing
   `begin_write_barrier` as the canonical entry point production code
   should use instead of calling `drain_writers` directly.

2. **`set_schema_activation_barrier`'s doc** (`table_manager.rs:780-806`,
   currently reads): "**Callers MUST set/clear this while holding
   `unique_write_lock`**" — this is the OLD lock-then-set discipline.
   The doc already partially acknowledges F-70 exists ("production callers
   use `begin_write_barrier` (F-70, #897), which now also reorders the
   lock acquisition to AFTER the drain") but its OWN primary stated
   contract for this raw setter still teaches the stale order as if it
   were still the right thing to do. Since this raw setter is
   test-only-direct-use today (per its own doc, production goes through
   `begin_write_barrier`), correct its doc to state clearly: this raw
   setter is for tests that exercise the flag transition directly, is NOT
   the production entry point, and does NOT need to be called under the
   lock — `begin_write_barrier` is the one true production sequence and
   this doc should point there rather than re-teach a superseded discipline
   as if it were current guidance.

**Do not change any actual code/behavior in this part** — this is a
doc-only correction. Re-read `writer_drain_barrier.rs`'s canonical-order
doc section carefully and make these two comments AGREE with it and with
each other, cross-referencing rather than duplicating the full derivation.

## Part (b) — relocate write_barrier_flags.rs's inline tests

`crates/shamir-index/src/legacy/write_barrier_flags.rs` (new file, added
this wave for F-69/#896) embeds `#[cfg(test)] mod tests { ... }` inline
(currently lines ~237-317), violating CLAUDE.md's rule: "Never embed
`#[cfg(test)] mod tests { ... }` inline inside implementation files —
move them to the tests/ directory." This is a FRESH violation since the
file is new this wave, not inherited legacy debt.

Relocate to `crates/shamir-index/src/legacy/tests/write_barrier_flags_tests.rs`,
following this crate's existing convention (see
`crates/shamir-index/src/legacy/tests/mod.rs` — a flat list of
`pub mod <name>_tests;` re-exports, no test code itself). Add
`pub mod write_barrier_flags_tests;` to that `mod.rs`. The relocated file
needs its own `use super::super::write_barrier_flags::*;` (or the
equivalent correct path — check how sibling files like
`index_definition_tests.rs` import their subject module) instead of the
original `use super::*;` (which only worked because the tests were
physically inside the same file).

Remove the `#[cfg(test)] mod tests { ... }` block from
`write_barrier_flags.rs` entirely once relocated — do not leave a stub or
duplicate.

## Definition of done

- `drain_writers`'s and `set_schema_activation_barrier`'s doc comments in
  `table_manager.rs` state the CURRENT (F-70) canonical drain-then-lock
  order accurately, cross-referencing `begin_write_barrier` rather than
  re-teaching a superseded lock-then-drain discipline.
- `write_barrier_flags.rs`'s 8 tests relocated to
  `crates/shamir-index/src/legacy/tests/write_barrier_flags_tests.rs`,
  wired into `tests/mod.rs`, passing unchanged (same assertions, same
  test names — this is a pure relocation, not a rewrite).
- `cargo fmt -p shamir-engine -p shamir-index -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine -p shamir-index --full` green —
  confirm the relocated tests specifically appear and pass by name in the
  test output (grep for `write_barrier_flags_tests::`), don't just trust
  an aggregate PASS count.
- No behavior change anywhere — this task is docs + test relocation only.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
