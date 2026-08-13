# #1103 PERF (low priority) — ReaderDrainGate phase 2: loom infra for shamir-index + evaluate ordering relaxation against real contention data

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Follow-up to `#1092` (completed, benchmark phase). The benchmark
(`crates/shamir-index/benches/reader_drain_gate.rs`, commit `a082c47c`) found
`ReaderDrainGate`'s marginal `SeqCst` cost roughly doubles under real
multi-core contention (~62ns/op uncontended → ~112–123ns/op at 8–64
concurrent readers, plateauing past 8-way) — a meaningful (~15–17%) but not
dominant fraction of a real ~730ns indexed lookup. This is real, measured
data but does not by itself mandate an architectural change — this is
explicitly LOW PRIORITY, real-but-modest debt, not a production-blocking
issue.

## Scope — do exactly this, in this order, and stop where the brief says to

1. **Wire up loom model-testing infrastructure for `shamir-index`**, mirroring
   `shamir-engine`'s EXACT existing pattern for
   `crates/shamir-engine/src/table/writer_drain_barrier.rs`'s `loom_model`
   module (F-56/#882). `shamir-index` has NONE of this today. Concretely:
   - Copy `crates/shamir-engine/build.rs`'s pattern into a new
     `crates/shamir-index/build.rs`: emit `cargo::rustc-cfg=loom` gated on
     `CARGO_FEATURE_LOOM` (read that file in full — it's ~16 lines, copy the
     exact mechanism and its doc comment's reasoning, adjusted for this crate).
   - Add to `crates/shamir-index/Cargo.toml`: a `loom = []` feature (mirror
     `shamir-engine/Cargo.toml`'s exact `loom = []` line and its surrounding
     doc comment) and a `loom = "0.7"` dev-dependency, gated the same way
     `shamir-engine`'s is (`[target.'cfg(loom)'.dev-dependencies]` or
     equivalent — read `shamir-engine/Cargo.toml`'s exact section for the
     precise TOML shape and copy it, adjusting only crate-specific bits).
2. **Write a `#[cfg(loom)] mod loom_model` inside
   `crates/shamir-index/src/reader_drain_gate.rs`**, proving the CURRENT
   `SeqCst` ordering preserves the memory-model proof already documented in
   that file's own module doc (read it in full — the two-case proof: reader's
   `fetch_add`-then-load vs. drop's store-then-wait, plus the termination
   argument). Mirror `writer_drain_barrier.rs`'s `loom_model` module
   structure as closely as the role-swap allows (`ReaderDrainGate` is
   `WriterDrainBarrier`'s own mirror-image per this file's module doc, so
   the loom model should be a near-direct role-swapped port):
   - A minimal `Model` struct: `in_flight: AtomicUsize`, `dropping: AtomicBool`,
     `reader_read: AtomicBool` (models a reader's read of the posting store
     landing), `read_at_drop_return: AtomicBool` (snapshot taken by the
     DROP thread at the exact instant its wait loop exits — analogous to
     `writer_drain_barrier.rs`'s `wrote_at_drain_return`, and for the SAME
     reason: sampling post-`join()` would be tautological, see that file's
     doc for why).
   - `run_reader`: `in_flight.fetch_add(1, SeqCst)` → (loom-only
     compensating fence, per `writer_drain_barrier.rs`'s own comment about
     loom 0.7's no-op SeqCst-access enforcement — copy that exact pattern)
     → `dropping.load(SeqCst)` → if `false`, model the read
     (`reader_read.store(true, SeqCst)`) and return `Some`; else
     `fetch_sub` and return `None`.
   - `run_drop`: `dropping.store(true, SeqCst)` → fence → spin on
     `in_flight.load(SeqCst) != 0` → snapshot `reader_read` into
     `read_at_drop_return` at the instant the spin exits.
   - One test, `drop_wait_returns_only_after_in_flight_reader_completes` (or
     similar name), asserting: if the reader took the fast path (returned
     `Some`), then `read_at_drop_return` must be `true` — i.e. the DROP's
     wait cannot return before an already-in-flight reader's read has
     landed. This is the SAME invariant shape as `writer_drain_barrier.rs`'s
     `drain_returns_only_after_fast_path_writer_completes`, roles swapped.
   - Run it and confirm it passes:
     `cargo test -p shamir-index --features loom --lib reader_drain_gate::loom_model -- --nocapture`
3. **STOP after step 2 passes and report.** Per the task's own scope: "ONLY
   once the loom model passes for a proposed relaxation, evaluate whether the
   ~2x contended-cost reduction (if any) it would buy is worth the added
   ordering-reasoning complexity, using the phase-1 benchmark numbers as the
   'is this worth it' yardstick." This means:
   - Step 2 above proves the CURRENT `SeqCst` ordering is sound (a baseline/
     regression-guard model, valuable on its own even with no ordering
     change).
   - Do NOT attempt an `Acquire`/`Release` relaxation in this same pass
     unless step 2's model is solid AND you have a genuinely low-risk,
     well-reasoned relaxation candidate — this is unfamiliar territory
     (loom model-checking) for this crate, and a wrong-direction ordering
     change on a proven-correct primitive is worse than the status quo.
     If you do attempt one: write a SECOND loom model for the relaxed
     ordering, confirm it ALSO passes, and only then would production code
     change — and only with a clear before/after bench comparison using the
     EXISTING `crates/shamir-index/benches/reader_drain_gate.rs` (same
     approach `#1099`'s `p1099_touched_probe.rs` used: temporarily revert
     just the ordering change and re-run the SAME bench binary for genuine
     before numbers).
   - If a relaxation attempt does NOT clearly pass its own loom model, or
     the reasoning gets shaky, ABANDON it and report why — leaving
     `ReaderDrainGate` on `SeqCst` with a NEW loom regression-guard is a
     fully successful, valuable close for this task. Do not force a
     relaxation that doesn't clearly hold up.
4. **Do NOT implement per-index granularity or `Notify`-based wakeup** (the
   original review's other two asks) — the task's own scope note says
   neither is justified by the phase-1 bench data (contention plateaus past
   8 threads rather than growing unboundedly; concurrent-DROP-drain already
   resolves promptly). Leave both alone entirely.

## Gate

```
cargo fmt -p shamir-index -- --check
cargo clippy -p shamir-index --all-targets -- -D warnings
./scripts/test.sh -p shamir-index --full
cargo test -p shamir-index --features loom --lib reader_drain_gate::loom_model -- --nocapture
```

All four must pass clean (note: the loom test does NOT run under
`./scripts/test.sh` — it needs the explicit `--features loom` invocation
shown above, same as `shamir-engine`'s `writer_drain_barrier::loom_model`).

Report: did the loom model for the CURRENT ordering pass? Did you attempt a
relaxation — if so, what happened (passed and shipped with before/after
bench numbers, or abandoned and why)? Real gate pass/fail counts.
