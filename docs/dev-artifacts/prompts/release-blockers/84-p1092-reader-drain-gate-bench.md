# #1092 phase 1 — benchmark `ReaderDrainGate` before any architectural change

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`crates/shamir-index/src/reader_drain_gate.rs` is a correct (verified in
#1081) but potentially expensive reader-vs-DROP exclusion gate: every hot
read pays a `SeqCst fetch_add` + `SeqCst load` on `enter()`, and a `SeqCst
fetch_sub` on `ReadGuard::drop`. A 2026-08-09 review
(`docs/dev-artifacts/research/2026-08-09-new-wave-release-readonly-review-codex.md`)
flagged this as worth measuring before any architectural change — the
module's own doc (lines 88-97) already states granularity was chosen
"not worth it without measurement", i.e. the review's asks (per-index
granularity, weaker Acquire/Release ordering, `Notify`-based wakeup) are
explicitly NOT to be implemented without a benchmark justifying them
first.

**This brief is deliberately scoped to ONLY the benchmark.** Do not touch
`reader_drain_gate.rs`'s logic, ordering, or granularity. Do not add loom
infrastructure. Do not implement a `Notify`-based wakeup. Those are
follow-up work, gated on what this benchmark shows.

## What to build

A new bench file `crates/shamir-index/benches/reader_drain_gate.rs`,
following the exact established pattern in this crate — copy the shape of
`crates/shamir-index/benches/posting_cache_hit.rs` (imports, `rt()`
helper, `Harness::new(...)`, the `fn main()` loop structure) rather than
inventing a new one. Uses `bench_scale_tool::Harness`, **NOT Criterion**
(see this repo's `CLAUDE.md` — Criterion was fully migrated off in
2026-07-07; do not reach for `criterion_group!`/`criterion_main!` from
memory/training data).

### What to measure

The review's own ask: "benchmark indexed QPS/latency before/after the gate
at 1/8/32/64 threads". Concretely:

1. **Baseline cost of `enter()`+drop with no concurrent DROP** — the
   common-case hot path. Spawn N concurrent tokio tasks (N ∈ {1, 8, 32,
   64}), each looping `gate.enter()` → (simulate a trivial read, e.g. a
   no-op or a cheap `black_box`) → drop the guard, for a fixed iteration
   count per task. Measure wall-clock ns/op (total wall time / total ops
   across all tasks), to see how the gate's SeqCst RMWs scale under
   increasing concurrent-reader contention.
2. **A "gate absent" control** — the same workload with the gate calls
   removed entirely (or replaced with a true no-op), at the same thread
   counts, to isolate the gate's own marginal cost from the workload's own
   overhead (task scheduling, etc.). This is the "before/after" comparison
   the review asks for — "before" being "as if there were no gate",
   "after" being "with the gate in the loop", since the gate has always
   existed in this codebase (there's no literal historical "before"
   binary to revert to, same honest-reporting situation
   `posting_cache_hit.rs`'s own doc comment describes — follow that
   file's precedent for how to phrase this in your own bench's doc
   comment).
3. **Cost of a concurrent DROP's drain window on sibling reads** — with
   readers continuously calling `enter()` on index A, trigger a
   `begin_drop()`+`wait_for_drain()` cycle on the SAME gate (simulating
   the "index A drop stalls sibling index B reads" collateral effect the
   review flags, since the gate is per-manager not per-index) and measure
   how long readers spend getting `None` back (falling back) during that
   window, and how long the drop's `wait_for_drain()` itself takes to
   observe zero in-flight readers under sustained contention.

Use `IndexManager::new`/`create_index`/`on_record_created`/
`lookup_by_index` (the real production call path, which internally uses
`ReaderDrainGate`) rather than calling `ReaderDrainGate` directly in
isolation, so the measured numbers reflect actual indexed-read cost, not
a synthetic microbenchmark divorced from the real call site — mirror
`posting_cache_hit.rs`'s `build_manager_with_hot_posting` helper shape for
this. If measuring the gate in isolation (bullet 1/2 above) is clearer
and more direct for isolating JUST the gate's own cost, that is also
acceptable — use your judgment, but include at least one measurement
through the real `IndexManager` call path so the numbers are
interpretable against real query latency, not just raw atomic-op cost.

## Deliverable

1. The bench file itself, runnable via:
   ```
   CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p shamir-index --bench reader_drain_gate
   ```
   (per this repo's `CLAUDE.md` bench-cache-isolation rule — use the
   dedicated target dir, not the default one test/clippy use).
2. Run it and capture the actual output numbers.
3. Write a short summary (a few sentences, in your final report — do NOT
   create a new markdown doc file) stating:
   - The gate's marginal ns/op cost at each thread count, and whether it
     scales linearly, sub-linearly, or shows a cliff under contention.
   - Whether the numbers are large enough (relative to a typical indexed
     lookup's total cost — you may need to also glance at
     `posting_cache_hit.rs`'s own numbers for a size-of-effect comparison)
     to justify the review's proposed changes (per-index granularity,
     ordering relaxation, `Notify` wakeup), or whether the gate's cost is
     already negligible in context.
   - Your own recommendation: is this worth pursuing further (and if so,
     which specific change from the review's list looks most justified by
     the numbers), or is the current per-manager/SeqCst/spin-wait design
     adequate as-is given the measured cost?

## Gate

```
cargo fmt -p shamir-index -- --check
cargo clippy -p shamir-index --all-targets -- -D warnings
```
(No behavior change to production code, so `./scripts/test.sh` should be
unaffected, but run `./scripts/test.sh -p shamir-index` anyway to confirm
nothing broke.)

Do not touch anything outside the new bench file. No production code
changes in this pass — this is measurement-only, exactly as the source
review itself requires before any of its other suggestions are acted on.
