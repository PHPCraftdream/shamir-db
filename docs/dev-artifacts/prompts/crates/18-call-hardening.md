# Brief: one hardened resolution path for nested function calls (PROC-2c stage 2a)

## Why

A function can call another stored function through the guest import `call`
(`crates/wasm/shamir-wasm-host/src/wasm/bindings/call.rs`, `host_call`). Today that path
looks the callee up in the registry and invokes it directly: there is no `Execute`
authorization of the target, the callee inherits the parent's actor, database gateway,
repository, network gateway and secret grants wholesale (a `Definer`/setuid target
therefore runs as the PARENT's actor), nothing remembers which functions are already on
the call stack except a depth counter, and the target is looked up by mutable name between
authorization and invocation (a gap a replacement can hit).

The reviewed design `docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md` (read
sections 2.3 and 2.4, the stage 2 row of section 6 and the stage 2 bullets of section 7, then
`docs/dev-artifacts/design/shamir-lang-design.md` where it speaks of calls) fixes the rules.
This brief implements ONLY the host-side common resolution of the existing `call` import (no
language change, no new import, no `call_checked`; that is the next task).

## What (decisions are made; do not re-open them)

One resolution path for the guest `call` import that does, in this order and failing closed on
any missing piece (target not in the catalogue, missing metadata or owner, missing capability):

1. **Capture.** Resolve the target ONCE to a captured binding (the exact artifact that will
   be invoked, plus its security metadata and owner). Authorization and invocation use that
   same captured value, never a second lookup by name
   (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/management.rs`
   `invoke_function_in_db_as` authorizes and then invokes by name today; keep that
   top-level path working, but the nested path must use the capture).
2. **Authorize.** `Action::Execute` on the target as the CURRENT effective actor of the
   calling hop (`authorize_invocation` in
   `crates/engine/shamir-db/src/shamir_db/shamir_db/access/authorize.rs`; "current actor"
   means the parent invocation's effective actor, not an original outer principal).
3. **Effective actor of the target** (addendum 2.3 table): `Security::Definer` -> the target's
   owner; `Invoker` without the legacy setuid bit -> the current actor; `Invoker` with the
   legacy setuid bit -> the owner (existing behaviour of the top-level path; reuse its
   decision function, do not copy the logic).
4. **Guard for Definer parents.** The frame keeps the parent's owner when the parent is a
   `Definer`. Before entering an `Invoker` target whose owner differs from that parent
   Definer owner, refuse the call (new fail-closed v1 guard).
5. **Rebuild the callee's context from the capture and the target's effective actor**:
   database gateway (with the target's effective actor), default repository, network
   gateway and secret grants are derived for the TARGET (as the top-level
   `invoke_function_in_db_as` does), not inherited from the caller. Batch context, globals,
   the shared fuel pool and the depth/limit keep being passed exactly as today (do not
   change the fuel/lease handling in `host_call`).
6. **Active-call identity path.** An immutable per-lineage path of function identities,
   shared by every stored-call mechanism, at most 32 entries INCLUDING the root. Before
   entering a target: refuse if its identity is already on the path (recursion across stored
   functions is rejected) or if adding it would exceed 32. The identity must be stable across
   rename and artifact replacement (do not use the mutable name; look at how functions are
   identified in the catalogue and the registry and pick the stable id, or introduce one if
   none exists, and say which). Entries are removed by a scope guard on return, error,
   cancellation and task teardown. The root invocation seeds the path. The existing lower
   depth policy stays; the 32 cap does not raise any lower limit.
7. The gateway-originated `Call` stays refused before dispatch (already done in
   `FacadeDbGateway::execute`); do not change it. Keep the existing transactional `Call`
   guard.

Architecture constraint: `shamir-wasm-host` cannot depend on `shamir-db`. The authorization
and catalogue live in `shamir-db`, the `call` import in `shamir-wasm-host`. Add the narrowest
seam: a trait object carried by `FnCtx` (`crates/wasm/shamir-wasm-host/src/runtime/context.rs`,
see how `DbGateway` and the net gateway are carried) that `shamir-db` implements and sets
when it builds the root `FnCtx`. When no resolver is present (plain registry use in
host-only tests) the import must still work as today ONLY for contexts that have no catalogue
(state this clearly in a doc comment and in the final message; production contexts built by
`shamir-db` always carry the resolver, so production never takes the unchecked path).
Visibility: widen nothing silently; report any item that has to become wider.

## Tests (they run in CI, not here)

Prefer native (non-wasm) functions registered in the function registry, because Rust-to-wasm
guests may be skipped in CI; look at `crates/engine/shamir-db/tests/functions/` and
`crates/wasm/shamir-wasm-host/src/tests/` for how nested calls (including the RI-7 actor
tests) are tested today. Cover, each as its own test:

* Execute denied on the target for the current actor -> the nested call fails, no body runs.
* `Definer` target runs as its owner even when called by a restricted parent; the callee
  gateway/secrets/net are the target's, not the parent's (assert a table the parent cannot
  access is reachable by the Definer target and not the other way round).
* `Invoker` target (no setuid) keeps the current actor; legacy setuid Invoker runs as owner.
* Definer parent calling an Invoker target of a different owner is refused; same owner is allowed.
* Recursion: a function calling itself, and a two-function cycle, are refused at the repeat
  (not at the depth limit); a chain of 32 distinct functions including the root works, a 33rd is
  refused; the path is restored after a success, an error and a cancelled call (a second
  call of the same target after a failed one is allowed).
* Rename/replacement does not evade repeat detection (identity is stable).
* Missing target and missing metadata fail closed.
* Existing tests: read the ones that encode the old behaviour (the RI-7 actor inheritance
  tests, nested call tests). Where one asserts the PARENT's actor for a Definer/setuid target
  it encodes the defect: change it deliberately and list each such change in the final
  message with the reason. Never delete or weaken any other test.

## Observable acceptance

1. All of the above implemented and tested; expectations derived from the rules above.
2. `cargo fmt -p <crate> -- --check`, `cargo check -p <crate> --all-targets` and
   `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate
   (`shamir-wasm-host`, `shamir-db`, and `shamir-engine` if touched).
3. `python -I scripts/ci/check_layout.py` passes (at most 7 entries per folder, 1000 lines per
   file; split rather than grow).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`,
   no new `#[allow]`, no visibility widening beyond what the seam strictly needs (list it).

## Non-goals

`call_checked`, any language/compiler/admission change, fuel/lease changes, gateway-originated
`Call` support, references, update/delete, version changes.

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
