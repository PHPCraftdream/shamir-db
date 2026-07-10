Task: WAL `SegmentSet::open` avoids a full replay of every sealed segment
just to compute `max_version`. Task #500, deferred from task #489
(`docs/audits/2026-07-06-perf-radical-o-notation.md` finding 2.1).

## Context (re-investigate — line numbers and even the exact function
## names may have shifted since the audit was written; this campaign
## has already touched this crate multiple times, most recently in
## task #494's durability-residual cluster — read the CURRENT code,
## do not trust the audit's line numbers blindly)

`crates/shamir-wal/src/segment_set.rs`'s `SegmentSet::open` (confirm
current lines, audit cites ~131-141): for EVERY sealed segment found at
startup, it calls `seg.replay().await` — a full read + decode of every
WAL entry in that segment — SOLELY to compute `max_version` (the
highest commit_version present, used to decide which segments are
fully reclaimable once history durability advances past their max).
Startup cost is therefore O(total on-disk WAL size); after a long
downtime with many un-truncated sealed segments, opening the database
reads and decodes gigabytes just to throw away everything except one
number per segment.

**IMPORTANT — this finding was investigated once before in this
campaign (task #489) and deferred with the stated reason "WAL format
doesn't support backward-seek without a format version bump."
Re-investigate this claim specifically before assuming it's still
correct** — the audit's OWN fix sketch (see below) is a purely
ADDITIVE change (write new metadata forward at seal time), not a
format rewrite requiring backward-seek through EXISTING data. It's
possible the earlier deferral was based on a different, more invasive
approach (e.g. trying to add a trailer that old segments would need to
be retrofitted with) rather than the audit's actual suggestion.
Confirm which concern applies before deciding whether this is
tractable now.

## Fix — per the audit's own sketch

1. At seal time (`seal_and_rotate`, confirm current line, audit cites
   `:212`), write `max_version` as a small footer — e.g. an 8-byte
   value appended to the segment file itself right before the final
   fsync, OR a separate sidecar file (`NNNNNNNN.meta` or similar,
   matching this segment's naming convention). Investigate which
   approach fits this codebase's existing patterns better — check how
   other metadata is currently persisted alongside WAL segments (if
   any sidecar-file convention already exists, follow it; if not,
   an in-file footer avoids extra file-handle/fsync overhead but needs
   a clear format marker so `replay()` doesn't misinterpret it as a
   data frame).
2. On `SegmentSet::open`, for each sealed segment: FIRST attempt to
   read the footer/sidecar. If present and well-formed, use its
   `max_version` directly — skip the full replay entirely. If ABSENT
   (e.g. a segment sealed by a version of this software predating this
   change, or a footer write that was interrupted by a crash between
   the data fsync and the footer write), fall back to the existing full
   `replay()` path — this is the compatibility guarantee: old segments
   (or a segment whose footer write didn't complete) still work
   correctly, just without the speedup.
3. Crash-safety: the footer/sidecar write must not be able to leave the
   segment in an inconsistent state. Investigate: does the footer need
   its own checksum to detect a torn/partial footer write (distinct
   from "absent" — a CORRUPTED footer is different from a MISSING one,
   and must fall back to replay too, not be silently trusted or treated
   as a fatal error)? Order of operations matters: the footer write
   should happen AFTER the segment's own data is durably fsynced (this
   mirrors the existing seal sequence — investigate the current
   `seal_and_rotate` ordering before adding to it, to avoid disturbing
   existing crash-safety invariants this campaign has already hardened
   in earlier tasks, e.g. #494's WAL fixes).
4. This MUST be format-VERSION-tolerant: a footer/sidecar is a NEW,
   OPTIONAL addition. Existing on-disk segments from before this change
   have no footer and must continue to open correctly via the fallback
   replay path — do not require a migration tool or reject old
   segments.

## Scope-down guidance

If investigation reveals a genuine blocker (e.g. the footer approach
would conflict with an existing crash-safety invariant from #494, or
the sidecar-file approach has an unresolved atomicity problem specific
to this codebase's file-rotation logic), STOP and document the
specific blocker + a follow-up task description, per this campaign's
established pattern — but this finding's OWN fix sketch is
additive/optional by design, so a genuine blocker should be rarer here
than in some other structural findings this campaign has deferred.

## TDD/regression requirement

1. A regression test proving a segment WITH a valid footer/sidecar
   skips the full replay (e.g. via an instrumented/counting mechanism,
   or by corrupting the segment's DATA in a way that would fail replay
   but leaving the footer intact — if `open` still succeeds using just
   the footer's max_version, that proves replay was skipped).
2. A regression test proving a segment WITHOUT a footer (simulating an
   old-format segment) still opens correctly via the fallback replay
   path, with the SAME `max_version` result as before this change.
3. A regression test proving a segment with a CORRUPTED/torn footer
   (e.g. wrong checksum, or a footer write that's truncated) correctly
   falls back to replay rather than trusting bad data or crashing.
4. A crash-safety test: simulate a crash between the data fsync and the
   footer write (footer entirely absent) — confirm this degrades
   gracefully to the replay fallback, not corruption.

## Performance verification requirement (MANDATORY — this is a PERF task)

Per this repo's `/opti` methodology: add a soak-style bench (the audit
explicitly notes NO bench exists for WAL startup/recovery time — "нужен
soak: накопить N сегментов → замерить open"). Accumulate N sealed
segments of realistic size, measure `SegmentSet::open` time before
(full replay of every segment) vs after (footer/sidecar fast path).
Report honest before/after numbers with the speedup ratio, following
this repo's `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo
bench` convention.

## Test scope

```
./scripts/test.sh -p shamir-wal
./scripts/test.sh -p shamir-tx
```

## Gate

```
cargo fmt -p shamir-wal -p shamir-tx -- --check
cargo clippy -p shamir-wal -p shamir-tx --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Investigation] Status: complete
  > Re-examined the prior "format doesn't support backward-seek" deferral
    reasoning from task #489 — still applies / does not apply, and why.
  > Feasibility verdict for the footer/sidecar approach.

[Implementation] Status: fixed / partially-fixed / deferred
  > What changed + regression tests added (if fixed/partial)
  > Bench: baseline (full replay) vs after (footer fast path), at N
    sealed segments of realistic size
  > OR: deferral reason + follow-up task description (if deferred)
```
Full test/gate/bench results (exact commands + pass/fail).
