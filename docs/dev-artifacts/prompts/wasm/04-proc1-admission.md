# PROC-1 stage A: the admission module in `shamir-wasm-host`

Implements, in `crates/shamir-wasm-host`, the load-time part of
`docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md` (read it fully first, sections
D1, D2, D3; also `wasm-two-tier-compilation.md` and `CLAUDE.md`). Out of scope for this stage:
anything in `shamir-db`/`shamir-server`/wire types, the compile semaphore and time cap, `check_function`,
per-function tier accessors (stage B does those).

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

* At most 7 direct entries per folder, at most 1000 lines per source file; `mod.rs` re-exports only;
  one file = one primary export; tests live in `tests/<topic>/` directories, never inline
  `#[cfg(test)] mod`; no local `use` (all imports at file top); no visibility widening and no new
  `#[allow]`; JSON literals in tests multi-line; comments terse; match the surrounding style.
* No machine-local absolute paths in any file. Do not mention how or where CI is run.
* Dependencies: `Cargo.lock` is allowed to change only by the new dependency below and by enabling
  features on `wasmparser`; run `cargo check` once without `--locked` for that and say so.

## Already done by the orchestrator

`wasm_sanitizer.rs` was moved to `src/wasm/admission/sanitizer.rs` and its test file to
`src/tests/wasm/admission/sanitizer_tests.rs` (with `admission/mod.rs` in both trees). New code goes
into these two folders; keep each folder at 7 entries or fewer (split into subfolders if needed).

## Deliverables

All under `src/wasm/admission/` (new files; `mod.rs` re-exports only) and wired through `wasm/mod.rs`
and `lib.rs` as public API where the ADR requires it:

1. **`AdmissionLimits`** (one file): the caps of ADR D2 step 1 and 2 with these defaults:
   `max_module_bytes` 8 MiB, `max_functions` 20_000, `max_types` 5_000, `max_imports` 64,
   `max_exports` 64, `max_globals` 1_000, `max_locals_per_function` 10_000,
   `max_function_body_bytes` 1 MiB, `max_operators_per_function` 500_000, `max_block_depth` 256,
   `max_br_table_targets` 4_096, `max_params_and_results` 128, `max_table_elements` 100_000,
   `max_element_segments` 1_000, `max_element_items_total` 200_000, `max_data_segments` 10_000,
   `max_data_bytes_total` 4 MiB, `max_memory_min_pages` 1_024. Plus `max_source_bytes` (1 MiB; stored
   here for stage B) and a method `validate_against_frame(frame_cap: usize) -> Result<(), String>`
   enforcing `4/3 * max_module_bytes + max_source_bytes + 64 KiB < frame_cap`. `Default` = the above.
   Add it to `WasmEngineOptions` as a field `admission` (keep every existing constructor source
   compatible; fix the literal constructions in tests if any).
2. **`AdmissionPolicy`** enum: `Admission(AdmissionLimits)` (all gates) and `Load` (deterministic
   gates only: parse, feature set, import allowlist, ABI section; no operator caps, no contract check,
   no time or size caps).
3. **Feature set** (`features` file): a `wasmparser::Validator` built with an exact `WasmFeatures`:
   MVP, sign-extension, saturating float-to-int, bulk memory, multi-value, mutable globals, reference
   types, floats. Everything else disabled (threads, SIMD, GC, exceptions, tail calls, memory64,
   multi-memory, component model, relaxed SIMD, extended const, function references, wide arithmetic,
   custom page sizes, and so on: enumerate the flags of the pinned `wasmparser` and set each
   explicitly). Operator scan for the partial reference-types use: at most one table, funcref only; no
   `externref` anywhere; in function bodies reject `ref.null`, `ref.is_null`, `ref.func`, `table.get`,
   `table.set`, `table.grow`, `table.size`, `table.fill`, `table.copy`, `table.init`, `elem.drop`
   (`call_indirect` stays allowed; `ref.func` stays allowed in constant expressions). Enable the
   `wasmparser` features the Validator needs in `Cargo.toml` (keep the exact version pin).
4. **Structural caps** in the same single `wasmparser` pass (cheapest first; use the section and
   operator readers, never compile): every cap of deliverable 1 yields a distinct error variant naming
   the cap, the limit and the observed value.
