# DEC-3 Q5-b — wasm compilation off the async executor

## Context

`WasmFunction::from_binary` (crates/shamir-wasm-host/src/wasm/wasm_function.rs)
compiles a wasm module with Cranelift: tens to hundreds of milliseconds of pure
CPU work. It is called synchronously from async fns on the tokio runtime, which
blocks a worker thread (and, with the new incremental compile cache, also does
blocking file I/O). The owner-approved ruling (DEC-3, Q5-b) moves exactly these
calls onto `tokio::task::spawn_blocking`. Behaviour, error values and ordering
stay identical. This task is Q5-b only: NOT the Windows private-dir crate (Q5-c)
and NOT the benches (Q5-d).

## Scope

The async call sites (verify the list with `grep -rn "from_binary(" crates --include=*.rs`,
exclude tests/benches; a site you find that is also async and on the runtime
belongs to the scope, list it):

- `crates/shamir-db/src/shamir_db/shamir_db/core.rs` — the two boot loops
  (around lines 457 and 558) that register stored functions/validators.
- `crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs` (~183).
- `crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs` (~225).

Wrap each call in `tokio::task::spawn_blocking` moving owned inputs (`Arc`
engine clone, the wasm bytes as an owned `Vec<u8>`/`Bytes`, the limits), await
the join handle and map a `JoinError` (panic or cancellation of the blocking
task) to the same kind of error the call site already returns for a failed
compile, with a message naming the function and the join failure. Keep the
`match`/error mapping that follows the call unchanged. If a site holds a lock or
guard across the call, do not move the guard into the closure and do not extend
its lifetime across the new `.await` without saying so; report such a site
instead of guessing.

Put the shared wrapper in one place (an async fn on `WasmFunction`, e.g.
`from_binary_blocking`, in the file that owns `from_binary`, or a small private
helper next to the call sites if the wasm-host crate must stay runtime-free:
decide by checking whether `shamir-wasm-host` already depends on `tokio`; do not
add a new dependency edge just for this). A new test (CI-run) in the existing
`tests/` layout of the crate that owns the wrapper proves: the compiled function
behaves as the synchronous one (compile a small wat module through the wrapper
and call it), and a panic inside the blocking closure surfaces as the mapped
error, not as a panic of the caller. JSON/queries in tests go through the query
builder as everywhere else in the repo.

## Do not

- Do not change `WasmEngine::compile`, `from_binary` itself, `WasmLimits`, the
  cache modules, or any `Cargo.toml` (a tokio feature that is missing is
  reported, not added).
- Do not touch layout-sweep files: nothing under `crates/shamir-engine/src/repo/`
  or `crates/shamir-engine/src/tx/` (other stages run there in parallel).
- Layout limits: at most 7 entries per folder, at most 1000 lines per file; no
  `mod.rs` with logic; imports at the top of the file; no `#[allow]`.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator
  commits.
- Compile / build / test policy: see the policy text at the top of this run
  (verbatim). Allowed locally and REQUIRED until clean:
  `cargo check -p shamir-wasm-host --all-targets`,
  `cargo check -p shamir-db --all-targets`,
  `cargo check -p shamir-server --all-targets`,
  `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`,
  `cargo clippy -p shamir-db --all-targets -- -D warnings`,
  `cargo fmt -p shamir-wasm-host -p shamir-db -- --check` (run `cargo fmt -p <crate>`
  first), `python -I scripts/ci/check_layout.py`. One cargo process at a time,
  `CARGO_BUILD_JOBS=4`. No `cargo test`, no `cargo build`.
- NEVER stop processes by image name or pattern (`Stop-Process -Name`,
  `taskkill /IM`, `pkill`); the machine runs other sessions' cargo.
- Never use the shell builtin `xargs`. Capture exit codes without pipes. Logs
  under `.scratch/`. No machine-specific absolute paths in any file.
- Before finishing, one verifier child agent (the whole policy pasted verbatim)
  reads every changed file in full and checks: every call site in scope is
  wrapped, owned inputs only, error mapping unchanged, no guard crosses the new
  await, the test exists and is wired into the `tests/mod.rs` manifest. Start at
  most one child at a time.

## Final message

1. Every call site changed (file:line) and any async site you left alone, with
   the reason.
2. The real exit codes of every gate above.
3. How `JoinError` is mapped at each site.
4. The new test names and where they are wired.
5. Anything you could not verify (CI-only: the test run).
