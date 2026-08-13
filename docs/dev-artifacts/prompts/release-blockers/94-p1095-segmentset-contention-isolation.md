# #1095 PERF — isolate `SegmentSet::inner`'s own contention contribution before touching it

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

⛔ This is the highest-blast-radius code in the workspace (WAL durability —
`crates/shamir-wal/`). **Do NOT modify `crates/shamir-wal/src/segment_set.rs`
(or any other WAL production file) unless your OWN isolated measurement in
this task PROVES real, non-negligible contention.** A "measured it, found it
negligible, documented the finding, changed no production code" outcome is
a fully successful, valuable close for this task — it is NOT a failure to
find something to fix. Do not migrate the lock speculatively "to be safe."

## Background

`SegmentSet::inner: std::sync::Mutex<Inner>` (`crates/shamir-wal/src/segment_set.rs`)
is flagged in `CLAUDE.md`'s "NOT covered by the above — hot-by-call-frequency,
tracked migration candidates" section as debt from `#1090`: it's taken on
every `append_batch` call (high call frequency), but `#1090`'s own
investigation found the existing purpose-built contention probe
(`crates/shamir-wal/benches/wal_append.rs`, `wal_append` bench) showed
per-op latency rising with concurrency at 1/4/16/64 concurrent committers —
but the SAME rise appeared in the `mem` sink scenario, which uses
`WalSink::mem()` and **never touches `SegmentSet` at all**. That means the
growth is NOT yet attributable specifically to `SegmentSet::inner`'s lock —
it's more likely `WalGroupCommit.pending: tokio::sync::Mutex<Vec<Pending>>`
+ the leader-election CAS coordination (both of which the `mem` scenario
DOES exercise, since `WalGroupCommit` wraps every sink including `mem`).

`#1095` exists to close that attribution gap: isolate `SegmentSet::inner`'s
OWN contribution to the observed latency rise, separate from
`WalGroupCommit`'s own coordination layer, BEFORE deciding whether a
lock-free migration is even warranted.

## Why this task is plausibly a "no-op, confirmed clean" close — read before benching

