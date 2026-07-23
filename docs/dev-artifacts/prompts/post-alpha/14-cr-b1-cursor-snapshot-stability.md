# Brief: CR-B1 — cursor snapshot stability vs. concurrent DELETE (#767)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — SILENT DATA LOSS, verified against the current tree 2026-07-23

`TableManager::read_as_of` (`crates/shamir-engine/src/table/read_temporal.rs:85`,
the read path every cursor `FetchNext` uses) enumerates candidate ids via
`self.list_stream(FULL_SCAN_BATCH)` (`read_temporal.rs:85`), which routes to
`MvccStore::current_stream` (`crates/shamir-engine/src/table/table_manager_streaming.rs:43`).
`current_stream`'s implementation
(`crates/shamir-tx/src/mvcc_store/mod.rs:1145-1224`, grouping logic in
`crates/shamir-tx/src/mvcc_store/version_entry.rs:97-273`) explicitly
**filters out any key whose CURRENT winner is a tombstone**
(`version_entry.rs:118-120`: `if !winner.is_empty() { out_batch.push(...) }`).

Consequence: a row alive at the cursor's pinned MVCC version, but DELETEd by
a concurrent writer between two `FetchNext` calls, is no longer yielded by
`current_stream` at all — `read_as_of` never even attempts a `get_at` call
for it, so it silently vanishes from every subsequent page. On the
no-ORDER-BY offset-bookmark path this ALSO shifts every subsequent offset
by one, dropping an unrelated second row too. The only existing
snapshot-stability test (`cursor_handler_tests.rs::
cursor_does_not_observe_a_write_committed_after_creation`) covers concurrent
INSERT, not DELETE — the DELETE case has no test today and is the R-1
finding from the `@fx` review
(`docs/dev-artifacts/research/2026-07-23-wave-review-followup.md`).

## Why this is fixable without a fundamentally new mechanism — investigation summary

The MVCC GC floor (`min_alive`, computed in
`crates/shamir-tx/src/repo_tx_gate.rs:578-605` from the smallest version held
by any active snapshot) already guarantees that any version `>= min_alive`
survives `gc_below`/`vacuum_key`'s reclamation check
(`crates/shamir-tx/src/mvcc_gc.rs`, the `if *version >= min_alive { continue;
}` guards). A cursor's pinned snapshot (`gate.open_snapshot()` at
`crates/shamir-server/src/db_handler/cursor_handlers.rs:669`) is a live
`SnapshotGuard`, which registers into `active_snapshots` — so as long as the
cursor stays open, `min_alive <= cursor's pinned version`, and the
pre-delete value at that version is guaranteed retrievable via
`mvcc.get_at(id, pinned_version)` (`read_temporal.rs:98`) **even after the
key's current winner becomes a tombstone.** The bug is purely in
**enumeration** (which ids get a `get_at` call attempted at all), not in
`get_at` itself, and not in GC correctness.

## Fix — add a tombstone-inclusive enumeration variant, used ONLY by `read_as_of`

**Do not change `current_stream`'s existing behavior or any of its other
callers** (`table_manager_sorted_index.rs`, `table_manager_replication.rs`,
`migration/coordinator.rs`, the normal `Latest` read path via
`TableManager::list_stream` at `table.rs:199`/`table_manager_streaming.rs:33`
— all of these correctly want tombstones filtered, since they only care
about CURRENT state). Add a new, narrowly-scoped variant instead:

1. In `crates/shamir-tx/src/mvcc_store/mod.rs`, add a new method alongside
   `current_stream` (e.g. `current_stream_with_tombstones` — name it
   whatever reads clearly next to the existing method) that reuses the SAME
   streaming group-by logic (`version_entry.rs`'s `StreamingGroupByState`/
   `drain_and_emit`/`flush_group`) but **omits the `if !winner.is_empty()`
   filter** at the tombstone-suppression point (`version_entry.rs:118-120`)
   — emit the winner regardless of whether it's a tombstone. Check whether
   the cleanest way to thread this through is a boolean parameter on the
   shared grouping helper, or a thin wrapper — follow whatever the existing
   code's own idiom is (look at how `current_stream` itself is structured
   before choosing).
