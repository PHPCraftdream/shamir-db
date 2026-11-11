# Brief: PROC-ATOMIC phase 1 — `abort_on_skip` on the wire, in the engine and in the builders

## Why

Phase 1 of `docs/dev-artifacts/design/shamir-lang-atomic-execute.md` (read it completely first: the decisions are
made and are not to be re-opened). A transactional batch needs a way to say "if this entry would be skipped, abort
the whole transaction instead". No engine mechanism exists. Phase 1 adds it end to end for native clients; the
`shamir` language is NOT touched here (phase 2, `25b-proc-atomic-lang.md`).

## What

1. **Wire.** `crates/query/shamir-query-types/src/ops/batch/ops/query_entry.rs`: add
   `pub abort_on_skip: bool` after `when`, with `#[serde(default, skip_serializing_if = "std::ops::Not::not")]` and a
   doc comment. Absent bytes for `false`: every existing encoding stays byte-identical. `From<ReadQuery>` sets
   `false`. Add `abort_on_skip: false` to every `QueryEntry { .. }` literal in the workspace (about 163 sites in 40
   files; `grep -rn "QueryEntry {" --include=*.rs crates`). This sweep is mechanical: keep it in its own commit-sized
   chunk of work and do not change anything else in those files.
2. **Engine, skip path.** `crates/engine/shamir-engine/src/query/batch/runner/query_runner/helpers.rs`: add
   `skip_or_abort(entry, alias, skipped) -> Result<bool, BatchError>` returning
   `BatchError::query_coded(alias, "skip_aborted", "entry skipped under abort_on_skip")` (use the real constructor
   name/shape of `BatchError` in this repo) when `skipped && entry.abort_on_skip`, else `Ok(skipped)`; and
   `abort_reason(&BatchError) -> String` returning exactly `"skip_aborted"` for that code and the existing
   `format!("{:?}")` text for everything else.
3. **Engine, executor.** `crates/engine/shamir-engine/src/query/batch/exec/batch_execute.rs` is at 994 of 1000
   lines: the net growth must be at most +6 lines (put logic in helpers; move code out if needed, keeping behaviour).
   In `execute_plan_tx_impl` wrap the `resolve_skip(..)` result in `skip_or_abort(..)?` (the transactional path
   only). In the transactional error arm that builds the abort `reason` use `abort_reason(..)` instead of
   `format!("{:?}", ..)`. The error already flows through the existing rollback path (locks released, response `Ok`
   with empty `results`, `transaction.status = "aborted"`); verify that by reading, do not change that path.
   The non-transactional runner must refuse the flag (item 4) so it never needs a skip-abort branch.
4. **Engine, scope validation.** `.../exec/batch_validate.rs`: add `validate_abort_on_skip_scope(&queries)`: a
   `BatchTreeVisitor` walk over the whole tree (nested `Batch`/`ForEach` bodies included); when any entry has the
   flag and the ROOT is not `transactional: true`, return a `BatchError::QueryError` with code `validation` and
   message `abort_on_skip requires a single-call transactional batch` BEFORE any operation runs (call it from the
   planning/validation step that already runs before execution, at depth 0 only). `interactive_tx.rs`
   (`execute_in_open_tx`) calls it unconditionally so any flag there is refused with the same message.
5. **Native builder.** `crates/query/shamir-query-builder/src/batch/batch_refs.rs` (next to `when`):
   `Batch::abort_on_skip(&mut self, handle: &Handle) -> &mut Self`; an unknown handle records a deferred
   `BuildError` exactly as `when` does. No transactional validation in the builder.
6. **TS builder.** `crates/client/shamir-client-ts/src/core/builders/write/batch.ts` (753 lines): the `add(alias, op,
   { when, abortOnSkip })` option emits `abort_on_skip: true` only when true; `src/core/types/write/batch.ts`:
   `abort_on_skip?: boolean`.
7. **Shared fixture.** `crates/query/shamir-query-builder/tests/fixtures/wire_ops_matrix.json`: add the case
   `entry_abort_on_skip_tx` (a transactional batch with a `when` entry carrying `abort_on_skip`). Both
   `tests/wire_ops_matrix.rs` and the TS `wire_ops_matrix_parity.test.ts` consume it; extend them only as far as
   needed to cover it.
8. **Protocol docs.** `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md`: a `reason:
   "skip_aborted"` row (not retryable without changing the input) and the `validation` refusal text; one short note
   in `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md` that `abort_on_skip` is an opt-in per entry
   that turns a skip into an abort inside a single-call transactional batch.

## Tests (run in CI, not here; respect the folder limit: check entry counts, add a sibling folder or file when a folder would pass 7)

* `shamir-query-types/src/ops/batch/tests/batch_types_tests/abort_on_skip_wire_tests.rs`: key absent when false (bytes
  equal the pre-change encoding of an entry with `when`); present when true; non-bool rejected; an old shape defaults
  to false; the key sits on `QueryEntry`, not on the op.
* `shamir-query-builder/src/batch/tests/flow/when_tests.rs` (extend): unknown handle gives a deferred error; key
  emitted only when set; `to_msgpack` equals `BatchRequest` serde.
* `shamir-engine/src/query/batch/tests/executor/guards/abort_on_skip_tests.rs`: (a) transaction: insert A then a
  flagged B with `when` false -> `aborted`, `reason == "skip_aborted"`, empty results, A not visible; (b) a cascaded
  skip aborts; (c) flagged entry inside a nested `Batch` and inside a `ForEach` body aborts the whole transaction
  with the exact token (pins that errors are not re-wrapped; if they are, report it and make the token survive);
  (d) non-transactional root with the flag at the root or nested -> `validation`, zero writes; (e) interactive
  `execute_in_open_tx` -> `validation`; (f) `when` true -> commits, response equals the unflagged one; (g) without the
  flag a skip inside a transaction still commits the other ops (OQL-03 regression); (h) Pessimistic isolation: after
  an abort another transaction can take the lock.
* TS: a test next to the batch builder tests: `abortOnSkip` emitted only when true.
* Never delete or weaken existing tests; list every changed one with the reason.

## Observable acceptance

fmt/check/clippy clean for every touched crate (`shamir-query-types`, `shamir-query-builder`, `shamir-engine`, and
every crate whose `QueryEntry` literals changed: find them with `cargo check` per crate, `-p <crate> --all-targets`);
`git add -A` then `python -I scripts/ci/check_layout.py` passes on the final tree (<=7 entries per folder, <=1000
lines per file); rust-guard rules (no logic in `mod.rs`, no inline test modules, no local `use`, no new `#[allow]`,
no visibility widening beyond what is strictly needed, list every one). Existing wire bytes unchanged.

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

Your last message must be a final report: what changed, the `QueryEntry` sweep file count, tests added/changed and
why, widened items, anything unverified (notably the nested abort-token question). Do not end on a tool call.
