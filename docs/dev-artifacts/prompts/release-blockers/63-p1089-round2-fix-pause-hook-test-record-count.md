# Brief 63 — #1089 round 2: fix tests 2+3's pause-hook hang (same root cause as #1087's, already diagnosed)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 1 got right — do not touch

Round 1 delivered the full wiring correctly and honestly reported that
tests 2 and 3 (`p1059_online_create_index_concurrent_mixed_ops`,
`p1059_online_create_index_writer_latency_bounded`) time out, without
guessing at a wrong cause or weakening the assertions. Everything else —
`phase_b_a_backfill`'s new `name` parameter + `any_index_exists` check
(TOCTOU fix), `create_index`'s try-online-then-fallback restructuring, the
`doctor::repair()` descope (correctly left untouched, as required) — is
correct. `cargo fmt`/`clippy` are clean, `./scripts/test.sh -p shamir-index`
is 398/398, and tests 1 and 4 in the new integration file pass. Leave all
of that alone.

## Root cause (diagnosed, not a guess — verified against the exact code)

`create_index`'s wiring calls
`self.phase_b_a_backfill(name, index_def.clone(), 1000)` —
`table_manager_index_mgmt.rs:669` — hardcoding `batch_size = 1000`.

The pause-hook seam inside `phase_b_a_backfill`'s scan loop only fires when
a SECOND stream batch arrives (`if batch_no == 1` checked at the top of an
iteration, `batch_no` incremented at the bottom of the PREVIOUS iteration —
see the loop in `phase_b_a_backfill`, same shape as `#1087`'s scan). The
underlying `InMemoryStore::iter_stream` (`storage_in_memory.rs:161-168`)
never yields a trailing empty batch — it stops the moment all records are
drained. So with `batch_size = 1000` and FEWER than 1000 records, the ENTIRE
scan fits in ONE batch, `batch_no` never reaches `1` at the top of a real
iteration, the pause hook's `wait_at_window()` is never called, and the
test's `hook.wait_until_parked().await` waits forever.

**This is the EXACT SAME bug class already found and fixed once in this
task chain** — see `docs/dev-artifacts/prompts/release-blockers/60-p1087-round4-fix-test-batch-size.md`,
which fixed an identical hang in `#1087`'s own concurrent-write test by
changing that test's `batch_size` ARGUMENT from `1000` to `1` (that test
called `phase_b_a_backfill` directly, so it controlled the batch_size
parameter). **Tests 2 and 3 here go through the PUBLIC `create_index` entry
point, which does NOT expose a batch_size parameter** (by design — it's an
internal implementation detail, hardcoded to `1000` to match today's
existing default). So the fix here is the OTHER side of the same
relationship: make the test's RECORD COUNT exceed the hardcoded
`batch_size` (1000), instead of shrinking the batch size.

## Fix

In `crates/shamir-engine/src/table/tests/p1059_online_create_index_tests.rs`:

**Test 2 (`p1059_online_create_index_concurrent_mixed_ops`).** Currently
inserts only 3 records (`insert_test_data`, hardcoded to alice/bob/charlie)
before spawning `create_index`. Insert MORE records BEFORE calling
`insert_test_data`'s 3 named rows, so the total row count exceeds 1000
(e.g. insert 1200 filler rows with a distinct field or the same "name"
schema, THEN insert alice/bob/charlie so they're identifiable and their
`RecordId`s are captured for the later assertions) — this forces the scan
into ≥2 batches, so the pause hook fires partway through and the test's
concurrent ops actually land while Phase A is genuinely still scanning
later batches. Keep the existing assertions (diane found, bob only under
new value, charlie not found, alice unchanged) — only the fixture size
needs to grow. Watch out: the filler rows must not collide with the "name"
values used in assertions ("diane", "bob_updated", "charlie", "alice") —
use a clearly distinct pattern like `format!("filler_{i}")`.

**Test 3 (`p1059_online_create_index_writer_latency_bounded`).** Currently
inserts 300 rows — bump this to something safely over 1000 (e.g. 1200) so
the same pause-hook mechanism fires. The rest of the test (timing the
concurrent insert, asserting the bound and that the row is indexed) stays
the same.

**Do not change `phase_b_a_backfill`'s hardcoded `batch_size = 1000` in
`create_index`'s wiring** — that's the production default, not a test
knob; changing it would be scope creep unrelated to fixing these two
tests. **Do not change the pause-hook seam's placement** in
`phase_b_a_backfill` — it's correct and already proven by `#1087`'s own
(now-passing) tests; the fix here is purely the two integration tests'
fixture size.

## After the fix — re-run and confirm

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p1059_online_create_index
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

All 4 `p1059_online_create_index_*` tests must show `PASS`, not
`TIMEOUT`/`SLOW`. Paste the exact nextest output. If either test still
doesn't pass after this change (e.g. a genuine assertion failure, not a
timeout), STOP and report exactly what happened — do not weaken an
assertion or shrink a fixture back down to force a pass.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff (should be small — record-count bumps in 2 tests)
and the exact nextest output for all 4 `p1059_online_create_index_*` tests.
