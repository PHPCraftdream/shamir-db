# Brief: `batch_update`, `batch_delete` and `response_affected` for `shamir` procedures (PROC-2c stage 1)

## Why

Procedures written in the `shamir` language can read and insert through the typed batch
builder (`batch_read`, `batch_insert`, `request`, `db_execute`, `response_rows`,
`response_inserted`) but cannot update or delete. The reviewed design is
`docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md`: this brief implements ONLY its
stage 1 (sections 4.1 to 4.3, the stage 1 rows of sections 6 and 7). Read that document
first, completely, and `docs/dev-artifacts/design/shamir-lang-design.md` sections 4 and 6
for the existing builder intrinsics. Where the addendum and the code disagree, the code is
the truth: report the difference in the final message instead of guessing.

## What

Add exactly the signatures of addendum section 4.1:

```text
batch_update(Batch, string alias, string table, Predicate filter, WireRecord set) -> Batch
batch_delete(Batch, string alias, string table, Predicate filter) -> Batch
response_affected(Response, string alias) -> i64?
```

* `Predicate`: a non-empty record of non-null scalar literals or parameters, lowered as a
  conjunction of field equalities; reject (compile diagnostics, reuse the existing
  codes as the addendum says) a bare scalar, a null, an empty record, a nullable field, a
  reference, any other expression. `set`: a non-empty typed wire-safe record. Alias and
  table must be compile-time string literals; table `main` only (no repository qualifier).
* No returning mode, no projection, no compare-and-swap, no new host import, no engine or
  wire DTO change, no ABI change; the emitted-module import count does not increase.
* Lowering (addendum 4.2): lower to the same request DTO the native builder produces for
  `Update`/`Delete` (read how `batch_read` and `batch_insert` are lowered in
  `crates/wasm/shamir-lang/src/back/batch/{lower,template}.rs` and mirror it; look at
  `crates/query/shamir-query-builder` for `update(...)`, `delete(...)`, `Batch::try_update`,
  `try_delete`, and the wire types in `crates/query/shamir-query-types`). Alias insertion
  order is kept. `response_affected` reads `results[alias].stats.records_scanned` as the
  addendum specifies, with the kind-3 projection errors listed there (missing root,
  missing stats/count, wrong type, negative, above `i64::MAX`; a missing alias is null only
  when a valid root `results` map exists). The compiler must carry operation-kind metadata
  from Batch through Request to Response as the addendum notes (the current helpers do not
  track it) only as far as needed for the compile-time checks of this stage.
* Update the guest-side helpers in `crates/wasm/shamir-lang/src/back/emit/support/db.rs`
  and the type checker `crates/wasm/shamir-lang/src/sem/check/builtins.rs` following the
  existing intrinsic conventions (contextual result types, `T010`, effects: these are
  procedure-only builtins like the existing batch ones).

## Tests (acceptance criteria; they run in CI, not here)

Put them next to the existing ones in the project's test layout (host runtime tests under
`crates/wasm/shamir-wasm-host/src/tests/runtime/lang/` such as `batch_tests.rs`, compiler
tests under `crates/wasm/shamir-lang/src/api/tests/`; one file per topic, folders at most 7
entries, files at most 1000 lines; JSON literals multi-line):

1. Byte oracle: the guest request bytes equal the native
   `Batch::try_update(...)`/`try_delete(...)` + `to_msgpack()` bytes for representative
   updates and deletes (same technique as `assert_matches_native_batch` in `batch_tests.rs`).
2. Rejection tests, one per rule: missing/empty/null/nullable predicate, bare scalar
   predicate, reference or expression in a predicate, empty `set`, non-literal alias or
   table, unsupported shapes; each with the exact expected diagnostic code.
3. Execution against a real database in `crates/engine/shamir-db/tests/functions/gateway/`
   (mirror `shamir_lang_procedure_e2e.rs` and `shamir_lang_definer_e2e.rs`): a procedure
   updates and deletes rows and `response_affected` returns the changed/removed counts
   (changed-only count for update, removed count for delete); a restricted caller with
   `Security::Invoker` is denied (Write for update, Delete for delete) and the same
   procedure with `Security::Definer` succeeds with the owner's rights; pin that
   `stats.records_scanned` equals the engine's `WriteResult.affected`.
4. Projection errors (kind 3): malformed response, missing stats, wrong type.

## Observable acceptance

1. All of the above exist and derive expectations from the platform rules; no loose
   assertions; no existing test weakened, renamed or removed.
2. `cargo fmt -p <crate> -- --check`, `cargo check -p <crate> --all-targets` and
   `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate
   (at least `shamir-lang`, `shamir-wasm-host`, `shamir-db` if its tests are touched).
3. `python -I scripts/ci/check_layout.py` passes.
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`,
   no new `#[allow]`, no visibility widening (report any item that would need it).
5. The `shamir-lang` production dependencies stay exactly `shamir-wasm-admission`,
   `wasm-encoder` and `thiserror`; the purity scan test must stay green (no `std::fs`,
   `process`, `net`, `env`, `thread`).
6. Documentation: do not edit `shamir-lang-design.md` or the addendum; the orchestrator
   updates the documents after CI.

## Non-goals

References, `call_checked`, returning modes, projections, repository-qualified operations,
engine/DTO/ABI changes, version changes.

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