Read `wal_group_commit.rs` in full, in particular the leader-election
mechanism: `append`/`append_batch` elect a single leader via ONE
`AtomicBool::compare_exchange` on `flushing` (~line 182, 244). Only the
tasks that win this CAS ever go on to call into the underlying `WalSink`
(and therefore `SegmentSet`) — every other concurrent committer just pushes
into `pending` and parks on a `Notify` until the leader's window includes
their entry. This means `SegmentSet::inner`'s mutex is, by this
architecture, **already serialized to a single acquirer at a time** for the
append path, regardless of how many concurrent committers are queued up in
`WalGroupCommit` — the queueing/serialization happens ONE LEVEL UP, before
`SegmentSet` is ever touched. `SegmentSet`'s own doc comment
(`segment_set.rs` ~line 52-60) makes exactly this claim ("single-writer
model is the group-commit leader... plus the truncator (rare); they do not
contend on a hot path").

If that architectural claim holds, `SegmentSet::inner`'s `std::sync::Mutex`
should show **negligible-to-zero measurable contention** regardless of
concurrency, because there is structurally never more than one concurrent
acquirer on the append path — a migration to a lock-free structure would
have no measurable benefit and would be a purely speculative rewrite of the
highest-risk code in the workspace. Your job is to verify or refute this
claim empirically, not assume it.

## What to build

1. **A new, isolated bench** (new file, e.g.
   `crates/shamir-wal/benches/segment_set_lock.rs`, following this
   codebase's `bench_scale_tool::Harness` + `bench_batched_async` convention
   — copy `wal_append.rs`'s structure/style) that measures `SegmentSet`'s
   OWN append/lock path DIRECTLY, bypassing `WalGroupCommit`'s
   `pending`-queue and leader-election CAS entirely. Call `SegmentSet`'s
   append method(s) concurrently from N tasks (same concurrency sweep as
   `wal_append.rs`: 1/4/16/64) and measure per-op latency. This isolates
   whatever `SegmentSet::inner`'s mutex itself contributes, independent of
   `WalGroupCommit`'s coordination layer.
   - Investigate `SegmentSet`'s public API (`crates/shamir-wal/src/segment_set.rs`)
     to find the right entry point — likely `append`/`append_batch` or
     whatever `WalSink::File`'s append delegates to. If `SegmentSet`'s
     append method isn't `pub` (only `pub(crate)` or private), check
     whether the bench can reach it as `shamir-wal` bench code (benches
     compile against the crate's public surface only — same constraint
     `wal_append.rs`'s own doc notes: "Bench-only crate API used is fully
     public... No prod-code visibility was widened"). If the real append
     entry point isn't reachable without widening visibility, that's a
     legitimate finding to report — don't widen `pub` surface just to
     make the bench work; investigate whether `SegmentSet`'s existing
     public methods (used by `WalSink::File`) are already sufficient.
2. **Also investigate**: does truncation (`SegmentSet::truncate_below` or
   equivalent) ever run CONCURRENTLY with an in-flight append in the real
   commit path (not just theoretically)? If truncation and append can
   genuinely overlap in time under load, that's a SECOND acquirer beyond
   the single leader, and worth a targeted concurrent bench scenario
   (append + truncation racing) in addition to the append-only sweep.
3. **Run the new bench** (`CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench
   cargo bench -p shamir-wal --bench segment_set_lock`, per this repo's
   established bench-cache-isolation convention) and record the real
   numbers: does per-op latency rise with concurrency, or does it stay flat
   (confirming the single-acquirer architecture)?
4. **Decide, based on YOUR OWN measured numbers** (not assumption):
   - **If latency stays flat / negligible rise across 1→64 concurrency**:
     the architectural claim holds. Do NOT migrate the lock. Update
     `CLAUDE.md`'s "NOT covered by the above" section for
     `SegmentSet::inner` to record this task's conclusion — replace the
     current "growth is not yet attributable... needs isolation" text with
     the actual isolated measurement and the conclusion that it's
     confirmed negligible, so this stops being "tracked debt" (either move
     it into the third sanctioned-exception category if it now clearly
     fits one, or state plainly why it's closed as investigated-and-clean
     without forcing it into an existing category — use your judgment,
     explain your reasoning). This is the expected, successful outcome if
     the architecture is what the doc comments claim.
   - **If latency DOES rise meaningfully with concurrency in the isolated
     bench** (i.e., the architectural single-acquirer claim is WRONG, or
     something else — cache-line bouncing across the leader-CAS handoff,
     truncation racing, lock poisoning checks, etc. — genuinely costs
     something under concurrency): THEN, and only then, propose a
     lock-free migration (`arc_swap::ArcSwap` for the `active` handle +
     a lock-free structure for the sealed list are the natural fits per
     this codebase's concurrency table in `CLAUDE.md`). Implement it with
     the SAME TDD discipline as every other fix this session: a failing
     test/bench-based Red step proving the contention, the fix, a
     before/after bench comparison (like `#1099`'s `p1099_touched_probe.rs`
     — temporarily revert just the fix and re-run the SAME bench binary
     for genuine before numbers, not a synthetic estimate), full gate,
     and a mutation test if the fix has a correctness-sensitive shape (a
     lock-free rewrite of WAL segment/truncation bookkeeping absolutely
     needs its own dedicated correctness tests beyond the perf bench —
     truncation racing with append, rotation racing with truncation,
     recovery-after-crash semantics must all stay correct; do not treat
     this as a pure perf refactor if you choose this branch).

## Gate

```
cargo fmt -p shamir-wal -- --check
cargo clippy -p shamir-wal --all-targets -- -D warnings
./scripts/test.sh -p shamir-wal --full
```

If you choose the migration path, also add: any new correctness tests the
migration needs, run to green, plus a mutation test proving they catch a
regression.

Report the real measured numbers (both the isolated `SegmentSet` bench and,
if you touched the group-commit path, before/after `wal_append` numbers),
your conclusion (migrate vs. no-op), and full reasoning for whichever path
you took — this decision matters more than the line-level diff, per this
session's established standard.
