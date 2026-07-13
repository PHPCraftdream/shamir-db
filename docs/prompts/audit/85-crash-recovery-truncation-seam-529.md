בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: fix the truncation crash-seam not firing in crash_recovery.rs (task #529 final gate)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

Found by the audit-remediation campaign's final full-workspace gate
(`./scripts/test.sh --full`, task #529 — the first time the ENTIRE
suite has been run in one pass; per-task gates throughout the campaign
were intentionally scoped to `cargo check` + narrow test filters, so a
regression here could have existed for a while without being caught).

Three tests in `crates/shamir-engine/tests/crash_recovery.rs` fail
**deterministically** (reproduced 3/3 in isolation, not a load-only
flake — confirmed via `./scripts/test.sh -p shamir-engine --full --
crash_at_post_truncate_recovers_all crash_at_mid_delete_recovers_all
crash_at_pre_truncate_recovers_all`, all 3 FAIL every time):

- `crash_at_pre_truncate_recovers_all`
- `crash_at_post_truncate_recovers_all`
- `crash_at_mid_delete_recovers_all`

All three panic at the SAME assertion,
`crash_recovery.rs:666-671` (in `trunc_crash_then_recover`):

```rust
let status = spawn_child_trunc(phase, &repo_path);
assert!(
    !status.success(),
    "truncation child must die abnormally at seam {phase}; got a clean \
     exit (status {status:?}) — the crash seam did not fire (not enough \
     sealed segments to truncate?)"
);
```

The child process is SUPPOSED to abort (via `process::abort()`, wired
through `crate::tx::commit::maybe_crash(seam, repo)` — see
`crates/shamir-engine/src/tx/commit.rs:83` for the env-var check, and
the two call sites in `crates/shamir-engine/src/tx/drainer.rs:710,716`
for `pre_truncate`/`post_truncate`) at a specific point during WAL
segment truncation. Instead it's exiting cleanly (status success) —
the crash seam is never reached.

## The mechanism (read before touching anything)

`run_child_scenario_trunc` (`crash_recovery.rs:539-607`) commits
`TRUNC_RECORDS` (= 40, line 91) individual single-field text records
under a TINY WAL segment cap (`TRUNC_SEG_CAP` = `"4096"` bytes, line
95, passed via `SHAMIR_WAL_SEGMENT_MAX_BYTES`) so the WAL rolls
MULTIPLE sealed segments. Either the auto-spawned background drainer
or the final explicit `drain_all()` call is expected to advance the
durable watermark far enough that `Drainer`'s truncation step
(`crates/shamir-engine/src/tx/drainer.rs:704-723`) finds
`wal.has_truncatable(ceiling)` true — ONLY THEN do the
`pre_truncate`/`post_truncate` seams (or `wal_mid_delete` inside
`SegmentSet::truncate_below`, `crates/shamir-wal/src/segment_set.rs:535`)
ever get a chance to fire. If 40 records at ~4096 bytes/segment no
longer produce enough SEALED segments crossing the drain ceiling
(e.g. because per-record WAL framing overhead shrank, or segment
rollover/ceiling-advancement logic changed), `has_truncatable` may
never return true, none of the three seams ever execute, and the
child runs to a clean, un-crashed completion — exactly the observed
symptom.

Confirmed NOT a resource-contention flake: reproduces 3/3 in
isolation with no other concurrent load. Confirmed the underlying
truncation MECHANISM itself still works in principle — the lib-level
unit tests in `crates/shamir-engine/src/tx/tests/truncation_tests.rs`
all pass (`./scripts/test.sh -p shamir-engine -- truncation_tests` —
5/5 green) — so this is specifically about whether THIS test's byte
budget (40 records, 4096-byte cap) still produces the SAME number of
sealed-segment-crossings it used to, not a fundamental break in
truncation.

## Investigation required (in this order)

1. **Confirm the root cause empirically** — don't guess. Add
   temporary instrumentation (a `log::debug!`/`eprintln!` in
   `run_child_scenario_trunc` or the drainer's truncation step,
   env-gated so it doesn't pollute normal test output, or just run the
   child binary directly and inspect stderr/the actual WAL segment
   file count under `dir.path()` before it's torn down) to determine:
   how many sealed WAL segments actually get created for 40 records
   under the 4096-byte cap on the CURRENT code, and whether
   `has_truncatable(ceiling)` is ever true during the run. Compare
   against what the test's own doc comments imply it used to need
   ("MULTIPLE segments... `wal_mid_delete` has >= 2", line 89-91).

2. **Find what changed.** `git log --oneline -- crates/shamir-engine/tests/crash_recovery.rs
   crates/shamir-engine/src/tx/drainer.rs crates/shamir-wal/src/segment_set.rs
   crates/shamir-wal/src/*.rs` — the most recent touch to this test
   file / the drainer's truncation step / the WAL segment/framing code
   is task #531 (`aa76c1d3`, "WAL test-hardening — group-commit
   fault-injection + reactivated-segment sidecar test"), but earlier
   perf tasks (#500 "WAL segment-open avoid full replay", #489/#501
   WAL/interner changes, #536 "fjall worker-loop write-only redesign")
   may have shifted per-record byte accounting or watermark/ceiling
   advancement timing too. Read the diffs of whichever candidates
   touched WAL record framing size, segment sealing thresholds, or the
   `has_truncatable`/ceiling computation
   (`crates/shamir-engine/src/tx/drainer.rs:680-687`) to find the
   actual behavioral delta.

3. **Decide the fix** based on what #1/#2 reveal. Likely candidates
   (do NOT pick blindly — let the instrumentation from #1 tell you
   which applies):
   - If per-record WAL framing shrank (a record now takes fewer bytes
     than before): bump `TRUNC_RECORDS` and/or shrink `TRUNC_SEG_CAP`
     in `crash_recovery.rs` so the test still reliably rolls enough
     sealed segments to cross the ceiling — a test-constant fix, no
     production-code change.
   - If the drain ceiling/watermark advancement logic itself changed
     such that `has_truncatable` is harder to satisfy for a legitimate
     reason (not a bug): same test-constant fix as above, but document
     WHY in a comment referencing the task that changed the behavior.
   - If this reveals a REAL correctness regression (e.g.
     `has_truncatable`'s ceiling computation has an off-by-one that
     makes it wrongly report "nothing truncatable" when there
     genuinely IS a full sealed segment below the ceiling): that's a
     production bug, not a test-tuning issue — fix `drainer.rs`
     directly and explain the mechanism found.

## Out of scope

- Do NOT weaken or remove the `assert!(!status.success(), ...)`
  assertion itself — it is testing the RIGHT thing (the crash seam
  must actually fire for the zero-loss recovery assertions below it to
  mean anything). If you cannot make the seam fire reliably, that is
  the bug to report, not something to route around by deleting or
  loosening the assertion.
- Do NOT touch `wal_mid_delete`'s OWN dedicated test
  (`crash_at_wal_mid_delete_recovers_all` or similarly named, check
  around line 855-867) unless your investigation shows it's ALSO
  affected — the brief above only names the 3 confirmed-failing
  tests; if a 4th one is secretly also broken (it currently passes,
  per the gate run), say so but don't "fix" something that isn't
  failing.
- Do NOT touch anything outside `crates/shamir-engine` and
  `crates/shamir-wal` (the WAL/drainer/segment-truncation subsystem) —
  this is a narrowly-scoped test-infrastructure/timing fix, not a
  license to touch unrelated code.

## Verification

```
cargo check --workspace --all-targets
cargo fmt -p shamir-engine -p shamir-wal -- --check
cargo clippy -p shamir-engine -p shamir-wal --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full -- crash_at_pre_truncate_recovers_all crash_at_post_truncate_recovers_all crash_at_mid_delete_recovers_all
./scripts/test.sh -p shamir-engine --full
```

Run the 3 named tests several times in a row (e.g. loop 5x) to confirm
the fix is not itself a new flake — the whole point is deterministic
reliability, matching this brief's own "reproduces 3/3" standard for
the RED state.

## Definition of done

- All 3 named tests pass RELIABLY (not just once) — confirmed via
  repeated runs.
- The root cause is understood and stated plainly in the final report,
  not just "papered over."
- `./scripts/test.sh -p shamir-engine --full` green, no new failures
  introduced elsewhere in the crate.
- Gate commands above all clean.

## Report

When done, produce a final summary (not a bare tool call): the root
cause (what actually changed and why the seam stopped firing, backed
by the instrumentation evidence from step 1), every file changed, the
gate command outputs, confirmation the 3 tests pass reliably across
multiple runs, and any discrepancy between this brief's assumptions
and what you found.
