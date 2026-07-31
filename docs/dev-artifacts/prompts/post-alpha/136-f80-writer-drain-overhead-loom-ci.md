# F-80 (#907) — measure writer-drain overhead + add a loom CI job

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`WriterDrainBarrier` (`crates/shamir-engine/src/table/writer_drain_barrier.rs`)
is the reusable primitive every writer's fast path checks before proceeding
lock-free through validate→write→index, and every DDL path uses to drain
in-flight writers before it can safely mutate schema. It has been through
three remediation rounds this wave:

- F-56 (#882) fixed its memory-ordering (all four cross-atomic ops now
  `SeqCst`) and added a `#[cfg(loom)]` model-checker module, gated behind
  the `loom` cargo feature (`build.rs` emits `cfg(loom)` only for this
  crate, so dependencies with their own incompatible `#[cfg(loom)]` blocks
  aren't dragged in). Run manually today via:
  `cargo test -p shamir-engine --features loom --lib table::writer_drain_barrier::loom_model -- --nocapture`
- F-69 (#896) collapsed six OR'd conditions (`needs_write_barrier()`) into
  one packed `Arc<AtomicU8>` (`WriteBarrierFlags`), extending F-56's proof
  to the whole predicate.
- F-70 (#897) fixed a real lock-order-inversion deadlock between DDL's
  lock-then-drain and commit's drain-guard-then-lock paths, establishing a
  canonical drain-then-lock ordering for DDL (see the module's top-level
  doc, "THE canonical lock-order hierarchy").

Two gaps remain, both flagged in the 2026-07-30 remediation roadmap as P1
follow-ups, not yet done:

1. **No overhead measurement.** Every writer's fast path now pays for: one
   `WriteBarrierFlags::any_set()` SeqCst load, `enter_writer`'s SeqCst
   fetch_add, and (on the barriered path) participation in a drain. F-69's
   collapse to one atomic word should have made the common (no-barrier)
   case cheaper than the original six-flag OR, but nobody has measured it.
   We need real numbers: per-op latency added to a write on the fast path
   (barrier absent) vs the barriered path (DDL in flight), before vs after
   F-69/F-70's shape — using this repo's actual benchmarking convention.
2. **The loom model never runs in CI.** It exists, it's referenced in three
   places' doc comments as the proof mechanism, but `ci.yml` has no job
   that invokes it — a future edit to this file could silently break the
   proven memory-ordering property and nothing would catch it before a
   human happens to run the loom command by hand.

## What to actually do

### 1. Writer-drain overhead benchmark

Add a bench using **`bench_scale_tool::Harness`** (NOT Criterion — see
CLAUDE.md's "Benches use `bench_scale_tool::Harness`" section; copy the
shape of an existing bench file such as
`crates/shamir-engine/benches/f78_writer_latency.rs` or
`crates/shamir-engine/benches/tx_pipeline.rs` as your template). Measure,
at minimum:

- **Fast-path cost, no barrier active**: `enter_writer`/exit latency (or
  the smallest reachable wrapper around it) when
  `WriteBarrierFlags::any_set()` is false — this is the "tax on every
  write" number that matters most, since it runs unconditionally on every
  write to a barriered table.
- **Barriered-path cost**: latency when a DDL has raised the intent bit
  and a writer must actually participate in / wait behind a drain.
- Run it with `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench` per
  CLAUDE.md's bench-cache-isolation rule so it doesn't invalidate the
  test/clippy incremental cache.

Report the actual measured numbers (not estimates) in the commit message:
ns/op for the no-barrier fast path, and for the barriered path, plus
whatever `bench-iters.txt` records after a run. If you can compare
against a pre-F-69 shape (e.g. by checking out the six-flag version in a
scratch location — do NOT use git checkout on the working tree, use
`git show <old-sha>:<path>` to read old source into a scratch file if you
need a comparison baseline), note the delta; if that comparison is
impractical within scope, it's fine to report only the current numbers
and say so explicitly — do not block the task on reconstructing history.

### 2. Wire the loom model into CI

Add a new job to `.github/workflows/ci.yml` (model it on the existing
`test`/`clippy` jobs' structure: pinned `dtolnay/rust-toolchain` action,
`Swatinem/rust-cache`, etc. — match the SHA-pinning convention already in
this file, do not introduce a bare `@v2`-style unpinned action reference).
The job should run:

```
cargo test -p shamir-engine --features loom --lib \
    table::writer_drain_barrier::loom_model -- --nocapture
```

Decide, and justify in the commit message:
- Which existing job this belongs near (a new standalone job vs. a step
  added to `test`) — a new standalone job is likely cleaner since it needs
  a different feature flag and loom model-checking can be slower than a
  normal test run.
- Whether it needs to run on every PR or is better scheduled/matrixed
  narrower (e.g. `ubuntu-latest` only, since loom model-checking is CPU
  work and doesn't depend on OS-specific I/O the way the `integration` job
  does) — a single-OS job is the right default unless you find a concrete
  reason the model depends on OS behavior (it shouldn't — loom mocks the
  atomics themselves).
- Whether this job should be a hard PR gate (blocks merge) or informational
  — given CLAUDE.md's TDD discipline and that this loom model is the
  actual proof backing three P0 fixes this wave, a hard gate is the
  correct default; only deviate if you find a concrete reason (e.g. loom
  model runtime is too long/flaky for per-PR gating) and document it.

Confirm the job actually runs and passes by triggering it locally with the
exact command above before committing (loom tests do not run under
`./scripts/test.sh` — they are intentionally excluded, per the Cargo.toml
comment quoted above — so this is a legitimate direct-`cargo test`
invocation for THIS ONE case; do not extend this exception elsewhere).

## Definition of done

- New bench file (or extension of an existing one) under
  `crates/shamir-engine/benches/`, using `bench_scale_tool::Harness`,
  reporting real fast-path and barriered-path latency numbers in the
  commit message.
- `.github/workflows/ci.yml` gets a new job (or step) that runs the loom
  model on every PR (or documents why not), using the SHA-pinned action
  style already established in this file.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine --full` green (loom itself is NOT
  run through this wrapper — verify it separately with the direct command
  above, once).
- Do not touch `writer_drain_barrier.rs`'s existing logic — this task is
  measurement + CI wiring, not a behavior change. If the benchmark reveals
  a regression worth fixing, STOP and report it rather than fixing it
  silently inside this task — F-80 is scoped to measure and wire, not to
  optimize.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
