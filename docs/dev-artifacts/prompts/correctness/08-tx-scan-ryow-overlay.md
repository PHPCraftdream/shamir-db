# FG-3: Tx-scan read-your-own-writes — full overlay of the staged write_set

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## DECIDED CONTRACT (user, 2026-07-21) — do not re-litigate

Full overlay (not reject-scan, not document-only) — the largest of the
options considered, but the only semantically-correct one. This is engine
core, so the architecture below was investigated and decided by the
orchestrator BEFORE this brief was written (per the user's explicit
"это ядро engine — сначала investigate" instruction) — implement exactly
this, do not re-derive or redesign it.

## Context — already investigated exhaustively, do not re-derive

### The gap (review 2026-07-21 P0#1, confirmed)

`crates/shamir-engine/src/table/tests/stream_tx_tests.rs`'s
`list_stream_tx_does_not_see_staged_insert` test and
`table_manager_streaming.rs`'s "KNOWN LIMITATION" doc comment on
`list_stream_tx` (~line 165) both confirm: streaming scans (`list_stream_tx`,
`filter_stream_tx`) do NOT overlay the tx's own `write_set` — a record
staged (inserted/updated/deleted) inside a tx is invisible to an in-tx
stream until commit. Only point reads (`read_one_tx`) currently do
read-your-own-writes (RYOW).

