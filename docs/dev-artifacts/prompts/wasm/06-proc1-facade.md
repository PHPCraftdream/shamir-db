# PROC-1 stage B2: the function facade in `shamir-db` (hash, row fields, boot states, describe/export/check/alter)

Continues stages A and B1 (`04-proc1-admission.md`, `05-proc1-compile-gate.md`, already in this tree:
`admit_module`, `AdmissionPolicy`, `AdmissionLimits`, `ModuleReport`, `ModuleHash`, `check_module`,
`WasmFunction::{from_binary_admitted_blocking, tier_report, mode}`). Read the ADR
`docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md` (D2, D3, D5 and the D6 permission
bullets), `CLAUDE.md`, and the code you will touch first:
`crates/shamir-db/src/shamir_db/shamir_db/db/function/management.rs` (moved there by the orchestrator, was
`db/function_management.rs`), the function-loading block of `shamir_db/core.rs` (search `load_functions`),
`validator_management.rs`, `crates/shamir-wasm-host/src/builtin/meta.rs` (`CreateFunctionOptions`),
the function registry and the `ShamirFunction` trait. Wire types, builders, `shamir-server` and TypeScript are
out of scope (PROC-3b); this stage delivers the Rust facade on `ShamirDb` and the load pipeline.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-proc1c`
(outside the repository) and never pipe check/clippy through head/tail. If you believe you need a build
or test run, say so in the final message. Crates touched: `shamir-wasm-host`, `shamir-engine` (only if it
re-exports what you change), `shamir-db`; run check/clippy/fmt for each and for dependents whose public
signature you changed (grep the workspace for users of every signature you alter).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
mutates the working tree or index. Only edit files; the orchestrator commits.

## Repository rules that bind this work

* At most 7 direct entries per folder, at most 1000 lines per source file. `shamir_db/core.rs` is at 990
  lines: it must not grow; move the function-loading block out of it (see 3) so it shrinks.
  `db/function/management.rs` is about 830 lines: add new logic in new files, keep edits there minimal.
  `mod.rs` re-exports only; one file = one primary export; tests live in `tests/<topic>/` directories, never
  inline `#[cfg(test)] mod`; no local `use`; no visibility widening and no new `#[allow]`; comments terse.
  Count folder entries by hand for every folder you add files to (the layout script cannot see untracked
  files) and nest into subfolders instead of passing 7.
* The storage row is msgpack through `QueryValue::Map`; follow the existing field-insertion style.
* No machine-local absolute paths in any file. Do not mention how or where CI is run.
* Query-construction rule: no raw `serde_json` for ops (none are needed here).

## Deliverables

0. **Small host additions** (`shamir-wasm-host`): `WasmEngine::admission_limits() -> &AdmissionLimits` (the
   options value stored in the engine); `ShamirFunction` gets a default method `fn tier_report(&self) ->
   Option<TierReport> { None }` overridden by `WasmFunction` (read how the registry hands out functions and
   how `describe` can reach it without downcasting); `CreateFunctionOptions` gains `optimize: TierMode`
   (default `Auto`; fix every literal construction). Re-export what `shamir-db` needs.
1. **Create path.** In `create_function_with_opts_as` replace `WasmFunction::from_binary_blocking` by
   `from_binary_admitted_blocking(engine, bytes, WasmLimits::default(), opts.optimize,
   AdmissionPolicy::Admission(engine.admission_limits().clone()))`. For `FunctionSource::Source`, reject
   source longer than `max_source_bytes` before spawning the compiler. The row stops carrying the Fx
   `wasm_hash` and carries instead: `module_hash` (32 hex, `ModuleHash::to_hex()`), `abi` (Int, from the
   report), `origin` (claimed origin string, absent when none), `optimize` (`"auto"|"now"|"never"`). Keep
   `lang`, `source`, `version`, `kind` and the metadata injection exactly as they are. Apply the same
   admission policy and `module_hash` to the validator create path (`validator_management.rs`); validators
   get no new facade methods in this stage.
2. **Runtime function state.** A per-function state kept next to `function_meta` (same concurrency
   primitive and lifecycle: inserted at create/boot, moved by rename, removed by drop):
   `Loaded`, `Unloadable { reason }`, `Corrupt { expected, actual }`, with the stored hash fields. A function
   that is `Unloadable` or `Corrupt` is registered as an `UnavailableFunction` (new `ShamirFunction` impl in
   `shamir-db`, one file) whose invocation fails closed with the reason, so the name stays visible, can be
   described, replaced (`replace: true` must succeed on it) and dropped.
