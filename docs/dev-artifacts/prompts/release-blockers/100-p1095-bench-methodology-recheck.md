# #1109 MEDIUM — #1095's "confirmed negligible" conclusion rests on a bench amortization artifact

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a final adversarial review of the whole session's accumulated work
(commit range `b556913f..HEAD`). This is a methodology critique of `#1095`'s
own investigation, not a newly discovered runtime bug — but `#1095`'s
finding was written into the NORMATIVE `CLAUDE.md` (removing
`SegmentSet::inner` from tracked concurrency debt), so an inaccurate
methodology there has downstream consequences: a future contributor trusting
`CLAUDE.md` won't reconsider this lock.

Files: `crates/shamir-wal/benches/segment_set_lock.rs`; the `CLAUDE.md`
paragraph added by `#1095` under "Investigated and closed — confirmed
negligible" (search for `SegmentSet::inner` — it currently cites `#1095` and
the date `2026-08-13`).

## The claim as currently written

`CLAUDE.md` says: "per-append latency IMPROVING 5x with concurrency (n_1:
99us -> n_64: 19.7us per op) — the lock's own amortized cost drops under
contention", used to justify "Investigated and closed — confirmed
negligible" and removing `SegmentSet::inner` from tracked debt.

## Why this is a measurement artifact, not evidence about the lock

Confirmed by direct reading of `segment_set_lock.rs`: `fan_out_raw(segset, n,
version_base)` spawns `n` `tokio::spawn` tasks (each calling
`segset.append_batch(...)`) and joins all of them — and this ENTIRE
fan-out-and-join round trip is what `h.bench_async(&id, ...)` times as ONE
iteration (see the `bench_scale_tool::Harness` usage at `:99-108`). So the
reported `ns/op` for `raw_append/n_64` is the cost of spawning 64 tasks,
running 64 appends, and joining 64 handles, divided by... nothing — it's
reported as one iteration's cost, and `#1095`'s own interpretation divides
it by `n` to get a "per single append" figure.

A mutex-protected critical section cannot get 5x FASTER per acquisition as
contention rises — that isn't how lock contention works; contention can only
add wait time, never subtract it. What the falling "per-op" number actually
shows is that the FIXED per-iteration overhead (spawning `n` tasks, the
runtime's scheduling, joining `n` handles) is amortized across `n` real
appends at `n=64` but paid in full (for `n=1` "spawn/join") at `n=1`. A
falling per-op curve under this harness shape is evidence the harness is
measuring fan-out/join overhead diluted by `n`, not evidence the lock itself
gets cheaper under contention.

Separately: the bench's own header comment already states it "creates an
ARTIFICIAL scenario that never occurs in production" (bypassing
`WalGroupCommit`'s leader election entirely, so all N tasks genuinely
contend on the mutex simultaneously — a scenario production structurally
avoids). So even a methodologically clean reading of these specific numbers
couldn't speak to production contention on its own — the actual load-bearing
argument for "no migration needed" is the PRE-EXISTING architectural
single-writer argument (only the group-commit leader, elected via a single
`AtomicBool` CAS in `WalGroupCommit`, ever calls into `SegmentSet` in
production), which stands on its own and predates `#1095`.

**This brief is NOT claiming the mutex IS a problem.** The underlying "no
migration warranted" conclusion may well still be correct on the strength of
the architectural single-writer argument alone. It's claiying the BENCH, as
currently written and interpreted in `CLAUDE.md`, doesn't establish what the
`CLAUDE.md` text says it establishes.

## Required work

1. Either (a) design a bench that isolates PER-ACQUISITION lock cost
   independent of fan-out/join amortization — e.g. measure the wall-clock of
   just the critical section itself (the mutex-guarded region inside
   `append_batch`) under a controlled, sustained contention level, not the
   wall-clock of a whole spawn-N-and-join-N round trip divided by N — OR (b)
   conclude that isolating it cleanly with this harness shape isn't
   practical, and say so explicitly, relying on the architectural
   single-writer argument alone (state that plainly, don't paper over it
   with a reinterpreted version of the same flawed numbers).
2. Correct the `CLAUDE.md` entry to reflect whichever conclusion the
   corrected methodology (or the explicit architectural-argument-only
   framing) actually supports. Do not leave the current
   amortization-confounded "5x improvement" interpretation as normative
   guidance — even if the bottom-line conclusion (no migration needed)
   stays the same, the REASONING given for it must be sound, since this text
   is what a future contributor will trust without re-deriving it.
3. If the corrected bench (or a closer look at genuinely production-shaped
   contention) reveals the lock is NOT in fact negligible, that reopens
   `SegmentSet::inner` as tracked migration debt — restore that framing in
   `CLAUDE.md` rather than silently leaving it closed. Do not force a
   predetermined conclusion either direction; follow what the corrected
   measurement actually shows.

Keep the existing `segment_set_lock.rs` bench file's raw fan-out scenario if
it's still useful for other purposes (e.g. proving lock CORRECTNESS under
worst-case contention, which is a separate, legitimate question from lock
COST) — add a new bench function/scenario for the isolated-cost measurement
rather than deleting the existing one, unless you determine it's genuinely
redundant once the new one exists.

## Gate

```
cargo fmt -p shamir-wal -- --check
cargo clippy -p shamir-wal --all-targets -- -D warnings
./scripts/test.sh -p shamir-wal --full
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-wal --bench segment_set_lock
```

All must pass clean. Report: what did the corrected methodology (or the
explicit architectural-argument-only framing) actually conclude about
`SegmentSet::inner`? Did `CLAUDE.md` change, and how? Real bench numbers,
not a paraphrase.
