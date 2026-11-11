# Brief: do not leak stored-procedure signatures through compile diagnostics (SEC-CALL-SIG-LEAK)

## Why

Stage 2b (`crates/20-call-checked.md`, landed) lets the `shamir` compiler check `call("target", {...})` against a
catalogue of the signatures of all stored `shamir` procedures that `shamir-db` supplies at create/check time
(`CompileOptions::callable_catalog`, built by `callable_catalog_snapshot` in
`crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/facade/source_gate.rs`). The snapshot is NOT filtered by
the caller's rights, and the checker's diagnostics (`T004` and the type-mismatch texts in
`crates/wasm/shamir-lang/src/sem/check/builtins/call_signature.rs`) name the target's declared parameter names,
types and result. So a principal that may create or check a function but has no Execute right on `target` can learn
that `target` exists and its exact signature by compiling probes. Runtime resolution already authorizes Execute before
comparing signatures (same error text for every refusal); the compile path must be equally opaque. Read the two files
above, `db/function/access/authorize.rs` (record-aware Execute), `facade/check.rs`, `management.rs` create paths, and
`docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md` sections 2.1-2.3 first.

## What (decisions are made; do not re-open them)

1. **Filter the catalogue by Execute.** `callable_catalog_snapshot` takes the compiling actor and includes only the
   procedures that actor has Execute on, evaluated with the same record-aware policy and the same per-row metadata
   the runtime uses (`authorize_invocation`-style check on the row under the current actor, administrators and owners
   as the policy says). Rows with malformed security metadata are excluded, not errors. The check must stay cheap (row
   metadata only; no module decoding or hashing; the previous review already moved that out of the snapshot) and
   must not hold any function DDL lock.
2. **Uniform diagnostics.** A target that does not exist, is not a `shamir` procedure, has no signature, or that the
   actor may not execute produces the SAME diagnostic with the SAME text (no parameter names, types or result; no
   hint which of the cases applies). The mismatch diagnostics (wrong argument name/type, wrong result use) can only be
   produced for targets present in the filtered catalogue, so they may keep naming the expected signature (the actor is
   allowed to execute that target).
3. **Systems that compile on behalf of others.** The actor used is the principal performing create/alter/check.
   Do not use the effective (Definer) actor at compile time. Document this in the design addendum (one short
   paragraph: compile-time visibility is the creator's Execute right; a later loss of the right fails at runtime by
   the existing stage 2a check).
4. **No other surface change**: no new diagnostic codes unless unavoidable (then say so), no ABI, persistence or
   host change.

## Tests (run in CI, not here)

* Compiler unit tests: absent target and non-visible target produce byte-identical diagnostics; visible target keeps
  the precise mismatch diagnostics.
* `shamir-db` end to end (`tests/functions/gateway/`, next to `checked_stored_calls_e2e.rs`): actor without Execute on
  `leaf` cannot distinguish `leaf` from a nonexistent name via `check_function` and via `create_function`; after an
  Execute grant the same source compiles; an administrator and the owner see it; revoking Execute makes a freshly
  compiled call fail at compile time again and an already stored caller fail at runtime with the existing refusal.
* Never delete or weaken existing tests; list every changed one with the reason.

## Observable acceptance

fmt/check/clippy clean for every touched crate (`shamir-lang`, `shamir-wasm-host`, `shamir-db` as touched);
`git add -A` then `python -I scripts/ci/check_layout.py` passes (<=7 entries per folder, <=1000 lines per file; rerun
on the final tree); rust-guard rules (no logic in `mod.rs`, no inline test modules, no local `use`, no new `#[allow]`,
no visibility widening beyond what is strictly needed, list every one). Docs in the same change: the addendum
paragraph from item 3.

## Constraints

Compilation, builds and tests run in CI, not here. Allowed locally only: `cargo fmt -p <crate>`,
`cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --` with
`-j 2`. No `cargo build/test/run/bench/doc`, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: what changed, tests added/changed and why, widened items, anything
unverified. Do not end on a tool call.
