# Brief 51 — #1056: MvccStore::snapshot_stream(batch, at_version)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

Slice 1 of the online CREATE INDEX redesign (RFC v2 approved by the
operator on 2026-08-09, `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`,
§2.2). This is the first code slice — narrow and self-contained.

`crates/shamir-tx/src/mvcc_store/mod.rs::current_stream_impl` (`:1347-1372`)
already implements a version-pinned scan, but it computes the pin internally:
`let floor = self.gate.last_committed();` (`:1356`), then threads `floor`
into the group-by state machine (filters "newest version ≤ floor") and into
`self.overlay.snapshot_le(floor)` (`:1367-1372`).

## Task

1. Add `pub fn snapshot_stream(&self, batch: usize, at_version: u64) -> impl
   Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send` — same body as
   `current_stream_impl`, but the floor comes from the `at_version`
   parameter instead of `self.gate.last_committed()`.
2. `current_stream(batch)` and `current_stream_with_tombstones(batch)`
   (`:1307-1339`) keep their exact current external behavior — turn them
   into thin wrappers calling the new primitive with
   `at_version = self.gate.last_committed()`.
3. **Tombstone semantics — resolve this explicitly, don't guess.** Read
   `current_stream_with_tombstones`'s doc comment (`:1314-1333`) — it exists
   because `read_as_of`'s AsOf enumeration needs to see a tombstoned winner
   (so it can still attempt `get_at(id, pinned_version)` for the pre-delete
   value). For online CREATE INDEX's Phase A backfill, the opposite is
   needed: a row deleted at or before the pinned version must NOT produce a
   posting. Determine whether `snapshot_stream` should default to the
   tombstone-SUPPRESSING variant (like plain `current_stream`) for its
   primary use, or whether it needs its own `include_tombstones` parameter
   mirroring `current_stream_impl`'s existing `bool` flag. Whichever you
   choose, write a one-line doc comment on `snapshot_stream` stating the
   choice and why — this decision affects Phase A's correctness in slice 1d
   (#1059), so don't leave it implicit.

## Tests (TDD — write failing first, per this repo's protocol)

Add to the existing `mvcc_store` test module (check
`crates/shamir-tx/src/tests/mvcc_store_tests/` — one file per topic per
this repo's test-organisation convention; add a new file if none fits,
e.g. `snapshot_stream_tests.rs`, wired into `tests/mod.rs`):

1. **Pin excludes post-pin writes.** Write N rows, capture
   `gate.last_committed()` as the pin, write M more rows, call
   `snapshot_stream(batch, pin)` → exactly N rows returned, not N+M.
2. **Pin sees the value AS OF the pin, not current.** Write a row v1,
   capture the pin, update the row to v2, `snapshot_stream(batch, pin)` →
   returns v1's bytes, not v2's.
3. **Equivalence with `current_stream`.** On the same fixture,
   `snapshot_stream(b, gate.last_committed())` produces byte-identical
   output to `current_stream(b)` — same rows, same order, same values.
4. **Overlay branch.** A scenario where a key's winner is in the overlay
   (not yet drained to history) rather than in `history` — see
   `snapshot_le`'s existing usage at `:1367-1372` for how to construct this
   in a test (check existing overlay tests in this crate for the exact
   setup pattern, e.g. `crates/shamir-tx/src/tests/` for
   `snapshot_le`/`gc_upto` usage). Test with a pin BEFORE the overlay write
   (row excluded) and a pin AFTER (row included, value from overlay).

## Boundaries — do not exceed scope

- This change is isolated to `shamir-tx`. No existing caller's behavior
  changes.
- Do NOT touch `get_at`/`get_at_many` — unrelated, point-lookup primitives.
- Do NOT touch anything outside `crates/shamir-tx/src/mvcc_store/`.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-tx
```

Report the exact diff and the exact test names added, plus confirm all 4
required tests are present and pass — paste the actual nextest output for
the new test file, not a paraphrase.
