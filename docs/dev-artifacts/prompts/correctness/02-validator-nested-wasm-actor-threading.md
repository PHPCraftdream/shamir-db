# RI-7: Thread the real caller actor through validators and nested WASM calls

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context and DECIDED contract (do not re-litigate)

Two related actor-semantics gaps, both investigated in full before this brief
was written, both independently reviewed by two separate consulting agents
who converged on the same conclusion. **The contract is decided — implement
it, do not second-guess it:**

1. **Validators see the CALLER's actor**, not `Actor::System`. A
   `foreign_key`/`unique`/custom validator's cross-table reads run with
   exactly the privileges the caller already has (the same actor that just
   passed table-level ACL for THIS write). This is fail-closed (worst case:
   a legitimate write is rejected because the caller can't see a referenced
   table — a UX bug, not a security bug) and matches this codebase's
   existing "ambiguous actor → return caller, not System" convention.
2. **Nested WASM `ctx.call(...)` inherits the PARENT `FnCtx`'s actor**, not
   `Actor::System`. The callee already shares the parent's batch context,
   globals, and fuel budget (per `host_call.rs`'s own doc comment — "the
   callee shares the same batch context and globals") — actor is part of
   the same invocation identity and must travel with it. The current
   System-default is a **confused-deputy privilege-escalation primitive**:
   any principal who can trigger a function that makes ONE nested call
   reaches `Actor::System` one hop in, regardless of what ACL checks the
   nested function itself performs — and this is invisible at code-review
   time (the callee's own actor checks look correct; the bypass is in the
   call-site plumbing, not the callee's logic).

Neither of these is a "maybe" — implement both as the caller/parent-
inheriting default. A future Definer/setuid-style explicit opt-in (a
function or validator explicitly declared to run with elevated privileges)
is the correct mechanism for any genuine escalation need — that is
EXPLICITLY OUT OF SCOPE for this task (do not design or build it now; if you
find a concrete need for it while implementing, note it in your summary as
a follow-up, don't build it ad hoc).

## The task — Part 1: validators (`crates/shamir-engine`)

`TableManager::run_validators_qv` and `run_validators_view`
(`crates/shamir-engine/src/table/table_manager_validators.rs`) ALREADY take
an `actor: &shamir_types::access::Actor` parameter — the plumbing exists.
Every call site is in `crates/shamir-engine/src/table/write_exec.rs`
(8 occurrences of `&Actor::System`, all inside `run_validators_qv`/
`run_validators_view` calls within `execute_insert_tx`, `execute_update_tx`,
`execute_delete_tx`) and ALL of them currently hardcode `&Actor::System`.

The real caller's actor is available one layer up:
`crates/shamir-engine/src/query/batch/query_runner.rs`'s `QueryRunner`
struct already holds `pub actor: Actor` — the authenticated caller — and
uses `trace_access(&self.actor, &resource, Action::Write)` for table-level
ACL immediately before calling `execute_insert_tx`/`execute_update_tx`/
`execute_delete_tx` (6 call sites total, all in this one file — verified,
this is the ONLY caller of these three functions in the engine).

1. Add an `actor: &shamir_types::access::Actor` parameter to
   `execute_insert_tx`, `execute_update_tx`, `execute_delete_tx`
   (`write_exec.rs`).
2. Update all 6 call sites in `query_runner.rs` to pass `&self.actor`.
3. Replace every internal `&Actor::System` passed to `run_validators_qv`/
   `run_validators_view` inside these three functions with the new `actor`
   parameter.
4. Update `run_validators_qv`'s doc comment
   (`table_manager_validators.rs:~114-131`) to state the decided contract
   plainly: *"validator cross-table reads execute with the CALLER's
   privileges; a validator referencing a table the caller cannot read will
   reject the write."*
5. Check `crates/shamir-engine/src/table/write_exec.rs` for any OTHER
   caller of these three `execute_*_tx` functions beyond `query_runner.rs`
   (grep to confirm — the brief's own investigation found none, but verify
   independently) and thread actor through those too if any exist.

## The task — Part 2: nested WASM (`crates/shamir-wasm-host`)

`FnCtx` (`crates/shamir-wasm-host/src/context.rs`) already has `actor:
Actor`, `with_actor(actor: Actor) -> Self`, and `actor(&self) -> &Actor` —
confirmed working plumbing (doc comment: "R2 adds actor — the Actor that
initiated the invocation, threaded through nested calls"). The gap is that
`HostState` (`crates/shamir-wasm-host/src/wasm/wasm_function.rs`) — the
struct living inside the wasmtime `Store`, read by host-import
implementations via `caller.data()` — has NO `actor` field, so the parent's
actor never survives the FnCtx→HostState→(nested call)→FnCtx round trip.

1. Add `pub(super) actor: Actor` to `HostState`
   (`wasm_function.rs`, near the other per-invocation fields like `depth`).
2. In `WasmFunction::call` (the SINGLE entry point used for both the
   top-level invocation and every recursive nested call — verify this by
   reading the `impl ShamirFunction for WasmFunction` block in full), add
   `let actor = ctx.actor().clone();` alongside the existing
   `globals`/`registry`/`depth`/etc. extraction, and include `actor` in the
   `HostState { ... }` construction (the real one — check if the
   `#[cfg(test)]` `test_linker_and_store` helper also needs a dummy actor
   field added to compile; use `Actor::System` there since it is a test-only
   bare-linker fixture unrelated to this invocation-identity flow).
3. In `host_call.rs`'s `host_call` function, Phase 1 (the sync
   clone-out-of-`caller.data()` block), add
   `let actor = state.actor.clone();` alongside the existing
   `registry`/`batch_ctx`/`globals`/`next_depth`/`depth_limit`/`fuel_budget`
   clones.
4. In Phase 2, add `.with_actor(actor)` to the `child_ctx` builder chain
   (currently `FnCtx::with_globals(globals).with_registry(reg)
   .with_depth(next_depth).with_depth_limit(depth_limit)
   .with_fuel_budget(fuel_budget)`), replacing the
   `// TODO(Shomer R2): thread actor from parent FnCtx into child` comment
   with a note that this is now done.
5. **Bonus finding from investigation** (optional, lower priority, only if
   time permits without expanding scope significantly): `host_call.rs`'s
   child `FnCtx` construction also does NOT thread `db`, `repo`, `net`, or
   `secret_grants` from the parent — these fail CLOSED (the nested call
   simply loses DB/net/secret-grant capabilities it should have inherited)
   so they are lower urgency than the actor fail-OPEN issue, but are the
   same unfinished-plumbing site. If straightforward, thread them too
   (`ctx.db_gateway()`/`ctx.repo()`/`ctx.net_gateway()`/`ctx.secret_grants()`
   are presumably accessible the same way `ctx.actor()` is — verify). If
   this expands scope non-trivially, SKIP it and note it as a separate
   follow-up in your summary — do not let it block the actor fix.

## Regression tests (MANDATORY — this is a security-semantics change)

For Part 1 (validators): add tests proving the CALLER's actor is what a
validator sees, not System. Concretely: register a `foreign_key`-style (or
a custom check) validator that reads a second table; run a write as a
caller actor that has ACL access to that second table (write succeeds) and
as a caller actor that does NOT (write fails/denies) — proving the
validator's effective read permission tracks the real caller, not a
hardcoded System bypass. Place in
`crates/shamir-engine/src/table/tests/` (or wherever `write_exec.rs`'s
existing tests live — check the `tests/` convention for this module) or
`crates/shamir-engine/src/validator/tests/` as appropriate.

For Part 2 (nested WASM): add a test proving a nested `ctx.call(...)`
sees the SAME actor as its parent invocation — e.g. a WASM function that
calls `ctx.actor()` (if exposed to guest code) or, more practically, a
function whose nested call target performs an ACL-gated action and assert
it succeeds/fails consistently with the ORIGINAL caller's actor, not
System. Check `crates/shamir-wasm-host/src/wasm/tests/` for the existing
test harness pattern for invoking nested `ctx.call` chains (there should
already be depth-limit tests exercising this call path — mirror their
setup).

Both test additions must independently prove the OLD behavior (hardcoded
System) would have failed the test — i.e., temporarily revert your fix
locally, confirm the new test fails, then re-apply the fix and confirm it
passes. Report this proof in your summary (per this campaign's established
regression-test discipline — see e.g. how task #729's fix was verified).

## Out of scope

- Do NOT design or implement a Definer/setuid opt-in escalation mechanism
  for validators or WASM functions — that is a separate, larger feature.
- Do NOT touch any ACL/access-control code outside the two call chains
  described above.
- Do NOT change `ValidatorBinding`'s schema (confirmed today: only
  `validator_id`, `ops`, `priority` — no definer field, and none should be
  added here).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @engine -- validator` and `./scripts/test.sh -p
  shamir-wasm-host --full` green, including your new regression tests.
- `./scripts/test.sh @engine --full` green (no regressions in the broader
  write-path test suite from the new `actor` parameters).
- Report the revert-and-confirm-fail proof for both new regression tests.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above.