**Second, equally real gap found during investigation (residual of task
#729), NOT limited to the streaming-read API:** `execute_update_tx` and
`execute_delete_tx` (`crates/shamir-engine/src/table/write_exec.rs`) each
have their OWN bespoke inline match-time scan — NEITHER goes through
`list_stream_tx`/`filter_stream_tx` at all. Each has two arms:
- `execute_update_tx` (~line 469): `lookup_records_via_index(filter, ctx)`
  (index-path) OR a manual `self.list_stream(batch_size)` +
  `compile_filter`/`callback.matches` loop (fallback path) — see ~line
  469-520.
- `execute_delete_tx` (~line 795): the SAME two-arm shape
  (`lookup_records_via_index` / manual `list_stream` + filter loop) for
  its own `to_delete` collection — see ~line 795-840.

Both are COMMITTED-store-only scans. **Consequence:** an `UPDATE`/`DELETE`
inside a tx whose `WHERE` clause should match a row THIS SAME TX JUST
INSERTED (but hasn't committed yet) will NOT match it — the row is
invisible to the match scan even though `read_one_tx` would see it fine.
(Note: `execute_update_tx` ALREADY has excellent staged-value merge logic
for rows the match-scan DID find — see its per-row loop ~line 580-620,
which correctly resolves an already-matched row's PRIOR staged mutation
before applying this op's new one. That logic is NOT the gap; the gap is
specifically that a purely-staged-inserted row never enters `matched` in
the first place.)

### The reusable pattern — VERIFIED to exist, use it

`crates/shamir-storage/src/storage_membuffer.rs`'s `merge_overlay_stream`
(~line 653, private `fn`, task #530) is a real, working 2-way sorted merge
between a sorted dirty-overlay snapshot and a sorted `inner` stream:
overlay wins on key match (`Slot::Live` → emit overlay value,
`Slot::Tombstone` → exclude the key even if `inner` still has stale data),
overlay-only keys are interleaved in sorted position (including a tail
flush for overlay keys sorting after the last inner key). Read this
function in full before starting — it is your algorithm template. You
CANNOT call it directly (it's private, operates on raw
`RecordKey`/`Bytes`/`Slot` in a different crate) — you must write an
ENGINE-level analog with the same algorithm, operating on
`(RecordId, RecordCow)` instead, sourced from a tx's `write_set` instead
of a `MemBufferStore`'s dirty buffer.

### The overlay source (VERIFIED, already exists)

`shamir_tx::staging_store::StagingStore` (`crates/shamir-tx/src/staging_store.rs`)
already exposes what you need:
- `snapshot_ops(&self) -> Vec<KvOp>` — every staged op for this table
  (`KvOp::Set(RecordKey, Bytes)` / `KvOp::Remove(RecordKey)`).
- `keys(&self) -> impl Iterator<Item = &RecordKey>` and
  `staged_op(&self, key: &[u8]) -> Option<StagedKind>` (per-key probe,
  already used by `read_one_tx`/the per-row merge loops above).
- `RecordKey` (= `KeyBytes`) already implements `Ord` — sortable.

Access pattern (mirrors `read_one_tx`'s existing per-key probe):
`tx.write_set.get(&self.table_token())` → `Option<&StagingStore>`. When
`None` (this tx never wrote this table), the overlay is empty — the merge
must be a zero-cost pass-through in that case (mirror `record_scan_reads`'s
existing "gate on Serializable before any work" cost-discipline for the
analogous "gate on `tx.write_set` non-empty before any work" case here).

## The task — Part 1: the shared overlay-merge primitive

Create a new function (new file, e.g.
`crates/shamir-engine/src/table/tx_scan_overlay.rs`, or a sibling module —
your call on exact placement) implementing the SAME 2-way sorted merge
algorithm as `merge_overlay_stream`, adapted to this crate's types:

```rust
/// Merge `inner` (a committed-store record stream, key-sorted ascending)
/// with `overlay` (this tx's staged ops for the table, pre-sorted by key)
/// so a staged Set overrides/injects, a staged Remove hides, and rows with
/// no staged op pass through unchanged. Mirrors
/// `shamir_storage::storage_membuffer::merge_overlay_stream`'s algorithm
/// exactly (task #530) — same tombstone-masking, same overlay-only-tail
/// handling, same ascending-order preservation.
fn merge_stream_with_tx_overlay<'a>(
    inner: impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a,
    overlay: Vec<(RecordId, StagedRowOverlay)>, // Set(Bytes) | Removed — pre-sorted by RecordId
    batch_size: usize,
) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>> + 'a
```

Build the sorted `overlay` input from `tx.write_set.get(&token)`'s
`snapshot_ops()` (convert `KvOp::Set(k, v)`/`KvOp::Remove(k)` into
`(RecordId::from_bytes(k)?, Set(v)/Removed)` pairs, sort by `RecordId`) —
find the right `RecordId`↔`RecordKey`/bytes conversion helpers already
used elsewhere in this file (e.g. `id.to_bytes()`/`RecordId::from_bytes`
used throughout `write_exec.rs` and `table_manager_streaming.rs`) rather
than inventing new ones. A staged `Set(bytes)` overlay row becomes
`RecordCow::Borrowed(bytes)` in the merged output (zero-copy, no decode —
matches the existing `RecordCow::Borrowed` convention for raw storage
bytes).

**Filtered-scan variant (MANDATORY, do not skip):** for any caller that
also has a `Filter`/`FilterContext` (i.e. every integration point below
except plain `list_stream_tx`), overlay-sourced rows (both purely-staged
inserts and staged-overridden updates) MUST be evaluated against the SAME
filter before being yielded — a staged row that does not match the filter
must be EXCLUDED, even though the raw merge would otherwise include it (a
staged UPDATE's new value may match/not-match the filter differently than
the old committed value did; a staged INSERT was never filtered at all
since it never went through a scan before). Design this as a parameter
(an optional filter-match closure, or two separate merge functions — your
call on the cleanest shape) rather than skipping filtering for overlay
rows.

## The task — Part 2: wire it into every match-scan (6 integration points)

1. **`list_stream_tx`** (`table_manager_streaming.rs`) — after the
   existing `record_scan_reads` wrap (leave that SSI-recording logic
   completely UNCHANGED — it already correctly records read-set entries
   from the raw committed stream; the overlay merge is a SEPARATE,
   downstream concern), merge in the tx's overlay for this table
   (unfiltered pass-through variant). Update the "KNOWN LIMITATION" doc
   comment to describe the NEW behavior instead of the old limitation.
2. **`filter_stream_tx`** (`table_manager_streaming.rs`) — same ordering
   (after `record_scan_reads`), using the FILTERED overlay variant (Part
   1's mandatory filter-match requirement) so injected/overridden staged
   rows are correctly filtered, not blindly included.
3. **`execute_update_tx`'s match-scan, list_stream fallback arm**
   (`write_exec.rs` ~line 485-520) — merge the tx overlay into this
   inline scan (filtered variant), so a staged-inserted row satisfying
   `filter` enters `matched`.
4. **`execute_update_tx`'s match-scan, index-path arm**
   (`lookup_records_via_index`, ~line 470-484) — the index itself does
   NOT contain staged-only rows (they were never indexed, since indexing
   happens at commit/stage-apply time — verify this assumption is
   correct by reading `lookup_records_via_index`'s implementation before
   proceeding). Decide and implement one of: (a) after the index lookup,
   ALSO scan the tx's overlay for keys matching `filter` that the index
   lookup didn't already return, and merge them in; or (b) if the index
   path cannot cheaply be made overlay-aware, fall back to the full
   `list_stream` + filter path (arm 3 above) whenever `tx.write_set`
   has ANY staged rows for this table (accept the perf cost of losing
   the index fast-path only for the rare case of an in-tx write followed
   by an in-tx read/write on the same table — document this trade-off
   explicitly if you take this route, and gate it so a plain
   `tx.write_set.get(token).is_none()` still takes the fast index path
   unconditionally). Prefer (a) if it is genuinely straightforward once
   you've read `lookup_records_via_index`; fall back to (b) and document
   the trade-off if not — this decision is yours to make, but it must not
   be silently skipped.
5. **`execute_delete_tx`'s match-scan, both arms** (`write_exec.rs`
   ~line 795-840) — same treatment as items 3-4, symmetric for delete.
   Given how similar `execute_update_tx`'s and `execute_delete_tx`'s
   match-scan blocks already look, consider factoring the "scan +
   overlay-merge + filter" logic into ONE shared private helper both call
   — this is a suggested (not mandatory) refactor to avoid a 4th near-
   duplicate of the same ~50-line block; use your judgment on whether
   it's a net win given the rest of this task's scope.
6. **Cross-table validator reads** (`run_validators_qv`/`run_validators_view`
   in `table_manager_validators.rs`, per the actor-threading work in an
   earlier task this campaign, RI-7) — a `unique`/`foreign_key` validator
   reads a (possibly different) table to check a constraint. Investigate
   whether these validator reads route through `list_stream_tx`/
   `filter_stream_tx` (in which case items 1-2's fix already covers them)
   or through a separate scan path that also needs the same treatment.
   Report which is the case in your summary; fix it if it's the latter.

## Tests (MANDATORY — every scenario named by the task, all levels)

1. **FLIP** `list_stream_tx_does_not_see_staged_insert`
   (`stream_tx_tests.rs`) — per its own doc comment, this test MUST flip:
   the staged insert becomes visible, `list_stream_tx` yields `n + 1`.
   Rename the test to reflect the new behavior (e.g.
   `list_stream_tx_sees_staged_insert`) and update its doc comment
   accordingly — do not leave a stale name/comment describing removed
   behavior.
2. **Staged update visible with staged bytes** — a record updated (not
   just inserted) inside a tx shows the STAGED (new) bytes in an in-tx
   stream, not the committed (old) bytes.
3. **Staged delete hidden** — a record deleted inside a tx is ABSENT from
   an in-tx stream, even though it's still present in the committed
   store.
4. **UPDATE matches a staged insert** (the #729-residual regression) — in
   one tx: insert a row, then run an `UPDATE ... WHERE` that should match
   the just-inserted row; assert it actually updates (not a no-op/0
   affected). Cover BOTH the index-path and list_stream-fallback arms
   (construct the test so the WHERE clause exercises whichever path your
   implementation of item 4 above takes — if you implemented option (a),
   test the index arm directly with a sorted-indexed field in the WHERE;
   if you implemented option (b), confirm the fallback triggers and
   still matches correctly).
5. **DELETE matches a staged insert** — symmetric to test 4.
6. **Unique-constraint validator sees staged rows** — a `unique` validator
   bound to a table; within one tx, insert row A (value "x"), then
   attempt to insert row B ALSO with value "x" in the SAME tx (before
   commit) — the validator must catch the in-tx duplicate (reject B),
   proving cross-row uniqueness checks see the tx's own staged data, not
   just the committed snapshot.
7. **Rust e2e** (through the real server, `crates/shamir-server/tests/`)
   — an interactive/transactional batch: `insert` then `find`/`query` in
   the SAME batch/tx, assert the inserted row is visible before commit.
8. **TS e2e** — mirror test 7 through the TS SDK's interactive-tx surface
   (follow the `describe.skipIf(!SERVER_AVAILABLE)` convention from
   `e2e-harness.ts`).
9. **Isolation regression (MANDATORY — do not skip)**: a CONCURRENT test
   proving tx isolation is preserved — tx A stages an insert (not yet
   committed); a DIFFERENT, concurrent tx B's stream/scan does NOT see
   tx A's staged-but-uncommitted row (only tx A's OWN stream sees its own
   staged overlay). This is the single most important regression to get
   right — a bug here would leak uncommitted data across transactions.
10. Confirm the existing `a3_record_scan_reads_records_snapshot_version_not_current_after_concurrent_commit`
    test (same file, exercises the REAL scan path's SSI read-set
    recording) still passes unmodified — this proves the overlay-merge
    change did not disturb the pre-existing SSI recording logic (which,
    per Part 2 item 1's instruction, should be completely untouched).

## Docs

- Update `table_manager_streaming.rs`'s "KNOWN LIMITATION" doc comments
  on `list_stream_tx`/`filter_stream_tx` to describe the NEW
  read-your-own-writes behavior (keep the existing, still-accurate
  "streaming-scan SSI scope" paragraph about predicate/range locks being
  out of scope — that is a SEPARATE, still-true limitation, do not touch
  it).
- `CHANGELOG.md`: one `[Unreleased]` bullet — this closes a real
  correctness gap (P0#1), not a breaking change (in-tx reads simply
  become MORE correct; no existing caller could have been relying on the
  old blind-to-own-writes behavior as a feature).

## Out of scope

- Full SSI predicate/range locking over streams (phantom-insert detection
  for a concurrent OTHER tx's insert into a scanned range) — this is a
  SEPARATE, already-documented, genuinely harder problem; this task is
  only about a tx seeing its OWN staged writes, not phantom protection
  against OTHER transactions.
- Any change to `record_scan_reads`'s existing SSI-recording behavior —
  leave it completely untouched; the overlay merge is a downstream,
  independent concern (see Part 2 item 1).
- `AsOf`/`History` temporal reads (`read_temporal.rs`) — these are
  point-in-time historical views, not "current committed state"; tx
  overlay semantics don't apply the same way and are explicitly not part
  of this task.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @oracle --full` green (this repo's scope alias for
  tx+engine).
- `./scripts/test.sh @e2e` green (this repo's scope alias covering
  shamir-db + shamir-server, forces `--full`).
- TS tests (`npm test` in `crates/shamir-client-ts`) pass.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above, which of the two
  options you took for item 4 (index-path overlay-awareness vs.
  fallback-to-full-scan) and why, whether you found item 6's validator
  reads already covered or needing a separate fix, and the exhaustive
  list of every match-scan site you touched.

If, after real investigation, some sub-part proves structurally harder
than described here (mirroring FG-1's and FG-2's honest "STOP and
report" outcomes for their own mandatory verifications) — do not force a
broken or silently-incomplete implementation. Report the exact structural
blocker precisely so it can be triaged, rather than papering over a gap.