5. **Sections** (`sections` file): `shamir.abi` (exactly 4 bytes, u32 LE; duplicate, other length or a
   version other than 1 is an error naming both versions; absent = v1) and `shamir.origin` (UTF-8, at
   most 64 bytes, at most one section; invalid is an error). Other custom sections are ignored.
6. **Contract check** (admission only): exports `memory` (a memory), `shamir_alloc` of type
   `(i32) -> i32`, `shamir_call` of type `(i32, i32) -> i64`; the error names the missing or mistyped
   export.
7. **`ModuleReport`** returned by `admit_module(bytes, &AdmissionPolicy) -> Result<ModuleReport,
   AdmissionError>`: byte size, the counts observed in step 4, `abi_version: u32`,
   `origin_claim: Option<String>`, `hash: ModuleHash`. `AdmissionError` is a `thiserror` enum with
   one variant per rejection class; a `From<AdmissionError> for FunctionError` (variant
   `FunctionError::Compute(message)`) keeps existing call sites working. Keep `verify_wasm_module`
   exported with its current signature and behaviour for the import allowlist (it may call into the
   new code); `SANCTIONED_HOST_IMPORTS` and its sync test stay as they are.
8. **`ModuleHash`** (`module_hash` file): `[u64; 2]` = XXH3-128 of the full bytes, seed 0, the high
   word first. `to_hex()` gives 32 lowercase hex characters (`{hi:016x}{lo:016x}`), `from_hex()`
   parses it strictly. Add the dependency `xxhash-rust` with the `xxh3` feature (it is already in
   `Cargo.lock`; keep the resolved version and make the feature set minimal). The ADR rule goes into
   the type's doc comment: identity and corruption check only; nothing may use it for lookup, dedup,
   caching, permissions or `definer` decisions.
9. **Wiring**: every existing `WasmFunction` constructor (`from_wat*`, `from_binary*`,
   `from_binary_blocking*`) runs `admit_module(bytes, &AdmissionPolicy::Load)` where it runs
   `verify_wasm_module` today. Add `WasmFunction::from_binary_admitted_blocking(engine, bytes,
   limits, mode, policy)` returning `(WasmFunction, ModuleReport)`, using the policy given and the
   existing blocking-pool compile helper; the other constructors keep their signatures.

## Tests (required, in `src/tests/wasm/admission/`, split by topic, manifest `mod.rs`)

One test per cap (a minimal WAT or hand-assembled module exceeding exactly that cap and an
at-the-limit module that passes); one per rejected feature (SIMD, threads/atomics, GC types,
exceptions, tail calls, memory64, multi-memory, externref, table ops in bodies, second table); one per
allowed feature (sign-extension, saturating conversions, bulk memory, multi-value, mutable globals,
funcref `call_indirect`): the module is admitted and also builds and runs through `WasmFunction`
with `TierMode::Auto`; for each allowed feature assert it also compiles on the optimized engine
(`TierMode::Now`) and report in the final message which of them the fast engine rejects (the `Auto`
fallback covers it). Section tests (absent, valid, duplicate, wrong length, unknown version, bad UTF-8
origin, oversize origin). Contract tests (missing export, wrong type, wrong export kind). `Load` vs
`Admission` behaviour: a module over an operator cap passes `Load` and fails `Admission`; a module
without the contract exports passes `Load`. Hash tests: layout equals `format!("{hi:016x}{lo:016x}")`
of the crate's `xxh3_128`, hex round trip, strict parse rejects uppercase and wrong length, a one-bit
flip changes the hash, empty input is stable. `AdmissionLimits::validate_against_frame`: accepts the
defaults against 16 MiB, rejects `max_module_bytes` above the frame budget. A test that the wasmtime
`Config` of both engines accepts every feature in the allowed set (the "superset" check of ADR D2 step
3): the positive module tests above are that check; do not duplicate it.

## Done means

`cargo fmt -p shamir-wasm-host -- --check`, `cargo check -p shamir-wasm-host --all-targets` and
`cargo clippy -p shamir-wasm-host --all-targets -- -D warnings` pass (also with `--features
bench-strategies`), no file over 1000 lines, no folder over 7 entries, every changed file read through
for the whole class of error (imports, privacy, cfg gating, unused items, module paths). Final
message: what is done and not done, per item above, honestly; the list of features the fast engine
rejects; the files touched.
