# No-C policy — native-code allowlist CI gate and the official "what C remains" statement

## Context

Owner ruling (adjudicated by @oh): the workspace goes Rust-only where that comes easily; where no
real Rust alternative exists, the C/C++ dependency stays and is documented. The easy removal
(the bench-only `mimalloc` option) is merged. What remains is to make the state enforceable and
documented:

- a CI gate that fails when a NEW crate that compiles C/C++/assembly (or links a C library)
  enters the built dependency graph, or when one of the already-removed/never-built native crates
  (`ring`, `libmimalloc-sys`, `zstd-sys`, `wasmtime-internal-jit-debug`, `openssl-sys`) enters it;
- `docs/native-code.md`, the official statement of which C remains and why.

Read first: `scripts/ci/gates/` (existing gate scripts such as `features_matrix_check.py`,
`npm_licenses_check.py`, `merge_gate_check.py` and their unit tests under `scripts/ci/tests/`),
`.github/workflows/ci.yml` (the required jobs, notably the `deny` job and how `merge-gate` lists
`needs:`), `scripts/ci/gates/merge_gate_check.py` (`REQUIRED_JOBS`), `deny.toml` header comments.

## Scope (exact)

1. New `scripts/ci/gates/native_code_check.py` (stdlib only, runnable as `python3 -I`).
   For each build tool T in `cc`, `cmake`, `bindgen`, `pkg-config` run
   `cargo tree --locked --workspace --all-features --target all -e normal,build,dev -i T --depth 1 --prefix none --format "{p}"`
   (package id lines; strip the version/source part to a bare crate name), collect the set of
   DIRECT dependents of T, and compare it with the allowlist below. Fail with a readable message
   on (a) any dependent not in the allowlist (a NEW native-code crate), and (b) any allowlist
   entry that is no longer a dependent (stale entry: the allowlist and the document must stay
   truthful). A tool that is not in the graph at all (`cargo tree` reports it is absent) means an
   empty dependent set, not a script error; any other cargo failure is a script error (exit 2).
   Allowlist (verify with the first run; the rulings analysis expects exactly this):
   - `cc`: `aws-lc-sys`, `cmake`, `blake3`, `generator`, `iana-time-zone-haiku`, `wasmtime`,
     `wasmtime-internal-fiber`
   - `cmake`: `aws-lc-sys`
   - `pkg-config`: `aws-lc-sys`
   - `bindgen`: `libproc`
   Hard deny list, asserted with `cargo tree -i <crate>` over the same flags (crate present in the
   built graph = fail): `ring`, `libmimalloc-sys`, `zstd-sys`, `wasmtime-internal-jit-debug`,
   `openssl-sys`. If a deny-list crate is only present because of lock structure but not in the
   `cargo tree` graph it passes (that is the point: the graph is what is built).
   Exit codes: 0 clean, 1 policy violation, 2 usage/tooling error. Keep the cargo invocation behind
   one function so tests can inject canned output.
2. Unit tests under `scripts/ci/tests/gates/` (follow the neighbours' layout and runner
   conventions; wire into whatever test inventory exists, e.g. the gate inventory test): canned
   `cargo tree` output for an allowed state (exit 0), a new dependent of `cc` (exit 1, names the
   crate), a stale allowlist entry (exit 1), a deny-list crate present (exit 1), cargo failing
   (exit 2). A fake output containing `ring` must produce exit 1.
3. Wire the gate into CI as a step of a job that `merge-gate` already requires (the `deny` job is
   the natural place; it already runs gate scripts such as `npm_licenses_check.py`; do not create a
   new required job unless the existing job cannot run cargo). The step must run on pull requests
   and on master pushes exactly like its neighbours, with the Rust toolchain the job already sets
   up. Update `REQUIRED_JOBS` / inventory tests only if you add a job (prefer not to).
4. New `docs/native-code.md` containing, verbatim in substance, the table of what C remains and why
   (KEEP-AS-IS list) and the allowlist, one line per entry:
   - `aws-lc-sys` (AWS-LC, C + assembly; prebuilt NASM objects on Windows x86_64): rustls TLS 1.3
     and certificate crypto. No audited production-ready pure-Rust provider exists
     (`rustls-rustcrypto` is alpha and marked not for production, `rustls-graviola` is new and
     unaudited, `ring` is C/asm too). Re-evaluate when graviola publishes an audit or reaches 1.0.
   - `wasmtime` `helpers.c` (Unix): part of wasmtime's runtime for native-code hosts; no Rust-only
     wasmtime build; WASM execution is a core product feature.
   - `wasmtime-internal-fiber` `windows.c` (Windows): stack switching behind wasmtime's async
     support, which our async host imports require.
   - `libproc` via `bindgen` (macOS build only): process metrics; binds to the OS libproc, no C is
     compiled; the only replacement is hand-written unsafe FFI.
   - `generator` (loom feature only): assembly context switching for the loom model checker; built
     only in the CI loom job, never in a product build.
   - `napi-sys` (shamir-client-node only): Node-API is a C interface by definition; nothing C is
     compiled; not part of the server binary.
   - Windows import libraries (`windows-targets`, `winapi-*-gnu`): prebuilt stubs for OS DLLs.
   - Listed in `Cargo.lock` but never built: `ring`, `wasmtime-internal-jit-debug`,
     `iana-time-zone-haiku`, and the `cc` build-dependency of `blake3` (feature `pure`): the gate
     keeps it that way.
   Add the short history (blake3 pure #226, zstd/cache rewrite #227/#246-#250, mimalloc bench option
   removed #255) and the rule for adding a native dependency (needs an owner decision and an
   allowlist + document entry in the same PR).
5. Point to `docs/native-code.md` from the `deny.toml` header comment and the script docstring.
   Do not change any other policy in `deny.toml`.

## Do not

- Do not change dependencies, `Cargo.toml`/`Cargo.lock`, features, other gates' behaviour or the
  version of anything. Do not run the gate against the network or write outside the repo.
- Layout limits: at most 7 entries per folder, at most 1000 lines per file; imports at the top of
  files; no machine-specific paths.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command
  that mutates the working tree or index (including `git add` / `git commit`). Only edit files;
  the orchestrator commits.
- Compile / build / test policy: see the policy text at the top of this run (verbatim). Allowed
  locally for this task: `cargo tree --locked ...` and `cargo metadata --locked` (read-only graph
  inspection, no compilation), `python -I -m py_compile`, `python -I scripts/ci/check_layout.py`,
  running the new gate script itself against the real workspace (it only calls `cargo tree`) so
  the allowlist can be confirmed, and running the python unit tests only if the repo documents a
  way to run them without a test runner (otherwise CI-only; say so). No `cargo build/test/bench`.
- NEVER stop processes by image name or pattern. Never use the shell builtin `xargs`. Capture exit
  codes without pipes. Logs under `.scratch/`. No network access beyond what cargo already needs.
  Never send the user's e-mail or any personal data anywhere.
- Before finishing, start ONE read-only verifier child (the whole policy pasted verbatim) that
  reads every changed file and checks scope items 1-5 against the diff. One child at a time.

## Final message

1. Per scope item 1-5: done / not done with file:line.
2. The REAL output of the gate script against the workspace (the actual dependent sets per tool and
   the deny-list result); if it differs from the expected allowlist, say exactly how and keep the
   allowlist truthful to the real graph with a justification in the document.
3. Where the step is wired and why that job; exit codes of the allowed gates; what is CI-only.
