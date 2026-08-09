# Brief 58 — #1087 round 2: fix decode calls, private-field access, and a shadowing bug

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 1 got right — do not touch

Phase B's logic (barrier acquisition, `open_index_build_snapshot()`,
`register_index_at_building`, `mark_build_in_flight`, the `Ok(false)`
fallback signal) is correct and matches the brief exactly. Leave it as-is.
`IndexManager::register_index_at_building` (new method,
`crates/shamir-index/src/base_index/index_manager.rs`) is also correct.

## What's broken — round 1 honestly reported these compile errors, do not re-litigate whether they're real

```
error[E0277]: the trait bound `RecordId: TryFrom<&[u8]>` is not satisfied
error[E0277]: the trait bound `Value<InternerKey>: TryFrom<&[u8]>` is not satisfied
error[E0616]: field `info_store` of struct `IndexManager` is private
error[E0616]: field `posting_cache` of struct `IndexManager` is private
```

### Fix 1 — wrong decode API

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s new
`phase_b_a_backfill` uses `RecordId::try_from(key_bytes.as_ref())` and
`InnerValue::try_from(value_bytes.as_ref())` — neither trait impl exists.
The correct APIs (verified 2026-08-09):
- `RecordId::try_from_bytes(b: &[u8]) -> Option<Self>`
  (`crates/shamir-types/src/types/record_id.rs:121`) — note: returns
  `Option`, not `Result`, so use `.ok_or_else(|| DbError::Internal(...))`
  instead of `.map_err(...)`.
- `InnerValue::from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self,
  rmp_serde::decode::Error>` (`crates/shamir-types/src/types/value.rs:57`)
  — already returns `Result`, `.map_err(...)` is correct as originally
  written, just call the right function name.

This exact pattern (`RecordId::try_from_bytes(&k).ok_or_else(...)` +
`InnerValue::from_bytes(&v).map_err(...)`) is already used for decoding
MVCC stream output elsewhere in this codebase — see
`crates/shamir-engine/src/tx/pre_commit.rs`'s
`rederive_base_index_ops_post_stage` for a working example of decoding a
key/value pair from the exact same kind of source (a `KvOp::Set(k, v)`
carrying raw MVCC bytes). Mirror it.

### Fix 2 — private field access, add a proper IndexManager-side write helper

`self.index_manager.info_store`/`self.index_manager.posting_cache` are
private to `shamir-index` — `TableManager` (in `shamir-engine`, a different
crate) cannot reach them, and should not (reaching into another type's
private storage fields from outside is the wrong shape regardless of
visibility).

Add a new method on `IndexManager`
(`crates/shamir-index/src/base_index/index_manager.rs`, near
`register_index_at_building`), e.g.:

```rust
/// #1087: write a batch of postings during online build's Phase A backfill,
/// and clear the posting cache for the affected index keys. Mirrors the
/// inline batch-write step inside `create_index_from_stream`'s Phase 2 body
/// (same `set_many` + `posting_cache.remove` pattern) — this is that same
/// logic, exposed as a callable unit for the online-build orchestration
/// living in `TableManager` (a different crate).
pub async fn write_postings_batch(
    &self,
    posting_writes: Vec<(Bytes, Bytes)>,
    cache_index_keys: Vec<Bytes>,
) -> DbResult<()> {
    if !posting_writes.is_empty() {
        let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        self.info_store.set_many(posting_writes).await?;
    }
    for ik in cache_index_keys {
        self.posting_cache.remove(&ik);
    }
    Ok(())
}
```

(Adjust imports/exact types to match what's already used in this file —
`Bytes`, `RecordKey`, `DbResult` are all already imported in
`index_manager.rs`.) Then in `table_manager_index_mgmt.rs`'s
`phase_b_a_backfill`, replace the direct `self.index_manager.info_store...`/
`self.index_manager.posting_cache...` calls with
`self.index_manager.write_postings_batch(posting_writes, cache_index_keys).await?`.

### Fix 3 — a latent bug, fix it while you're in this code regardless of whether it was the actual compile blocker

In the current (broken) diff, `posting_writes`/`cache_index_keys` are
declared ONCE, OUTSIDE the `while let Some(batch) = ...` loop (unlike the
existing, working `create_index_from_stream`'s Phase 2, where these are
declared FRESH INSIDE the loop, per batch — check
`index_manager.rs:1727+`'s existing body for the correct shape). Inside the
`if !posting_writes.is_empty()` block, `let posting_writes: Vec<(RecordKey,
Bytes)> = posting_writes.into_iter()...` SHADOWS the outer binding and
consumes it by value. The subsequent `posting_writes.clear()` (if it was
reachable at all — check whether this line survives after Fix 2's refactor)
would refer to the WRONG (shadowed, and now moved-into `write_postings_batch`)
variable, not the outer per-scan accumulator — meaning the OUTER
`posting_writes`/`cache_index_keys` would never actually reset between
batches, silently reprocessing/rewriting every prior batch's postings on
every subsequent `set_many` call (wasteful, likely O(n²) over a large scan,
though not incorrect since `SetPosting` is idempotent).

**Fix by declaring `posting_writes` and `cache_index_keys` fresh INSIDE the
`while` loop body**, exactly matching `create_index_from_stream`'s existing
Phase 2 shape (`index_manager.rs:1727+` — copy that structure, don't
improvise a hoisted-accumulator variant). This eliminates the shadowing
entirely and matches the proven, existing pattern.

## After fixing — verify compilation, THEN run the actual tests

Round 1 could not get this far — you're now unblocked to actually validate
the 3 tests it wrote (or, if it didn't get to writing them because of the
compile wall, check `crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs`
and `p1087_phase_b_a.md`'s original 3 required tests — basic correctness,
concurrent-writes-land-in-dirty-set via a pause hook, and the changefeed-
unavailable fallback signal). Write whichever are missing.

## Gate before you report done

```
cargo check -p shamir-engine --lib
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Run `cargo check -p shamir-engine --lib` FIRST and confirm it's clean before
running anything else — don't repeat round 1's pattern of writing tests
against code that doesn't compile. Report the exact diff and the exact
nextest output for the 3 required tests.
