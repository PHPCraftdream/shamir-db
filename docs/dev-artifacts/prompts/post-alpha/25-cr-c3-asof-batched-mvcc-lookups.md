# Brief: CR-C3 — `AsOf` read path: batch per-record MVCC lookups (#778)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — verified against the current tree 2026-07-23

`TableManager::read_as_of` (`crates/shamir-engine/src/table/read_temporal.rs`,
~lines 85-123 — re-verify exact lines, CR-B1 touched this function
recently) enumerates candidate ids via a stream of batches
(`FULL_SCAN_BATCH`-sized), and for EACH id inside each batch individually
awaits `mvcc.get_at(id.as_bytes(), version).await?` one at a time:

```rust
for (id, _cow) in batch {
    let asof_bytes = mvcc.get_at(id.as_bytes(), version).await?;
    ...
}
```

`MvccStore::get_at` (`crates/shamir-tx/src/mvcc_store/mod.rs`, ~lines
1046-1049) delegates to `resolve_read`, whose common "direct path" (the
version pinned by the snapshot hasn't been superseded by a concurrent
write — `cur_v > 0 && cur_v <= snapshot_version`, ~lines 659-678) does ONE
`self.history.get(encode_version_key(key, cur_v)).await` per call — a
single-key point lookup against the underlying `Arc<dyn Store>`. This
means every cursor `FetchNext`/`CreateCursor` page pays N SEQUENTIAL
single-key awaits over the WHOLE table (not just the page — `read_as_of`
must scan and filter the entire matched set before it can sort/paginate).

**A batched multi-get surface already exists and is unused here.**
`shamir-storage`'s `Store` trait (`crates/shamir-storage/src/types.rs`,
~lines 44-53) has `get_many(&self, keys: Vec<RecordKey>) ->
DbResult<Vec<Option<Bytes>>>` — "Vectored read — fetch many records in one
logical call... Disk backends override with a single transactional read to
collapse N×`spawn_blocking` + N transaction setups into one." Confirm the
fjall backend's override (`crates/shamir-storage/src/storage_fjall.rs`,
~lines 519-535) really does collapse N individual `spawn_blocking` calls
into ONE `task::spawn_blocking` doing all N gets inside a single blocking
closure — this is the real cost `read_as_of`'s current per-record `.await`
loop is paying and not recovering.

## Fix — add `MvccStore::get_at_many`, use it from `read_as_of`

### 1. `MvccStore::get_at_many` (`crates/shamir-tx/src/mvcc_store/mod.rs`)

Add a new method, sibling to `get_at` (~line 1046):

```rust
pub async fn get_at_many(&self, keys: &[Bytes], snapshot_version: u64) -> DbResult<Vec<Option<Bytes>>>
```

