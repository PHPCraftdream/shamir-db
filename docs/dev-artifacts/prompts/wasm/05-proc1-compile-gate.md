# PROC-1 stage B1: the compile gate, the dry-run check and the tier report in `shamir-wasm-host`

Continues `04-proc1-admission.md` (stage A, already in this tree: `admit_module`, `AdmissionPolicy`,
`AdmissionLimits`, `ModuleReport`, `ModuleHash`, `WasmFunction::from_binary_admitted_blocking`). Read the ADR
`docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md` (D2 step 6, D5, the `check_function`
paragraph), `wasm-two-tier-compilation.md`, `CLAUDE.md`, and the stage A code under
`crates/shamir-wasm-host/src/wasm/admission/` first. Out of scope: `shamir-db`, wire types, anything outside
`shamir-wasm-host` (stage B2 does the facade).

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-proc1a`
(outside the repository) and never pipe check/clippy through head/tail. If you believe you need a build
or test run, say so in the final message.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
mutates the working tree or index. Only edit files; the orchestrator commits.

## Repository rules that bind this work

* At most 7 direct entries per folder, at most 1000 lines per source file (`wasm/wasm_function.rs` is at
  952 lines: keep it at or below 985, put new logic in new files instead); `mod.rs` re-exports only; one
  file = one primary export; tests live in `tests/<topic>/` directories, never inline `#[cfg(test)] mod`;
  no local `use`; no visibility widening and no new `#[allow]`; no `impl` blocks for `WasmFunction`
  outside `wasm_function.rs` (rust-guard rule), so new logic is free functions or methods of the types
  that already own an `impl` in the file you edit; comments terse; match the surrounding style.
* Folder entry counts (a new file must not push a folder over 7): `wasm/engine/` is full (7),
  `wasm/` is full (7), `wasm/admission/` has 6, `tests/wasm/` is full (7), `tests/wasm/engine/` has 7,
  `tests/wasm/admission/` has 7, `tests/wasm/admission/gates/` has 4. Put new tests under
  `tests/wasm/admission/gates/`; if you need a second folder level, nest it inside an existing subfolder.
* No machine-local absolute paths in any file. Do not mention how or where CI is run.
* No new dependencies.

## Deliverables

1. **`AdmissionLimits`** gains `max_compile_time: Duration` (default 10 s) and
   `max_concurrent_compiles: usize` (default `max(1, cores / 2)`); update `Default`, the defaults test and
   any literal constructions.
2. **Compile gate** (`wasm/admission/compile_gate.rs`): `WasmEngine` owns one `Arc<tokio::sync::Semaphore>`
   sized by `options.admission.max_concurrent_compiles` (add the field and an accessor in
   `wasm/engine/wasm_engine.rs`). A crate-visible async function runs a blocking job under it:
   acquire an **owned** permit, move the permit **into** the `spawn_blocking` closure (so the permit is
   released only when the compile thread really ends, even after a timeout), and for
   `AdmissionPolicy::Admission` wrap the join in `tokio::time::timeout(max_compile_time)`; on expiry return
   `FunctionError::Compute("... exceeded the compile time cap of <t>; the module is rejected")` and document in
   the function's comment that the thread cannot be cancelled and finishes in the background (structural
   caps are the real bound). Under `Load` there is no time cap but the semaphore still applies. Rewire
   `WasmFunction::from_binary_admitted_blocking` onto it (it must keep its signature and semantics,
   including `run_compile_blocking`'s `JoinError` mapping).
3. **Dry-run check** (`wasm/admission/check.rs`, exported): `check_module(engine: Arc<WasmEngine>, bytes: &[u8],
   policy: AdmissionPolicy, mode: TierMode) -> FnResult<ModuleReport>`: admits the bytes, then compiles
   exactly the way `compile_initial_slot` would decide for `mode` (Auto: fast then fall back to optimized;
   Never: fast only, a Winch rejection is an error, with the existing fallback when no fast engine exists;
   Now: optimized), builds the `InstancePre`, and discards everything, **without ever writing the
   incremental disk compile cache**. Winch never uses the cache; for every optimized-tier compile use a
   cache-less optimized engine held lazily in `WasmEngine` (`OnceLock`), built from the same base config
   and non-pooling, without `enable_incremental_compilation`. Runs under the compile gate. Persists nothing.
4. **Tier report**: `WasmFunction::tier_report() -> TierReport` and `WasmFunction::mode() -> TierMode`.
   `TierReport { tier, mode, calls_observed, upgrade_state }` (put the type next to `TierStats`; reuse the
   existing upgrade-state representation in `tier_upgrade.rs`; `calls_observed` saturates at the upgrade
   threshold because the counter stops after the upgrade: say so in its doc comment). The mode must be
   stored where the function state already keeps it (read `tier_upgrade.rs`/`wasm_function.rs`).
   Export `TierReport` and `check_module` from `lib.rs`.

## Tests (required, under `src/tests/wasm/admission/gates/`, manifest `mod.rs`)

* Semaphore: with `max_concurrent_compiles = 1`, two concurrent admitted compiles never overlap their
  blocking sections (use a test-only hook or two modules and a counter around the compile; if no clean hook
  exists, add a `cfg(test)` fault seam like the existing `TierFaults` and say so).
* Timeout: with a tiny `max_compile_time` and a test-only delay seam inside the blocking job, the call
  returns the time-cap error under `Admission`, the permit stays held until the job ends (a second compile
  waits), and `Load` ignores the cap.
* `check_module`: Auto accepts a normal module and reports its hash; Never rejects a module Winch rejects
  (use a seam or a module known to be rejected; if none exists in this wasmtime version, test the `Never`
  fallback with the fast engine disabled via `TierOptions { enabled: false, .. }`); Now compiles optimized;
  after a check the disk compile cache directory is untouched (use an engine with a temp cache dir if the
  options allow it, assert the directory stays empty) and nothing is registered anywhere; admission
  failures (cap, feature, contract) surface unchanged.
* `tier_report`: fresh function in `Auto` reports `Fast`, mode `Auto`, `calls_observed` 0; after crossing
  `hot_calls` and waiting for the upgrade (reuse the engine tests' helpers) it reports `Optimized`;
  `Never` stays `Fast`; `Now` is `Optimized` from the start.
* Defaults test updated for the two new `AdmissionLimits` fields.

## Done means

`cargo fmt -p shamir-wasm-host -- --check`, `cargo check -p shamir-wasm-host --all-targets` and
`cargo clippy -p shamir-wasm-host --all-targets -- -D warnings` pass (also with `--features
bench-strategies`), `wasm_function.rs` at most 985 lines, no folder over 7 entries, every changed file read
through for the whole class of error. Final message: what is done and not done per item, honestly, which
seams you added, and the files touched.
