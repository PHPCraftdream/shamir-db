# Brief 66 — #1061: pipeline property tests (completeness, convergence, bounded barrier, equivalence)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

This is slice 1f, the LAST test-only task of online CREATE INDEX (RFC v3).
The full pipeline (`#1087` Phase B+A, `#1088` Phase C+D, `#1089` wiring,
`#1060` crash recovery) is done and committed. This task writes 4 END-TO-END
property tests that can ONLY be written now the pipeline exists whole —
each proves a specific claim from the RFC empirically, not just a unit of
one primitive.

New file: `crates/shamir-engine/src/table/tests/p1061_pipeline_property_tests.rs`
(wire into `tests/mod.rs`).

## Quality bar — read this before writing anything

**Every test must FAIL on code lacking the mechanism it claims to prove.**
A test that passes whether the mechanism exists or not has zero regression
value — this session already caught that exact failure mode twice on other
tasks (once via a delegate skipping tests with false excuses, once via a
test whose pause hook never fired because of a batch-size mismatch, making
it pass vacuously). The orchestrator will personally verify each test's
discriminating power by temporarily breaking the relevant mechanism and
re-running — you do not need to do this yourself, but write tests precise
enough to survive it (specific value assertions, not "didn't panic" checks,
not loose count comparisons where an exact comparison is available).

## Test 1 — Completeness of capture (proves RFC §3 Claim 2 empirically)

Direct proof, not reasoning: pause Phase A mid-scan, drive a MIX of
concurrent operations — including operations the scan already passed and
operations it hasn't reached yet — resume, and check the FINAL index
reflects the FINAL state of every touched row.

Use `phase_b_a_backfill`/`phase_c_d_catchup_and_publish` directly (not the
public `create_index`) with `batch_size = 1`, matching the proven pattern
in `p1087_phase_b_a_tests.rs`'s `p1087_phase_b_a_concurrent_write_captured_in_dirty_set`
— this avoids needing 1000+ filler rows to force multiple batches.

Setup: insert 5 rows (say ids 0-4, distinct "name" values). Install
`online_index_backfill_hook`, spawn `phase_b_a_backfill(name, index_def,
1)`, wait for the hook to park (fires after processing row 0, i.e. mid-scan
with rows 1-4 not yet visited).

While parked, from a separate task, concurrently:
- **Insert** a brand-new row (never existed before the build started).
- **Update** row 0 (the ALREADY-SCANNED row) to a new "name" value.
- **Update** row 4 (a row the scan has NOT YET REACHED) to a new "name"
  value — this is the complementary case to row 0's: proves capture works
  regardless of scan progress, not just for already-visited rows.
- **Update row 1 TWICE** in sequence (two different new values) — proves
  the dirty-set's re-read-at-current-version mechanism correctly picks up
  the FINAL value, not an intermediate one, even though only one dirty-set
  entry exists for it (RecordId-only capture, no value history).
- **Insert-then-delete** a brand-new row within the same window (create it,
  then immediately delete it, both before resuming Phase A) — this row
  NEVER existed at the pin (Phase A never wrote it a posting) and is gone
  by the time Phase C looks at it. Assert it has ZERO postings anywhere in
  `info_store` after the build completes — scan the index's posting
  keyspace directly (mirror `collect_postings` from
  `crates/shamir-index/src/base_index/tests/index_manager_tests/f78_streaming_equivalence_tests.rs:73-90`
  — prefix-scan via `IndexRecordKey::new(false, name_interned).to_prefix_bytes()`)
  and confirm no posting key embeds this row's `RecordId` suffix.

Resume the hook, await `phase_b_a_backfill`'s completion, then run
`phase_c_d_catchup_and_publish` to completion (`Ready`).

Assertions (via `lookup_by_index`, matching `#1088`/`#1089`'s established
pattern):
- The new plain insert is findable under its value.
- Row 0 is findable ONLY under its new value, not the original.
- Row 4 is findable ONLY under its new value, not the original.
- Row 1 is findable ONLY under its SECOND new value (not the first
  intermediate one, not the original).
- The insert-then-delete row is findable under NO value, AND has zero
  postings on disk (the stronger, direct check above).
- Rows 2 and 3 (untouched during the window) are findable under their
  original values.

## Test 2 — Convergence / termination under sustained write load

A generator that keeps writing new dirty records concurrently with Phase
C's catch-up loop, for a bounded duration, then stops — proving Phase C
(and the `CATCHUP_ITERATION_CAP` hand-off to Phase D) actually terminates
rather than looping forever chasing a moving target.

