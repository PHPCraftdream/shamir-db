# Brief 60 — #1087 round 4: fix the concurrent-write test's batch_size (test bug, not production bug)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What rounds 2+3 got right — do not touch

Round 2 fixed the 3 compile errors. Round 3 correctly identified and fixed
the real deadlock (barrier guards never dropped before Phase A) by adding
`drop(_barrier); drop(_uwl_guard);` right after `mark_build_in_flight`,
mirroring the existing `doctor.rs:633-634` precedent. That fix is CORRECT —
do not revert or second-guess it. `cargo check`/`fmt`/`clippy`/
`./scripts/test.sh -p shamir-index` are all clean.

## The remaining failure — root-caused, not a guess

After round 3's fix, `p1087_phase_b_a_concurrent_write_captured_in_dirty_set`
STILL times out at 180s, but at a DIFFERENT point than round 2: it now
hangs on `hook.wait_until_parked().await` in the test itself — meaning the
`phase_b_a_backfill` task never reaches its pause point at all.

I traced the exact cause by reading the production loop and the storage
layer directly (not guessing):

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s
`phase_b_a_backfill` loop:

```rust
while let Some(batch_result) = posting_stream.next().await {
    #[cfg(test)]
    {
        if batch_no == 1 {
            if let Some(hook) = self.online_index_backfill_hook.load_full() {
                hook.wait_at_window().await;
            }
        }
    }
    // ... decode batch, write postings ...
    batch_no += 1;
}
```

The pause fires only when a SECOND stream batch arrives (checked at the top
of that batch's iteration, i.e. `batch_no` was incremented to `1` at the
END of the FIRST iteration).

`crates/shamir-storage/src/storage_in_memory.rs`'s `iter_stream`
(the underlying source `MvccStore::snapshot_stream` streams through):

```rust
Box::pin(stream! {
    let mut entries = entries;
    while !entries.is_empty() {
        let take = std::cmp::min(batch_size, entries.len());
        let batch: Vec<(RecordKey, Bytes)> = entries.drain(..take).collect();
        yield Ok(batch);
    }
})
```

This NEVER yields a trailing empty batch — it stops as soon as `entries` is
drained. The test
(`crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs`,
`p1087_phase_b_a_concurrent_write_captured_in_dirty_set`) inserts exactly 3
records (`insert_test_data`) and calls
`tbl_clone.phase_b_a_backfill(index_def_clone, 1000)` — batch_size 1000
with only 3 records means the ENTIRE scan fits in ONE batch. The stream
yields that one batch (`batch_no == 0` at the top of that iteration, so the
pause check is skipped), then ends. There is no second iteration, so
`batch_no` never reaches `1` at the top of a loop body, so
`hook.wait_at_window()` is never called, so the test's
`hook.wait_until_parked().await` waits forever.

This is a **test bug**, not a production code defect: the test's chosen
`batch_size` (1000) can never produce more than one batch for 3 rows, so
the pause seam (which is deliberately gated on there being a 2nd batch —
matching the RFC's "pause mid-scan, after ≥1 batch written" requirement)
can never trigger.

## Fix

In `p1087_phase_b_a_concurrent_write_captured_in_dirty_set`
(`crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs`), change
the `phase_b_a_backfill` call's `batch_size` argument from `1000` to `1`:

```rust
let backfill_task =
    tokio::spawn(async move { tbl_clone.phase_b_a_backfill(index_def_clone, 1).await });
```

With `batch_size: 1` and 3 records, the scan produces 3 separate
single-record batches: batch 0 (record "alice") processed with no pause
(`batch_no == 0`), then at the top of the NEXT iteration (`batch_no == 1`,
about to process "bob") the pause fires — "mid-scan" with 1 of 3 records
written, exactly matching the test's intent and its own doc comment ("pause
hook to park mid-scan"). Do NOT change the production loop or the pause
seam's placement — only this one test call-site argument.

Do not change `p1087_phase_b_a_correctness_no_concurrency`'s or
`p1087_phase_b_a_fallback_when_changefeed_absent`'s batch_size (both pass
already and don't rely on multi-batch pausing).

## After the fix — re-run and confirm

```
cargo check -p shamir-engine --lib
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine -- p1087_phase_b_a
./scripts/test.sh -p shamir-engine
```

All 3 `p1087_phase_b_a_*` tests must show `PASS`. Paste the exact nextest
output. If the concurrent-write test still fails after this change (e.g. a
real assertion failure, not a timeout), STOP and report exactly what
happened — do not weaken the assertion or delete the test to force a pass.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff (should be a 1-line argument change) and the exact
nextest output for all 3 `p1087_phase_b_a_*` tests.
