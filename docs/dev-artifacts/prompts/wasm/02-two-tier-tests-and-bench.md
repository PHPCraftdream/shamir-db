# Brief: two-tier WASM compilation, tests and measurement cells (TIER-3)

The core is implemented and committed (`feat(wasm-host): two-tier compilation core`). Read
`docs/dev-artifacts/design/wasm-two-tier-compilation.md`, then the code under
`crates/shamir-wasm-host/src/wasm/engine/` (`tier_upgrade.rs`, `wasm_engine.rs`, `tier*.rs`),
`crates/shamir-wasm-host/src/wasm/wasm_function.rs`, the existing tests under
`crates/shamir-wasm-host/src/tests/wasm/` (especially `wasm_tests.rs`, `fuel/`,
`wasm_engine_tests.rs`), the bench `crates/shamir-wasm-host/benches/wasm_procedures/`, and `CLAUDE.md`.

## Compile / build / test policy (verbatim, mandatory)

**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`,
`npm`/`tsc`/`vitest`, WASM builds, e2e and integration harnesses, any release or full-workspace
build, and any other command whose purpose is to build the project or run its tests or
benchmarks. Allowed locally, scoped to the crates you touch (`-p <crate>`):
`cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` and `-- --check`, `python -I scripts/ci/check_layout.py` (it reads tracked
files only: new files count only after staging, so also verify folder entry counts by hand).
Set `CARGO_TARGET_DIR` to a directory OUTSIDE the repository for those commands. If you believe
you need a build or test run, say so in your final message; do not run it.

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
> command that mutates the working tree or index. Only edit files; the orchestrator commits.

No machine-local absolute paths in any file you write. Do not mention how or where CI is run.
Layout limits: at most 7 direct entries per folder, 1000 lines per source file. The folder
`crates/shamir-wasm-host/src/tests/wasm` already has 7 entries, so it cannot gain a file or a
folder: create `tests/wasm/engine/`, MOVE `wasm_engine_tests.rs` into it, put the new tier test
files next to it (one topic per file, a `mod.rs` manifest that only declares modules), and update
`tests/wasm/mod.rs`. In `benches/wasm_procedures/` there are 7 entries: extend existing files
(or fold two small ones) instead of adding files. Tests follow the repo layout rules: no inline
`#[cfg(test)] mod tests`, imports at the top, expected values multi-line and readable.

## Part A: tests (`crates/shamir-wasm-host/src/tests/wasm/engine/`)

Use `WasmEngineOptions { tier: TierOptions { enabled, hot_calls, background_jobs }, ..Default::default() }`
with a small `hot_calls` (for example 3) so the upgrade happens inside a test. WAT guests: reuse
the identity and compute modules from `benches/wasm_procedures/kernels.rs` (copy the few lines you
need; do not import from a bench). Waiting for the background upgrade: `tokio::time::timeout`
around a loop that yields or sleeps briefly and checks `WasmFunction::tier()` or
`engine.tier_stats()`; never an unbounded wait. Required cases, each its own `#[tokio::test]`
with an assertion that fails on the opposite behaviour:

1. Results identical before and after the upgrade (same kernel, same params, compare values).
2. The slot is `Tier::Fast` before the threshold and `Tier::Optimized` after it (`tier()`), and
   `upgrades_completed == 1` in `TierStats`.
3. Exactly one upgrade under a race: many concurrent calls crossing the threshold together give
   `upgrades_started == 1`.
4. A call that started before the swap finishes correctly (hold a call open with a guest that
   awaits a host import, trigger the upgrade meanwhile, finish the call, check its result).
5. `TierMode::Never` stays on `Fast` after far more than `hot_calls` calls and starts no upgrade.
6. `TierMode::Now` is `Optimized` right after creation and starts no upgrade.
7. `TierOptions { enabled: false, .. }`: `Auto` and `Never` functions are created on the
   optimized tier and never upgrade (this was a real defect; the test must fail without the fix).
8. A fast-tier compile failure falls back to optimized: use a module the Winch backend rejects if
   you can find one that Cranelift accepts; if none exists, add a minimal `#[cfg(test)]` seam in
   production code (no visibility widening of existing items) that forces the fast compile to
   fail, and say so in your report.
9. A failing upgrade keeps the function working and counts `upgrades_failed`; a PANICKING upgrade
   (inject through a `#[cfg(test)]` seam on the build-pre-instance function pointer or the
   compile step) is counted as failed, logged, and does not leave the state stuck: a later
   crossing of 16x `hot_calls` retries (assert `upgrades_started` goes to 2).
10. Fuel exhaustion and epoch interruption trap identically on both tiers: a spinning guest with
    a tiny fuel budget and a spinning guest with a tiny wall-clock deadline, each run with
    `Never` and with `Now`, same error class (reuse the helpers in `tests/wasm/fuel/`).
11. Both engines' epochs advance: the wall-clock deadline variant of case 10 on the fast tier
    makes it explicit.
12. Dropping the function while its upgrade is queued behind a held permit (`background_jobs = 1`
    and a long job occupying the permit) does not panic and installs nothing.

## Part B: measurement cells (`benches/wasm_procedures/`)

* The existing `compute`, `warm`, `host` and `cold` cells use the default creation mode, which is
  now `Auto` (fast first, upgrade after 64 calls), so their numbers would mix tiers. Pin them to
  the optimized tier with `from_wat_with_mode(.., TierMode::Now)`, keeping their cell names, so
  the baseline stays comparable with the earlier published numbers.
* New cells: `compute/wasm_fast/<kernel>` and `warm/call_identity_fast` (`TierMode::Never`);
  `cold/compile_<identity|fn64|int_sum>_fast` and `_optimized` (`Never` vs `Now`, same engine);
  a compile-scaling pair on a larger generated module (`many_functions(512)`), fast vs optimized;
  `tier/create_and_first_call_auto` (create in `Auto` and call once: the new cold start). Extend
  `scripts/bench/wasm_procedures_report.py` with a `### Tier trade-off` section: per kernel, the
  compile cost difference, the per-call saving of the optimized tier over the fast one, and the
  break-even number of calls (compile_optimized - compile_fast) / (call_fast - call_optimized)
  when the saving is positive. The whole run must still fit the 15 s budget.
* `SHAMIR_WASM_STRATEGY` under `--features bench-strategies` selects the OPTIMIZED engine's
  strategy only; say so in the bench module docs.

## Deliverable and verification

Edit files only. Finish with `cargo fmt -p shamir-wasm-host`, `cargo check -p shamir-wasm-host
--all-targets` and `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings` (also with
`--features bench-strategies`), a manual folder-entry check of every folder you touched, and
`python -I scripts/ci/check_layout.py`. Your final message lists every file changed, added or
moved, each test with the behaviour it pins, any `#[cfg(test)]` seam added in production code,
and anything you could not verify. No claim of completion that the evidence does not support.