Setup: insert a small base fixture, run `phase_b_a_backfill` to completion
(uninterrupted). Spawn a background task that inserts ~200 new rows in a
tight loop (no artificial delay — the point is to plausibly outrun a
single `drain_dirty_set` cycle via natural tokio scheduling interleaving;
this does not need to be perfectly deterministic, only "a realistic
concurrent load"). Immediately (racing the generator) call
`phase_c_d_catchup_and_publish`.

**Wrap the call in `tokio::time::timeout(Duration::from_secs(30), ...)`
and assert it returns `Ok(Ok(()))`** — this is the "must not hang forever"
requirement from the brief: a regression that broke termination shows up
as a clean, fast test FAILURE (timeout elapsed), not a 180s nextest kill.
After the generator task finishes (`.await` its `JoinHandle`), assert:
the index is `Ready`, and `index_manager_ref().drain_dirty_set(name_interned)`
returns empty (proving Phase D's final residual really did catch
everything left over after the capped loop, not just "some" of it).

## Test 3 — Bounded publish-barrier duration (THE point of the whole redesign)

Run the identical no-concurrent-writes scenario at two RADICALLY different
table sizes and assert Phase D's barrier-held duration is small and
roughly CONSTANT across them, while Phase A's scan duration is NOT.

Sizes: pick numbers that produce a clear, non-flaky signal while keeping
total test wall-clock reasonable (well under nextest's 180s kill,
ideally under 30s total for both runs) — e.g. 500 rows vs 50,000 rows (not
literally 1k/500k from the task's illustrative example; scale down for
test speed while preserving a large ratio). For EACH size:

1. Insert the fixture (no concurrent writers).
2. `Instant::now()` → `phase_b_a_backfill(...)` → measure elapsed as
   `phase_a_duration`.
3. `Instant::now()` → `phase_c_d_catchup_and_publish(...)` → measure
   elapsed as `phase_d_duration`. Because there are NO concurrent writes,
   Phase C's loop breaks on its FIRST iteration (dirty-set empty
   immediately) — so this measured duration is effectively pure Phase D
   (barrier acquire + empty residual + flip + persist), not conflated with
   real catch-up work.

Assertions (concrete pass/fail thresholds, not printed numbers to eyeball):
- `phase_a_duration` for the LARGE table must be at least 3× the SMALL
  table's (proves Phase A scales with table size — sanity check that the
  test fixture sizes actually produce a measurable difference; adjust the
  multiplier down if 3× proves flaky in practice, but it must be
  unambiguously non-1×).
- `phase_d_duration` for BOTH sizes must be under an absolute ceiling (e.g.
  100ms — pick a number generous enough to avoid CI-machine flakiness but
  tight enough that it would clearly fail if Phase D held the barrier for
  anywhere near Phase A's duration). This is the actual correctness gate:
  Phase D's cost must not scale with table size.
- Additionally assert `phase_d_duration` for the LARGE table is NOT within
  the same order of magnitude as `phase_a_duration` for the LARGE table
  (e.g. `phase_d_duration_large * 10 < phase_a_duration_large` once the
  table is large enough that Phase A takes at least, say, 50ms — skip this
  specific comparison if Phase A is too fast to measure meaningfully at
  your chosen size, but then the table isn't large enough to make the
  point — tune the size up until Phase A is clearly slower).

Use `std::time::Instant`, not `tokio::time::Instant` (wall-clock, not the
tokio test clock — these tests run against a real InMemoryStore with real
async I/O work, not a paused virtual clock).

## Test 4 — Equivalence with the old path (no concurrent writes)

Mirror `f78_streaming_equivalence_tests.rs`'s exact technique (already
proven, `crates/shamir-index/src/base_index/tests/index_manager_tests/f78_streaming_equivalence_tests.rs`):
build the SAME fixture through BOTH paths against separate stores, collect
each `info_store`'s posting keyspace into a `BTreeSet<(Vec<u8>, Vec<u8>)>`
via a prefix scan, and assert the two sets are byte-identical — not just
matching counts (`#1089`'s existing
`p1059_online_create_index_correctness_equivalence` test only compares
per-value lookup COUNTS, which is weaker; this test is a genuine
strengthening, write it as a NEW test in THIS file rather than editing
`#1089`'s already-committed file).

- OLD path: a table WITHOUT changefeed, `create_index(...)` (falls back to
  `create_index_from_stream` internally).
- NEW path: a table WITH changefeed, `create_index(...)` (takes the online
  path — Phase B+A then C+D — with NO concurrent writers during the whole
  call, run fully sequentially/awaited, no `select!`, no pause hook).
- Use `collect_postings`'s exact pattern (prefix-scan
  `IndexRecordKey::new(false, name_interned).to_prefix_bytes()` against
  each table's `info_store`, collect into a `BTreeSet<(Vec<u8>, Vec<u8>)>`).
- Assert the two `BTreeSet`s are equal.
- Include the same "both-sides-empty false-pass" guard the precedent uses
  (`assert!(postings.len() > fixture.len() / 2, ...)` or similar — a
  non-trivial fixture that actually produces postings).

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report exactly which 4 (or more, if you split into sub-tests) tests you
wrote, their individual pass/fail status, and the full suite's final
summary line. If any test's timing-based assertion (test 3) proves flaky
across repeated runs, say so explicitly and report what you tried — do not
silently loosen a threshold to make a flaky test "pass" without flagging
it.
