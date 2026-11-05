# Brief: two-tier WASM compilation, documentation (TIER-4)

Document the committed core (`feat(wasm-host): two-tier compilation core`). Read
`docs/dev-artifacts/design/wasm-two-tier-compilation.md`, the code under
`crates/shamir-wasm-host/src/wasm/engine/` and `crates/shamir-wasm-host/src/wasm/wasm_function.rs`,
`docs/guide-docs/guide/05-functions.md`, `docs/guide-docs/KNOWN_LIMITATIONS.md` and `CLAUDE.md`.

## Policy (verbatim, mandatory)

**Compilation, builds and tests run on GitHub CI, not on this machine.** You only edit
documentation; run no build, test, bench or npm command at all.

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
> command that mutates the working tree or index. Only edit files; the orchestrator commits.

No machine-local absolute paths in any file you write. Do not mention how or where CI is run.
Examples that build database queries go through the query builders, never raw JSON.

## What to write

1. `docs/guide-docs/guide/05-functions.md` (Russian, same tone and structure as the file): a new
   section after the compilation/deploy part, "Уровни компиляции": what the fast and the
   optimized tier are, what happens at creation and after the call threshold, the three modes
   (Auto, Now, Never) and when each fits, the defaults (threshold 64 calls, background pool
   `max(1, cores / 4)`), what a failed upgrade means, how to read the current tier from Rust
   (`WasmFunction::tier()`), and an honest limits paragraph: not yet selectable from DDL/OQL (a
   later task), Pulley is not a fast tier, the fast tier runs compute roughly 1.8-6x slower than
   native and the optimized tier 1.3-4x (measured on a hosted runner: read them as ratios, not
   absolutes), a database call from a guest costs about 6 us of gateway overhead regardless of
   tier, so a database-bound procedure gains little from the upgrade.
2. `docs/guide-docs/KNOWN_LIMITATIONS.md`: a short entry for the same limits (mode and threshold
   are engine options, not DDL; no persisted tier state, every start recompiles on the fast tier;
   two allocator pools reserve address space twice).
3. `docs/dev-artifacts/design/wasm-two-tier-compilation.md`: change `Status` to implemented,
   replace estimates with what the code does (read it), and add a short "Implementation notes"
   section listing the files and the deviations from the design (`WasmEngine::compile` is now
   test-only, the pooling ceiling follows the optimized engine, the fast engine is not built when
   tiering is disabled, `Never` falls back to the optimized tier when there is no fast engine).

Final message: the files changed and any statement in the docs you could not confirm from code.
