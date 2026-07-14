Task: G6 (task #531) — two independent WAL test-hardening items. Former
#508 (real fault-injection test for `WalGroupCommit::append_many`'s
all-or-nothing atomicity claim) + former #522 (strengthen
`reactivated_segment_sheds_stale_sidecar` to exercise the poison-rotation
path directly, found during task #500's second `@fl` review).

## Part A — real fault-injection test for `append_many` atomicity (former #508)

### Context (confirmed by reading current code)

`crates/shamir-wal/src/wal_group_commit.rs::append_many` (currently
~line 223) has an extensive doc comment claiming: all entries in one
batch are pushed under one lock, drained together, and written via ONE
`sink.append_batch(...)` call — so a partial write (e.g. `ENOSPC`
mid-frame) quarantines the segment and rolls back ALL frames in the
batch, meaning "no entry survives a partial write, so recovery never
replays a subset of a failed batch."

`crates/shamir-wal/src/tests/wal_group_commit_tests.rs::append_many_is_atomic_all_entries_land`
(currently ~line 320) only tests the HAPPY path — 5 entries, no failure
injected, all land, replay sees all 5. **There is no test that actually
injects a write failure mid-batch and confirms zero entries survive.**
This is exactly what #508 asks for: a REAL fault-injection test, not
another happy-path assertion.

### Why this needs care

`WalSink` (`crates/shamir-wal/src/wal_sink.rs`, currently ~line 59) is
DELIBERATELY an enum, not a trait object — the doc comment says "no dyn
dispatch on the hot path". This means there's no existing seam to swap
in a mock/faulty sink implementation without either:

1. Adding a genuine fault-injection knob to the existing `MemSink`
   variant (e.g. a `#[cfg(test)]`-gated counter/flag: "fail the Nth
   `append_batch` call" or "fail after writing N bytes"), used only by
   tests, OR
2. Some other mechanism you discover during investigation (e.g. if the
   `File` variant's underlying write path has a testable seam already,
   or if a tiny scoped test-only wrapper around `MemSink` is cleaner
   than modifying `MemSink` itself).

**Investigate and pick the least invasive option that lets you inject a
GENUINE failure partway through a multi-entry batch write** (not a
happy-path success followed by manually asserting "well it would have
rolled back") — the test's value comes from actually exercising the
failure branch of `lead_until_drained`'s single-write logic. Whatever
mechanism you add, keep it `#[cfg(test)]`-gated or otherwise excluded
from the production build if it touches non-test code — do NOT change
`WalSink`'s enum-not-trait design for the `File` (production hot-path)
variant; if you need to add a variant, gate it clearly.

### TDD

A test that:
1. Injects a failure so that a batch of N>1 entries fails partway
   through the single `append_batch` write (or fails the whole batch —
   whichever your injection mechanism naturally produces, but the
   failure must be REAL, not simulated by directly calling an internal
   method that skips the actual write path).
2. Asserts `append_many` returns `Err` to the caller.
3. Asserts that a SUBSEQUENT replay of the sink sees ZERO of the batch's
   entries (not a subset — the all-or-nothing claim specifically means
   no partial survival). If your injection mechanism can only cleanly
   fail the WHOLE write (not a genuine "N of M bytes written" partial
   write), that's fine — the test still meaningfully proves the
   quarantine-on-failure path leaves no entries behind, which is the
   actual property `append_many`'s doc comment claims.
4. Confirm the existing happy-path test
   (`append_many_is_atomic_all_entries_land`) still passes unchanged.

If, after genuine investigation, injecting a real failure turns out to
require invasive production-code changes you're not confident are safe,
STOP and document the specific blocker + a scoped-down follow-up rather
than forcing something risky — but try the `MemSink` fault-injection-knob
approach first, since it's the more surgical of the two hardening this
brief describes.

## Part B — strengthen `reactivated_segment_sheds_stale_sidecar` (former #522)

### Context

`crates/shamir-wal/src/tests/segment_set_tests.rs::reactivated_segment_sheds_stale_sidecar`
(added in task #500) proves the fix for a stale-sidecar-on-reactivation
bug via a MID-TEST assertion (`!meta_path(dir.path(), 0).exists()`
immediately after reopen) — this IS a genuine, load-bearing regression
check (confirmed correct by task #500's second `@fl` review). However,
that same review noted the test's END-TO-END tail (append past the
stale value, force a reseal, reopen again, replay sees the true max) is
NOT load-bearing by itself — it would pass even without the fix, since
`replay()` never consults sidecars and `seal_and_rotate` always
overwrites the sidecar with the correct value on its own path regardless.

Strengthen the test (or add a sibling test) to exercise the disaster
path more directly: drive the re-activated segment through
`SegmentSet::rotate_after_poison` specifically (the OTHER call site that
was defensively hardened in task #500 alongside `SegmentSet::open`), or
otherwise force a sidecar-rewrite failure during a real reseal, and
confirm that WITHOUT the fix a stale/too-low sidecar would survive to
mislead a future open's `truncate_below`. Read
`crates/shamir-wal/src/segment_set.rs`'s `rotate_after_poison` (the
defensive `remove_blocking` call added in task #500) to understand the
exact scenario to reproduce.

### TDD

A new or strengthened test that is genuinely load-bearing through the
`rotate_after_poison` path specifically — not relying solely on the
existing mid-test existence-assertion for coverage of THIS path.

## General

Per this session's lighter per-task gate:
```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-wal
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Part A] Status: fixed / scoped-down-with-followup
  > Fault-injection mechanism chosen (and why over the alternative)
  > New test + confirmation it exercises a REAL failure, not a simulation
  > Confirm existing happy-path test unaffected

[Part B] Status: fixed
  > New/strengthened test covering the rotate_after_poison path
  > Confirmation this is genuinely load-bearing (would fail without
    task #500's fix, distinctly from the existing mid-test assertion)

[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-wal: pass/fail
```