(Adjust the exact key type — `read_as_of` currently calls `get_at` with
`id.as_bytes()`, a `&[u8]`; pick whatever key representation lets you
build a `Vec<RecordKey>` for the underlying `history.get_many` call
without an awkward extra allocation pass — check `encode_version_key`'s
signature and `RecordKey`'s exact type before finalizing.)

**Partition, don't force-batch the fallback path.** Classify each input
key via `self.current_version(key)` — this is an in-memory, NON-I/O
lookup (a `RecordCell` map read), so classifying every key up front is
cheap:

- **Direct-path keys** (`cur_v > 0 && cur_v <= snapshot_version` — the
  COMMON case, no concurrent write since the snapshot was pinned): for
  each, first probe the overlay (`self.overlay.get(key, cur_v)`, already
  an in-memory, non-I/O op per `resolve_read`'s existing logic) — overlay
  hits resolve immediately, no batching needed for those (they never hit
  `history` at all). Collect the OVERLAY-MISS subset's `encode_version_key(key,
  cur_v)` values into one `Vec`, and issue exactly ONE
  `self.history.get_many(that_vec).await` call for the whole subset —
  this is the actual win: N sequential single-key gets collapse into one
  vectored call.
- **Fallback-path keys** (`cur_v == 0` cold-start, or `cur_v >
  snapshot_version` — a concurrent write landed after the snapshot was
  pinned, needing the range-scan-newest-visible fallback,
  `resolve_read`'s ~lines 679-693): keep these on the EXISTING per-key
  `resolve_read` call, unbatched. This is the rare path (only fires when
  a concurrent writer touched the SAME key after the cursor's pin) — do
  NOT attempt to batch the range-scan fallback in this task; that's a
  separate, harder problem the review explicitly scoped out (a
  "cheap first step," not the full versioned-index rewrite).
- **Reassemble results in the ORIGINAL key order** (parallel to the
  input `keys` slice) — callers must not have to re-sort or track which
  subset each result came from.

Document this partition clearly in a doc comment on `get_at_many` — a
future reader needs to understand WHY only the direct path is batched,
not just that it is.

### 2. `read_as_of` — use the batched call per stream-batch

`crates/shamir-engine/src/table/read_temporal.rs`'s enumeration loop:
instead of the per-id `for (id, _cow) in batch { let asof_bytes =
mvcc.get_at(...).await?; ... }`, collect the WHOLE stream-yielded
`batch`'s ids into a `Vec`, call `mvcc.get_at_many(&ids, version).await?`
ONCE per stream batch, then zip the returned `Vec<Option<Bytes>>` back
against the original `(id, _cow)` pairs to run the EXACT SAME
filter/projection logic that already exists (the `let Some(bytes) =
asof_bytes else { continue; }` / WHERE-filter-apply / `matched.push(...)`
body) — this task changes ONLY how the byte lookups are fetched, not
anything about what happens once a byte value is in hand. Do not change
`FULL_SCAN_BATCH`'s batch size or the streaming/pagination behavior
itself.

## Tests (TDD — write failing tests first for `get_at_many`, then confirm `read_as_of`'s refactor is behavior-identical)

In whatever test module covers `MvccStore` today (`crates/shamir-tx/src/tests/mvcc_store_tests.rs`
or similar — find it):

- **Mixed batch, all direct-path**: several keys all at versions `<=
  snapshot_version`, no concurrent writes — `get_at_many` returns the same
  values `get_at` would return per-key, in the SAME order as the input
  keys.
- **Missing key in the batch**: a key that was never written returns
  `None` at its position, other keys in the SAME batch still resolve
  correctly (proves one absent key doesn't corrupt/misalign the rest of
  the batch).
- **Tombstone in the batch**: a deleted-then-still-alive-at-snapshot key
  (the tombstone bytes are empty per this codebase's convention) resolves
  to `None` in `get_at_many`, matching `get_at`'s own tombstone-to-`None`
  mapping.
- **Mixed direct-path + fallback-path in the SAME batch**: some keys
  resolvable via the batched `history.get_many` path, others needing the
  per-key range-scan fallback (a concurrent write landed on THOSE specific
  keys after the snapshot pin) — all resolve correctly, correctly
  interleaved back into the original order.
- **Empty input**: `get_at_many(&[], version)` returns `Ok(vec![])`
  without attempting any I/O (mirrors `Store::get_many`'s own early-return
  for an empty key list, ~`storage_fjall.rs` line 520).

In `crates/shamir-engine/src/table/tests/asof_read_tests.rs` and
`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`: all
EXISTING tests must stay green (this is a behavior-identical refactor of
HOW bytes are fetched, not WHAT `read_as_of`/cursor pages return) — no new
behavioral test should be needed for `read_as_of` itself beyond the
existing suite passing, since the fix does not change matching/filtering/
pagination semantics; if you find a genuine edge case the existing tests
don't cover that only THIS refactor could newly break, add it, but don't
invent redundant coverage for what the `get_at_many` unit tests above
already prove.

## Performance evidence (do not hand-wave)

Per this codebase's `/opti` discipline (`CLAUDE.md`'s bench-cache-isolation
section): if you want to demonstrate the speedup, use
`CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
shamir-engine --bench <name>` against an EXISTING bench target that
exercises `read_as_of`/AsOf reads (check `crates/shamir-engine/benches/`
for one) — do NOT invent a new Criterion-style bench (this workspace
migrated off Criterion; use `bench_scale_tool::Harness`, see any existing
`crates/*/benches/*.rs` file as the template if a new bench file is
genuinely warranted). A simple timed-comparison INTEGRATION TEST (measure
wall-clock before/after over a few thousand rows, log the numbers, no
strict pass/fail threshold since CI runner timing is noisy) is acceptable
evidence at this scale if writing a full bench-harness file feels like
overkill for a "cheap first step" optimization — your call, but report
SOME concrete before/after number in your final report rather than
asserting the speedup exists without measurement.

## Gate

```
cargo fmt -p shamir-engine -p shamir-tx -p shamir-server -- --check
cargo clippy -p shamir-engine -p shamir-tx -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-tx -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-tx`
(`mvcc_store/mod.rs`, its tests), `shamir-engine`
(`table/read_temporal.rs`). Do NOT touch the fallback range-scan path's
OWN implementation (`scan_history_newest`/`resolve_read`'s second branch)
— this task only adds a NEW batched entry point that reuses those
existing per-key helpers for the keys that need them, it does not
optimize the fallback path itself (explicitly out of scope, noted as
future work in this brief).
