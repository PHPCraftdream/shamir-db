# Brief: two-tier WASM compilation, core (TIER-2)

Implement the core of `docs/dev-artifacts/design/wasm-two-tier-compilation.md` in this
worktree. Read that document first, then `crates/shamir-wasm-host/src/wasm/wasm_engine.rs`,
`crates/shamir-wasm-host/src/wasm/wasm_function.rs`, the call sites of `WasmEngine` /
`WasmFunction` in `crates/shamir-db/src/shamir_db/shamir_db/core.rs`,
`.../db/function_management.rs`, `.../db/validator_management.rs`, and `CLAUDE.md`.

## Compile / build / test policy (verbatim, mandatory)

**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`,
`npm`/`tsc`/`vitest`, WASM builds, e2e and integration harnesses, any release or full-workspace
build, and any other command whose purpose is to build the project or run its tests or
benchmarks. Allowed locally, scoped to the crates you touch (`-p <crate>`):
`cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` and `-- --check`, `python -I scripts/ci/check_layout.py`. Set
`CARGO_TARGET_DIR` to a directory OUTSIDE the repository for those commands. If you believe you
need a build or test run, say so in your final message; do not run it.

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
> command that mutates the working tree or index. Only edit files; the orchestrator commits.

No machine-local absolute paths in any file you write. Do not mention how or where CI is run.
Layout limits: at most 7 direct entries per folder and 1000 lines per source file. The folder
`crates/shamir-wasm-host/src/wasm` is already at 7 entries (`bindings/`, `compile_cache/`,
`fuel.rs`, `mod.rs`, `wasm_engine.rs`, `wasm_function.rs`, `wasm_sanitizer.rs`), so it cannot
gain a file or a folder. Do this: create the subfolder `wasm/engine/`, MOVE `wasm_engine.rs`
into it (keeping its content and tests working; its `mod.rs` only re-exports `WasmEngine`,
`WasmEngineOptions`, `WasmLimits` and the new tier items), put every new item in its own file
there (`tier_options.rs`, `tier.rs`, `tier_mode.rs`, `tier_stats.rs`, the upgrade scheduler,
the engine pair, ...), and update `wasm/mod.rs` and every `use` path that named
`wasm_engine`. `wasm/` then still has 7 entries. `wasm_function.rs` is at 779 lines: keep its
growth under 150 lines by putting the slot type and the upgrade logic in `wasm/engine/` files.
Report exactly what you moved.

## What to build

1. **Cargo.** `wasmtime` gets the `winch` feature in the normal (non-bench) dependency of
   `shamir-wasm-host`. The existing `bench-strategies` feature stays and must still compile
   (it keeps Pulley and the `SHAMIR_WASM_STRATEGY` switch for the OPTIMIZED engine).
2. **Engine.** `WasmEngine` owns a fast engine (Winch) and an optimized engine (Cranelift, the
   current construction including the incremental cache). Each engine gets its own
   allocator strategy with the existing pooling-with-fallback logic, the same fuel, epoch and
   `memory_init_cow` settings, and one epoch ticker thread that ticks both engines (hold both
   `EngineWeak`s; exit when both are gone). Provide accessors for each engine and
   `compile_fast` / `compile_optimized` returning the `Module`. `WasmEngineOptions` gains a
   `tier: TierOptions { enabled: bool, hot_calls: u64, background_jobs: usize }` with the
   defaults from the design (`enabled = true`); fix the existing struct literals in
   `crates/shamir-wasm-host/src/tests/wasm/wasm_engine_tests.rs` (use `..Default::default()`).
   If building the fast engine fails, log at `warn` and run single-tier on the optimized
   engine (everything below must work with `fast` absent).
3. **Function.** `WasmFunction` holds an `ArcSwap` slot with `{ engine: Engine, instance_pre:
   InstancePre<HostState>, tier: Tier }` plus the call counter and the upgrade state needed to
   schedule at most one upgrade. The call path loads the slot once, builds the `Store` from
   the slot's engine and instantiates from the slot's `InstancePre` (today
   `Store::new(engine.engine(), state)` and `self.instance_pre`); everything else in the call
   path (fuel leases, epoch deadline, limits, accounting) stays exactly as is.
   `build_linker` / `build_instance_pre` must take the engine of the tier being built.
4. **Creation modes.** Keep `from_wat`, `from_binary`, `from_binary_blocking` source
   compatible (they behave as `TierMode::Auto`) and add `*_with_mode` variants taking
   `TierMode::{Auto, Now, Never}`. Sanitizer and limit validation run once, before any compile.
   `Auto`: compile fast synchronously (fall back to optimized if the fast compile fails, log
   once at `info`), return. `Now`: compile optimized synchronously as the slot, no upgrade.
   `Never`: fast only, no upgrade.
5. **Upgrade.** When the counter crosses `hot_calls` exactly once, schedule one upgrade:
   `tokio::task::spawn_blocking` guarded by a semaphore of `background_jobs` permits (a
   process-wide or engine-wide semaphore owned by `WasmEngine`), compile with the optimized
   engine, `instantiate_pre`, swap the slot. The job holds only a `Weak` to the function's
   shared state. Failure keeps the fast slot, logs at `warn`; allow one retry when the counter
   next crosses 16x `hot_calls`. A call that already loaded the old slot is not disturbed.
   When no tokio runtime is available at the threshold (sync callers/tests), skip scheduling
   rather than panic.
6. **Observability.** `pub fn tier(&self) -> Tier` on `WasmFunction`; `info` log on every
   tier decision with tier, compile time and module size; a small `TierStats` of atomic
   counters (upgrades started/completed/failed) reachable from the engine for tests.
7. **Call sites.** `shamir-db` call sites keep compiling unchanged. Do not touch any DDL/OQL
   surface.

## Invariants (re-read before finishing)

Same results on both tiers; fuel and epoch trap identically; one upgrade per function; no
blocking work on the call path; every `.await` free of held locks; no new `#[allow]`; no
visibility widening of existing items (add new items instead); no logic in `mod.rs`; no local
`use`; imports at the top.

## Deliverable and verification

Edit files only. Finish with `cargo fmt -p shamir-wasm-host`, `cargo check -p shamir-wasm-host
-p shamir-db --all-targets`, `cargo clippy -p shamir-wasm-host -p shamir-db --all-targets -- -D
warnings`, the same clippy with `--features bench-strategies` for `shamir-wasm-host`, and
`python -I scripts/ci/check_layout.py`. Your final message lists every file you changed or
added, what you moved for layout, anything you could not verify, and any place where you had to
deviate from this brief with the reason. Write no tests in this stage (stage TIER-3 does).
