# PROC-2 M1: the `shamir-lang` crate scaffold and the guest runtime foundation

Milestone 1 of `docs/dev-artifacts/design/shamir-lang-design.md` (read it fully first: sections 5 "Guest runtime", 6
"Compiler architecture", 7 "Bounds", 9 "Testing" and milestone 1 of section 10 are binding; where this brief and the design
differ in detail the design wins and you say so in your final message). The crate is `crates/wasm/shamir-lang`
(the workspace is grouped into `crates/<group>/<crate>`; the new crate belongs to group `wasm`).

Goal: a pure Rust crate that can EMIT, through `wasm-encoder`, the guest runtime every compiled module embeds: the
re-entrant bump allocator, the iterative MessagePack decoder and encoder over the 16-byte tagged cells, the error
bridge to `raise_error`, and the fixed memory layout; plus a test-support assembler that builds a minimal runnable
module around the runtime so later milestones (and the host-side execution tests) can run it. No lexer, parser, type
checker or source language yet (milestones 2 to 4).

## Already in place

The grouped layout, `shamir-wasm-admission` (admission of the emitted module: `admit_module`, `AdmissionPolicy`,
`AdmissionLimits`, ABI constants in `abi.rs`), `shamir-wasm-host` (engine that runs modules; host imports in
`src/wasm/wasm_function.rs` and `src/wasm/bindings/`), `shamir-sdk` (existing Rust guest SDK: the wire convention you must
mirror, see `runtime/` and `api/value.rs`).

## Deliverables

1. `crates/wasm/shamir-lang/Cargo.toml`: `publish = false`, edition and license as the other crates, `[lints] workspace = true`,
   `[lib] doctest = false`. Production dependencies exactly: `shamir-wasm-admission` (path), `wasm-encoder = "=0.254.2"`,
   `thiserror = "2.0"`. Dev-dependencies: `wat = "1.254.2"`, `wasmparser` pinned as in the admission manifest. No
   `wasmtime`, no `tokio`, no filesystem or process APIs.
2. `src/lib.rs` declaring the modules of the design tree that this milestone populates (`back`, plus a `cfg(test)`
   `testsupport`); `api`, `front`, `sem` appear later. Keep every `mod.rs` re-exports only.
3. `src/back/runtime/` exactly as in the design tree (`layout.rs`, `alloc.rs`, `cell.rs`, `codec/{decode,encode,frames}.rs`,
   `error.rs`, with their `tests/` folders registered by the owning parent): layout constants and address computation
   (guard, error scratch, static data, workspace, heap; checked arithmetic), the allocator helper emission
   (`shamir_alloc`, `memory.grow` receives the DELTA, publish the bump pointer only after successful growth,
   `shamir_alloc(0)` returns the heap base, 8-byte alignment), the 16-byte cell tag table and accessors, the iterative
   decoder and encoder emission with the explicit frame stack in the 64 KiB workspace, all caps from design section 5 and 7
   enforced BEFORE allocation, and the error helper (`raise_error(kind, ptr, len)` with the kind table, message
   truncation, terminal `unreachable`). Helpers are emitted as `wasm-encoder` functions with dependency-based
   emission (each helper once, stable topological order; a dependency cycle is an internal error).
4. `src/testsupport/`: `wasm.rs` assembles a minimal runnable module (memory, the runtime helpers, exports `memory`,
   `shamir_alloc`, and a test-only `shamir_call` that decodes the envelope and re-encodes a selected value so a
   MessagePack round trip can be driven), `msgpack.rs` (a tiny independent reference encoder for building test vectors),
   `vectors.rs` (the vectors listed in design section 9 for codec acceptance and rejection: every rejection class named in
   the "Codec contract" paragraph, boundary depths 31/32/33, counts and lengths at and over the caps), `fixtures.rs`.
5. Tests inside the crate that need no wasm engine: layout arithmetic, cell tag table, helper dependency order and cycle
   rejection, emitted bytes validate with `wasmparser` (the same feature set admission allows) and pass
   `shamir_wasm_admission::admit_module` under `AdmissionPolicy::Admission`, determinism (same input, same bytes).
6. Host-side execution tests (this is where the runtime is actually RUN): in `crates/wasm/shamir-wasm-host` add a
   dev-dependency on `shamir-lang` and a test module (registered by its owner parent, `src/tests/...`) that builds the
   testsupport module and executes it through the real engine with the vectors: decode and re-encode round trips,
   every rejection vector raising the right error kind, allocator re-entry (a second `shamir_alloc` while a call is
   active), memory growth at the heap boundary. Write them against the host crate existing test helpers (read how other
   `src/tests/runtime/` tests build and call a `WasmFunction`). They are NOT run locally.
7. Docs: one paragraph in `CLAUDE.md` workspace section for `shamir-lang` (crate count 36, `wasm` group row), and the
   crate listed where the design says. `Cargo.lock` changes only as needed: run `cargo check` once WITHOUT `--locked`.
   Add the crate to `.rust-guard/*` and `scripts/ci/layout_baseline.txt` only if a gate requires it (grep first).

## Constraints

Behaviour is judged by CI later: you cannot run tests, so every helper must be written so that it is easy to verify by
reading (small functions, named constants from `layout.rs`, no magic numbers, comments only where a byte offset or a
wasm control-flow depth is not obvious). Wasm control flow is the main risk: label depths for `br`/`br_if` inside nested
`block`/`loop`, and `memory.grow` handling; re-read every emitted function body at the end counting block depths.
Layout rules: at most 7 entries per folder, 1000 lines per file, one primary export per file, no local `use`, no inline
test modules, no new `#[allow]`, no visibility widening.

## Done when

`cargo check --all-targets` and `clippy -D warnings` pass for `shamir-lang` and `shamir-wasm-host` (the latter also with
its existing feature set used by tests), `fmt --check` passes, layout check passes after staging, and the final message
states honestly what is implemented, which design caps are enforced where, which vectors exist, and what could not be
verified without running tests.

## Binding policy (verbatim from CLAUDE.md)

Compilation, builds and tests run on CI, not on this machine. Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, `npm`/`tsc`/`vitest`,
WASM builds, e2e harnesses, any command whose purpose is to build the project or run tests/benches.
Allowed locally, scoped to the touched crates only: `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py` (reads tracked files only; new files are invisible to it until
staged: say so, do not work around it). Use `CARGO_TARGET_DIR=D:/system_artefact/target-lang` (outside the
repository) and never pipe check/clippy through head/tail. Crates you will touch: shamir-lang (new), shamir-wasm-host, plus every
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
