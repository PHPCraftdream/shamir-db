# Brief: nested calls through a batch `Call` op must not reset depth, fuel and the call stack

## Why (existing defect found by design review)

A function may call another function through the guest import `call`
(`crates/wasm/shamir-wasm-host/src/wasm/bindings/call.rs`). That path enforces a depth
limit (`next_depth > depth_limit`, around line 139) and passes `with_depth(next_depth)`
and `with_depth_limit(...)` to the callee context.

A guest function that has a database gateway can also run `db_execute` with a batch that
contains a `Call` op. That path is:
`FacadeDbGateway::execute` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs`)
-> the batch executor -> `function_invoker.rs`
(`crates/engine/shamir-db/src/shamir_db/execute/core/function_invoker.rs`, the `CallOp`
handler) -> `invoke_function_in_db_as`
(`crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/management.rs`, around line
753), which builds a brand new `FnCtx::with_globals(...)`: depth 0, no shared fuel pool, no
record of the functions already on the call stack. So a function can call itself
indefinitely through batch `Call` ops, each hop restarting the depth counter, and a `Call`
inside a transactional batch re-enters the gateway while the outer transaction's locks are
held (possible deadlock).

## What

1. **Red test first.** Prove the defect with a test in `crates/engine/shamir-db` (look at
   `crates/engine/shamir-db/tests/functions/` and `src/shamir_db/tests/gateway/` for
   existing patterns). Rust-to-wasm guests may be skipped in CI when no wasm toolchain is
   available, so prefer a test that does not need one: a native function registered in the
   function registry whose body issues a gateway request with a `Call` op back to itself
   (or a two-function cycle), through the same `FacadeDbGateway` path a wasm guest uses.
   Before the fix the recursion must exceed the depth limit without an error (bound the
   test itself with a counter in the native function so it cannot run away: stop after,
   say, 3 times the depth limit and let the test assert it was not stopped by the engine).
2. **Fix at the narrowest layer that owns the bug.** A function's database gateway must
   know it is serving a function call and carry the current depth, depth limit, and the
   set/stack of active function identities (and the shared fuel pool if one exists in
   `FnCtx`; read `crates/wasm/shamir-wasm-host/src/runtime/context.rs`, `FnCtx`, to see
   what the `call` import propagates today and propagate the same things). A `Call` op
   executed through such a gateway must:
   * increment the depth and fail with the same error kind/text as the `call` import when
     the limit is exceeded;
   * fail when the target function is already on the active-call stack (recursion
     across stored functions is rejected, per the design; keep the stack bounded by the
     depth limit);
   * never run under a different identity than the existing rules give it (do not change
     authorization semantics in this task).
   Gateways created for top-level client requests (no enclosing function) keep depth 0 and
   an empty stack: behaviour of ordinary client batches with `Call` ops must not change.
   If carrying the state into the gateway is not possible without a wide refactor, the
   fallback is to refuse `Call` ops in batches that arrive through a function's gateway with
   a clear error; state which option you chose and why in the final message.
3. **Transaction re-entry.** Check whether a `Call` op executed from a function gateway
   inside a transactional batch can re-enter and deadlock (read the batch executor
   `crates/engine/shamir-engine/src/query/batch/` and the tests `call_tx_tests.rs`). If the
   refusal fallback is not chosen, make the transactional case explicit (refuse or document
   with a test). Report what you found.
4. Keep every existing test green (read them; do not weaken or delete any test). Add the new
   tests next to the existing ones in the project's test layout.

## Observable acceptance

1. The red test fails on the old code path logic (explain in the final message how you
   know) and passes with the fix; a second test shows depth is carried (an N-deep
   non-recursive chain below the limit still works, one above fails with the limit error).
2. `cargo fmt -p <crate> -- --check`, `cargo check -p <crate> --all-targets` and
   `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate
   (at least `shamir-db`, and `shamir-wasm-host` / `shamir-engine` if touched).
3. `python -I scripts/ci/check_layout.py` passes (at most 7 entries per folder, 1000 lines
   per file; split into subfolders rather than growing).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`,
   no new `#[allow]`, no visibility widening (an item that must become wider is reported in
   the final message instead of widened silently).

## Non-goals

No change of authorization/identity semantics, no new language feature, no `call_checked`
import, no version change.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and
`python -I scripts/ci/check_layout.py`, plus read-only git. Run cargo only under
`wnice -p idle -c 50 -t 4 -m 50 --` and with `-j 2`. No `cargo build/test/run/bench/doc`,
no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
