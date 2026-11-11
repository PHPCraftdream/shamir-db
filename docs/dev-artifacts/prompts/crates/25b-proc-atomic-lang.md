# Brief: PROC-ATOMIC phase 2 — `batch_tx()` in the `shamir` language, guest checks, host oracle, end to end

## Why

Phase 2 of `docs/dev-artifacts/design/shamir-lang-atomic-execute.md` (read completely first; decisions are made).
Phase 1 (`25-proc-atomic-engine.md`) added `QueryEntry.abort_on_skip`, its engine semantics and
`Batch::abort_on_skip` in the native builder; it is already in this tree. Phase 2 lets a procedure write
`batch_tx()` and makes the guest react correctly to every transactional outcome.

## What

1. **Semantics.** `crates/wasm/shamir-lang/src/sem/effects.rs`: add `BatchTx` / `"batch_tx"`.
   `sem/check/builtins/core.rs`: signature `() -> Batch`, same effects as `batch`. `sem/check/entry.rs`:
   `PlanFact::Batch { .., transactional: bool }`. `sem/check/builtins/batch/metadata.rs`: `BatchTx` makes a
   transactional fact; every append keeps the bit. `sem/tables.rs`: record a `transactional` bit per reference sink.
   References on a transactional plan are accepted under the existing rules.
2. **IR.** `back/ir/module.rs`: `BatchUses.transactional`, set by a `mark_transactional()` in the builder (like the
   reference-guard mark).
3. **Batch lowering.** `back/batch/template.rs`: a lazily baked second static `BatchRequest` cell with
   `transactional = true` (field 1, `BOOL`, payload 1) and transactional guarded read/insert descriptors
   `[..., "when", "abort_on_skip"]` with a static `true` cell (all `Option`, never baked eagerly).
   `back/batch/lower.rs`: a `BatchTx` arm; guarded read/insert in a transactional plan use the transactional
   descriptor. Unguarded operations unchanged. A module that does not reach `batch_tx` must be byte-identical to
   the current output.
4. **Emission.** New `back/emit/support/db/tx.rs` (folder entry count to check; `support/db/mod.rs` re-exports only):
   static keys/messages and `emit_response_tx_check_helper(..)` with signature `(request, response) -> response`,
   implementing the table of design section 3 exactly (statuses, reason tokens, kinds 2/3, the constant messages;
   never copy `reason` into a message). `support/db/core.rs`: `db_execute` calls the transactional check (only when
   the request cell's `transactional` field is true, read at runtime) BEFORE the existing skip check.
   `back/emit/module.rs` (889 lines): one `Option<TxCheckPlan>` field plus helper registration; keep the file under
   1000 lines. The six-import ceiling, ABI and wire are unchanged.
5. **Docs.** Update `docs/dev-artifacts/design/shamir-lang-design.md` (builtin table; the "Exact DTO mapping" sentence
   that says `transactional=false`: it is false unless the plan is rooted in `batch_tx`) and add one-line
   "superseded by shamir-lang-atomic-execute.md" notes in `shamir-lang-call-refs-addendum.md` section 1 decision 6,
   section 8 and section 9. Functions guide `docs/guide-docs/guide/05-functions.md`: replace the atomicity warning
   with the real rule (`batch_tx` makes ONE `db_execute` atomic; everything else as before; Snapshot isolation,
   last-writer-wins; a wall-clock timeout is not proof of rollback). `docs/dev-artifacts/release-1/06-deferred-and-
   non-goals.md`: record the commit-cancel-on-timeout risk and the joined-spawned-task idea as deferred.

## Tests (run in CI, not here; check folder entry counts, add sibling folders when a folder would pass 7)

* `shamir-lang/src/sem/check/builtins/batch/tests/transactional_tests.rs`: `batch_tx` has type `Batch`; wrong arity
  is `T004`; the bit propagates through `batch_id`, `batch_read`, `batch_insert`, `batch_update`, `batch_delete`,
  `request`; references accepted on a transactional plan; scalar/validator capability rules unchanged.
* `shamir-lang/src/back/lower/tests/transactional_tests.rs`: static `BOOL = 1`; only guarded ops carry
  `abort_on_skip`.
* `shamir-lang/src/back/emit/tests/tx_emission_tests.rs`: a module without `batch_tx` has no tx check plan, no
  transactional strings in its static image and an unchanged helper count (byte-identical output pinned against an
  existing fixture module).
* `shamir-wasm-host/src/tests/runtime/lang/procedures/tx_batch_tests.rs`: byte oracle: the guest request equals
  `shamir_query_builder` `Batch::new().transactional()` + `.when(h, g)` + `.abort_on_skip(&h)` + `to_msgpack()`;
  fake-gateway responses map exactly to every row of the design table including kind 3 for a malformed
  `transaction`; a non-transactional request in the same module tolerates a missing `transaction`.
* `shamir-db/tests/functions/gateway/shamir_lang/scalar_refs/atomic_rollback.rs`: (1) insert then a failing op
  (unique violation) in one `db_execute` -> kind 2 `transactional batch aborted`, table unchanged; (2) reference
  guard fails -> kind 2 reference message and the earlier insert is rolled back; (3) the positive path commits all.
  Reuse `support.rs`; do not widen visibility.
* `.../scalar_refs/atomic_acl.rs` (the folder would reach 7 entries: re-check): Definer transaction writes as the
  owner where the invoker lacks rights; an Invoker missing Create on the second table is denied before `begin_tx`
  with nothing written; denial classification unchanged.
* `shamir-db/tests/functions/facade/builder_execute_e2e.rs` (extend): a gateway request shape
  `TransactionalCrossRepo` built with the native builder -> cross-repository refusal, nothing written (the language
  cannot express repositories).
* Never delete or weaken existing tests; list every changed one with the reason.

## Observable acceptance

fmt/check/clippy clean for `shamir-lang`, `shamir-wasm-host`, `shamir-db`; `git add -A` then
`python -I scripts/ci/check_layout.py` passes on the final tree; rust-guard rules (no logic in `mod.rs`, no inline
test modules, no local `use`, no new `#[allow]`, list every visibility widening). Modules that do not use
`batch_tx` compile to the same bytes as before.

## Constraints

Compilation, builds and tests run on GitHub CI, not on this machine. Banned unless the user explicitly asks:
`cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`,
`npm`/`tsc`/`vitest` builds and test runs, WASM builds, e2e harnesses, any release or full-workspace build.
Allowed locally, only these, scoped to the touched crates (`-p <crate>`): `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `-- --check`,
`python -I scripts/ci/check_layout.py`, read-only git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --` with
`-j 2`. If you believe you need a build or test run, say so in your final message.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: what changed, tests added/changed and why, widened items, anything
unverified. Do not end on a tool call.