3. **Boot** (move the function-loading block of `core.rs` into `db/function/` as its own file; the call
   site in `core.rs` shrinks to one call). For each non-native row: decode base64; **verify the hash first**:
   if `module_hash` is present, compare `ModuleHash::of(bytes)`; else if the legacy 16-hex `wasm_hash` is
   present, verify it with the legacy algorithm (rustc-hash `FxHasher` over `bytes` as `Vec<u8>`'s `Hash`
   impl, i.e. including the length prefix, exactly as the old create path computed it: keep that helper in
   one function with a doc comment); a mismatch means `Corrupt` (do not compile the bytes). On a legacy
   match compute the composite hash in memory only (no write at boot). A row with neither field is
   "unknown", not corrupt. Then compile with `from_binary_admitted_blocking(.., stored optimize mode,
   AdmissionPolicy::Load)`; a failure means `Unloadable { reason }` (the old behaviour logged and skipped
   the function; the new behaviour registers it as unavailable and keeps the warning log). Boot honours the
   stored `optimize` (absent means `auto`). Keep the rest of the loop (native skip, collision error,
   metadata publication) behaviourally identical.
4. **Facade methods** (new files under `db/function/`, one primary export each; `*_as` variants take an
   `Actor`, thin `Actor::System` wrappers like the existing ones):
   * `describe_function_as(name, actor) -> DbResult<FunctionDescription>`: requires `Read` on
     `ResourcePath::Function` (check that `Action::Read` is meaningful for that resource in the access
     model; if the model needs an addition, make the smallest one and say so). `FunctionDescription`
     (own file): name, lang (server-derived: `"wasm"`/`"rust"`), claimed origin, ABI version, composite hash
     (hex, or `None` for unknown legacy), byte size, `optimize` mode, `Option<TierReport>`, state.
   * `export_function_as(name, actor) -> DbResult<FunctionExport>`: requires `Write` on the function.
     `FunctionExport` (own file): the exact stored bytes, the source text for `lang = rust`, the hash. Verify
     the stored hash on export; a mismatch marks the function `Corrupt` and returns an error.
   * `check_function_as(actor, source: FunctionSource<'_>, optimize: TierMode) -> DbResult<ModuleReport>`:
     requires `Create` on `FunctionNamespace`; for `FunctionSource::Source` it additionally requires
     `Execute` on `WasmCompiler` exactly as create does (same code path or shared helper, not a copy that
     can drift), applies the source cap, compiles, then runs `check_module` under
     `AdmissionPolicy::Admission`. Persists and registers nothing.
   * `alter_function_optimize_as(name, mode, actor)`: requires `Write` on the function; reads the stored
     row, admits and rebuilds the function from the stored bytes with the new mode, replaces the registry
     entry, rewrites the row with only `optimize` changed (preserving `ResourceMeta`, using the same
     save path as the replace branch of create), updates the runtime state. Refuses `Corrupt`/`Unloadable`.
5. **Docs**: add an "Implementation notes" section to the ADR file recording the deviations of this PROC-1
   (contract check at admission only; wire ops and builders deferred to PROC-3b; the server option for
   `AdmissionLimits` and the frame check call deferred to PROC-3b because `shamir-db` cannot see the frame
   constant; `ModuleReport` is what `check_function` returns).

## Tests (required; follow the existing layout under `crates/shamir-db/src/shamir_db/tests/`, nest new
topic folders so no folder passes 7 entries)

Create then export: bytes identical, composite hash equal to `ModuleHash::of`, describe shows ABI (use a
module carrying a `shamir.abi` and a `shamir.origin` custom section), origin claim, size, mode, tier report;
create from the exported bytes yields the same hash (round trip, for a `wasm` upload and, if the Rust
toolchain is present and the existing tests skip without it in the same way, for a `rust` source).
Legacy rows written directly to the system store: matching legacy `wasm_hash` loads and reports the composite
hash; a tampered legacy row is `Corrupt`; a row with no hash is `Loaded` with unknown hash. `module_hash`
tampering (wasm_b64 altered after create, then restart) gives `Corrupt`, the invocation error carries the
reason, describe reports it, `replace: true` repairs it, drop removes it. A stored module that fails the
`Load` policy (for example SIMD written directly to the store) is `Unloadable`, survives restart as visible,
and is replaceable. `check_function`: success leaves no row and no registry entry; admission failures
surface; `Source` without `Execute` on `WasmCompiler` is denied with a non-system actor (reuse the existing
permission-test pattern); a plain `Create` holder may check a `wasm` module. `alter`: mode changes and
describe reflects it; the new mode survives restart (boot honours stored `optimize`); a non-writer is denied;
a corrupt function is refused. `describe` needs `Read`, `export` needs `Write` (denied otherwise).
Existing function tests keep passing: fix the ones that asserted the old skip-on-compile-failure boot
behaviour or the `wasm_hash` field, and say which.

In `shamir-wasm-host` add one test file (under `src/tests/wasm/admission/gates/` or a nested folder if that
folder is full) with a corpus of small Rust sources compiled through the existing
`compile_rust_source_blocking` (trait object, function pointer table, `Vec`/`String` use, a panic path)
and each admitted under `AdmissionPolicy::Admission`; it must skip (not fail) exactly the way the existing
compile tests do when the toolchain or target is unavailable.

## Done means

`cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` pass for
`shamir-wasm-host` (also `--features bench-strategies`) and `shamir-db`, plus the dependents of any public
signature you changed; `core.rs` smaller than before; no folder over 7 entries; every changed file read
through for the whole class of error. Final message: what is done and not done per item, honestly, which
existing tests you changed and why, and the files touched.
