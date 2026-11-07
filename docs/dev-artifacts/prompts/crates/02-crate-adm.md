# CRATE-ADM: the `shamir-wasm-admission` crate (module admission without a wasm runtime)

Goal: the admission pipeline (`admit_module`, `AdmissionPolicy/Limits/Error`, `ModuleReport`, `ModuleHash`, the
import allowlist, the ABI constants) becomes its own crate with NO `wasmtime`/`tokio` dependency, so that (1) its
tests run in seconds and only when it changes, (2) the future in-process language compiler can depend on it to
validate its own output and to emit the ABI sections. Behaviour does not change.

## Already done by the orchestrator (you cannot move files; do not recreate them)

`git mv` into `crates/shamir-wasm-admission/src/`: `admit.rs`, `module_hash.rs`, `sanitizer.rs`, `scan/`,
`types/` (from `crates/shamir-wasm-host/src/wasm/admission/`), and the pure tests: `tests/caps/` (all),
`tests/features/rejected_tests.rs`, `tests/gates/{contract,policy,sections}_tests.rs`,
`tests/identity/hash_tests.rs`, plus a COPY of the old host fixture file as `tests/support.rs` (the host keeps its
original). The new crate has no `Cargo.toml`, no `lib.rs`, no `tests/mod.rs` and no `tests/*/mod.rs` yet.
Left in the host (`crates/shamir-wasm-host/src/wasm/admission/`): `gate/` (`check_module`, compile gate) and
`mod.rs`; host tests left: `tests/wasm/admission/{features/allowed_tests.rs, gates/{check,compile_gate,tier_report}_tests.rs,
identity/limits_tests.rs, sanitizer_tests.rs, support.rs}` and the `mod.rs` manifests there, which still list the
moved modules and must be fixed.

## Deliverables

1. New crate `crates/shamir-wasm-admission` (`publish = false`, same edition/license/`[lints] workspace = true`,
   `[lib] doctest = false`): `Cargo.toml` with only what the moved code needs (`wasmparser` pinned exactly as in the
   host manifest with the same features, `xxhash-rust` with `xxh3`, `thiserror`; dev-dependencies: `wat`
   (same pin as the host)); `src/lib.rs` (`mod` declarations and `pub use` of the public surface: `admit_module`,
   `AdmissionError`, `AdmissionLimits`, `AdmissionPolicy`, `ModuleReport`, `ModuleHash`, `verify_wasm_module`,
   `SANCTIONED_HOST_IMPORTS`, plus ABI constants if you introduce them, see 3) and `#[cfg(test)] mod tests;` with the
   manifests `tests/mod.rs`, `tests/{caps,features,gates,identity}/mod.rs` listing exactly the moved files.
2. Decouple from the host: the new crate must not reference `crate::runtime::error::FunctionError` or any host
   type. `types/error.rs` loses its `From<AdmissionError> for FunctionError` (it moves to the host, see 4);
   `sanitizer.rs::verify_wasm_module` must return `Result<(), AdmissionError>` built from the existing variants,
   preserving the exact message texts that existing tests assert (the three variants that keep the old
   "wasm sanitizer: ..." wording). Fix the moved tests' imports (`crate::{admit_module, ...}` now means the new
   crate) and give `tests/support.rs` only the pure helpers (`assemble`, the WAT builders, `admit`/`load` wrappers,
   custom-section append); everything that builds a `WasmFunction`/`WasmEngine` stays in the host support file.
3. ABI constants in one file of the new crate (`abi.rs`, one primary export): the three contract export names
   (`memory`, `shamir_alloc`, `shamir_call`), the section names `shamir.abi` / `shamir.origin`, the supported ABI
   version (1) and the origin length cap; use them from `scan/` instead of the string literals that are there now
   (behaviour identical), and re-export them from `lib.rs`.
4. Host (`crates/shamir-wasm-host`): add the dependency on the new crate; `src/wasm/admission/mod.rs` keeps `gate`
   and re-exports the public admission surface from the new crate so that every existing path
   (`shamir_wasm_host::admit_module`, `crate::wasm::admission::...`, `shamir_engine::function::...` in `shamir-db`)
   keeps compiling unchanged; the host owns `impl From<AdmissionError> for FunctionError` (new file next to the
   gate, same mapping as before) and a thin `verify_wasm_module(bytes) -> FnResult<()>` wrapper if the public host
   signature must stay `FnResult<()>` (keep the public host API identical); drop the `wasmparser` and
   `xxhash-rust` dependencies from the host manifest if nothing else in the host uses them (grep), otherwise keep.
   Fix the host test manifests; the sync test `sanctioned_list_matches_linker_registrations` stays in the host and now
   reads `SANCTIONED_HOST_IMPORTS` from the new crate.
5. `limits_tests.rs` (identity): split it, the defaults/frame tests of `AdmissionLimits` move to the new crate, the
   `WasmEngineOptions` default assertion stays in the host.
6. Docs and metadata: `CLAUDE.md` (crate count 32 after both new crates, list entry for `shamir-wasm-admission`,
   one sentence), the ADR implementation notes (one line: admission lives in its own crate), `scripts/ci/*`,
   `.config/nextest.toml`, `.rust-guard/*.toml`, `scripts/ci/layout_baseline.txt` references to the moved paths
   (grep; lower or remove baseline entries, never add).

## Success criteria

The new crate's dependency list has no `wasmtime`, `tokio`, `shamir-*`. `cargo check`, `clippy -D warnings` and
`fmt` pass for `shamir-wasm-admission`, `shamir-wasm-host` (also with `--features bench-strategies`) and
`shamir-db` (a dependent through re-exports; its check is slow, run it once at the end). No folder over 7
entries, no file over 1000 lines. Final message: what is done and not done per deliverable, honestly, and the files
touched.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-adm` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: shamir-wasm-admission (new), shamir-wasm-host, shamir-db (dependent check only), plus every
dependent whose public signature or path you changed (grep the workspace). `Cargo.lock` may change only by what
this task needs: run `cargo check` once WITHOUT --locked so cargo writes it, and say so. If you believe you need a
build or test run, say so in the final message. No other shell commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
mutates the working tree or index. Only edit files; the orchestrator commits (and has already done every file
move: you cannot move or delete files, so work with the layout you find).

## Repository rules that bind this work

At most 7 direct entries per folder, at most 1000 lines per source file; `mod.rs` re-exports only; one file = one
primary export; tests live in `tests/<topic>/`, never inline `#[cfg(test)] mod`; no local `use`; no visibility
widening and no new `#[allow]`; no `impl` blocks outside the owning module (rust-guard); JSON literals in tests
multi-line; comments terse; no machine-local paths; do not mention how or where CI is run. Count folder entries by
hand for every folder you touch (the layout script cannot see untracked files) and nest instead of passing 7.
Children, if you use any, only edit files, never run commands, and get the NO-BUILD policy above quoted verbatim.