2. Add a corresponding `TableManager` wrapper next to `list_stream`
   (`table_manager_streaming.rs:33`), e.g.
   `list_stream_with_tombstones(batch_size)`, that calls the new
   `MvccStore` method instead of `current_stream`. Keep `list_stream`
   itself completely untouched.
3. In `read_as_of` (`read_temporal.rs:85`), switch the enumeration source
   from `self.list_stream(FULL_SCAN_BATCH)` to the new
   `self.list_stream_with_tombstones(FULL_SCAN_BATCH)`. **No other change
   needed in the loop body** — the existing `mvcc.get_at(id.as_bytes(),
   version).await?` call (line 98) already correctly returns `None` for an
   id that never existed at `version` (excluded, `continue`) and `Some(bytes)`
   for one that did — a tombstoned-now key just means more ids get a
   `get_at` attempt, some of which will legitimately return `Some` for a
   pre-delete-as-of-pinned-version value.
4. **Performance note (expected, not a regression to chase):** this makes
   `read_as_of`'s enumeration set slightly larger on a table with deletes
   (each dead-but-not-yet-GC'd key gets a `get_at` attempt instead of being
   skipped). This is the SAME O(table) cost class the whole AsOf/cursor path
   already accepts (see the R-6 cost-model note just added to
   `CURSORS.md` by CR-A7) — do not try to optimize this away in this task;
   a batched-lookup optimization is separately tracked as CR-C3/#778
   (blocked on this task landing first).

## Tests (TDD — write failing tests first)

In whatever test module covers `read_as_of` today
(`crates/shamir-engine/src/table/tests/` — find the existing AsOf test file,
e.g. search for `read_as_of` or `Temporal::AsOf` test coverage) AND in
`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- **Engine-level regression** (`shamir-engine`): pin a version via a live
  snapshot/read-at-version, delete a row that existed at that version via a
  separate write, then `read_as_of` at the pinned version — the deleted
  row must still appear (this test must FAIL against the current code
  before the fix, confirming it reproduces the bug).
- **Cursor-level delete-mid-scroll** (`shamir-server`): open a cursor over N
  rows (small `page_size` so multiple `FetchNext` calls are needed), delete
  a row that has NOT yet been fetched via a separate batch/connection,
  drain the rest of the cursor — every row alive at the cursor's pinned
  version must appear exactly once across all pages (this must also FAIL
  before the fix).
- **Update-mid-scroll companion** (should already pass, prove it doesn't
  regress): update (not delete) a not-yet-fetched row mid-scroll — the
  cursor must keep returning the row's PINNED-version value, not the new
  one, across the rest of its pages.
- **Regression guard**: the existing INSERT-mid-scroll snapshot-stability
  test (`cursor_does_not_observe_a_write_committed_after_creation`) and all
  other existing `read_as_of`/cursor tests must stay green — this fix must
  not change behavior for the no-concurrent-delete case, and must not
  change `current_stream`'s behavior for any of ITS other callers (spot
  check: run the sorted-index and migration-coordinator test suites too).

## Docs follow-up (small, do NOT skip)

`docs/guide-docs/KNOWN_LIMITATIONS.md`'s "A cursor's 'stable snapshot' can
still be disturbed by a concurrent DELETE" bullet (added by CR-A7, search
for that exact heading) describes the bug THIS task fixes. Once the fix
lands and the new tests pass, **remove that bullet** (or replace it with a
note that it's now fixed, if you'd rather leave a short changelog-style
trace — your call, but the limitation itself must not still read as
present-tense-true). Also check `CURSORS.md`'s status blockquote (added by
CR-A7) — it doesn't currently mention this caveat by name, so likely no
change needed there, but skim it in case a cross-reference needs updating.

## Gate

```
cargo fmt -p shamir-engine -p shamir-server -- --check
cargo clippy -p shamir-engine -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-engine`
(`crates/shamir-tx/src/mvcc_store/mod.rs`,
`crates/shamir-tx/src/mvcc_store/version_entry.rs`,
`crates/shamir-engine/src/table/table_manager_streaming.rs`,
`crates/shamir-engine/src/table/read_temporal.rs`), plus test files in both
`shamir-engine` and `shamir-server`, plus the two docs files named above.
Do NOT touch `crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
pagination/tie-breaker logic (CR-A4's territory) — this task only changes
WHAT enumeration source `read_as_of` reads from, not how cursor pagination
slices the results.
