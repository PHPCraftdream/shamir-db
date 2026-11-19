# Brief: R1c fix — the migration model must admit only the functional indexes the server can actually build

Context (read first): `docs/dev-artifacts/design/app-schema-migrations.md` §3 and §5 (typed bodies; a migration is journaled BEFORE its DDL runs, so an admitted-but-unbuildable operation strands the migration in doubt). Landed: the typed model with `SchemaMigrationOp::validate` in `crates/query/shamir-query-types/src/admin/types/schema_migration/model/operation.rs`, the canonical encoder, the server apply in `crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/apply/` (the operation intent is persisted before `apply_operations.rs` calls the DDL handler, and a DDL failure afterwards records an in-doubt outcome), the Rust client (`crates/client/shamir-client/src/receipts/schema_migration/`) and the TypeScript client (`crates/client/shamir-client-ts/src/core/builders/admin/schema-migration/`, validation in `model/validation.ts` and the wrappers in `model/ddl-wrappers.ts`).

## The defect (found by independent review of the TypeScript client, to be confirmed or refuted by you from the code)

The model admits `create_index` with `index_type = "functional"` and a `functional_op` from the allowlist `lower | upper | trim | length | substring | mod | coalesce | concat`, plus optional `functional_args` (query values). The review claims: (a) the server registers scalar functions under library-qualified names (for example `strings/substring`, see `crates/query/shamir-funclib/src/lib.rs` ~l.56–60 and `registry.rs` ~l.173–179) and `ScalarResolver::get` looks up the given name without adding a prefix (`scalar_resolver.rs` ~l.182–188), so a bare name such as `substring` may not resolve; (b) the functional-index backend handles only a few names directly and otherwise needs a trusted indexable scalar; (c) the index expression built for the backend receives only the indexed field and NOT `functional_args` (`write_domain_index_admitted.rs` ~l.223–250, `crates/index/shamir-index-core/src/model/query/expr.rs` ~l.176–187), so `substring [1,2]` cannot mean what the author wrote. Existing server tests build functional indexes with `functional_op("lower")` (many e2e tests), so at least `lower` works. If any admitted operation fails inside the DDL handler AFTER the journal intent, the migration is stranded in doubt, which is exactly what admission is supposed to prevent.

## What (this step only)

1. **Investigate from the code, per allowlisted name** (`lower`, `upper`, `trim`, `length`, `substring`, `mod`, `coalesce`, `concat`): does `create_index` with this `functional_op` (with and without `functional_args`) succeed on the real server, does the built index actually serve lookups, and does `functional_args` change the index expression? Produce a table in your final report with the file/line evidence for every row. Do not guess: where the code does not settle it, say so and cover it with the CI test of item 3.
2. **Decide the admitted subset = what the server demonstrably builds and uses.** Narrow admission to exactly that subset, at EVERY layer that admits: the Rust model (`operation.rs` and its tests), the canonical encoder if it enumerates names, the TypeScript wrappers and `validation.ts` (the `FUNCTIONAL_INDEX_OPS` set and the argument rules), the Rust and TypeScript client guides where they list admitted forms, and the shared vector file only if a case uses a now-refused form (do NOT change the canonical bytes of any surviving case; if a case must change, say why and regenerate both expectations from the Rust implementation). `functional_args` that the server ignores must be refused (not silently dropped), because the canonical checksum would cover a value the server never applies. If every non-`lower`-class name fails, the allowlist shrinks accordingly; if the server can be taught cheaply and safely to build more (for example by resolving the bare name through the funclib registry), prefer REFUSING in the model and recording the extension as a note, not changing the server's index semantics in this step.
3. **Server-side proof (CI-only test).** In `shamir-db` next to the existing schema-migration apply tests, add a deterministic test that applies, through the real apply path, a migration creating a functional index for EACH admitted `functional_op` and then queries through the index (assert the index is used and the results are right), and tests that every refused form is refused by `plan`/validation BEFORE any intent is written (assert nothing was journaled). Follow the stack lessons below.
4. **Client parity tests.** Rust (`shamir-client` and `shamir-query-builder` model tests) and TypeScript (`migration/__tests__/migration-model.test.ts` style) tests that the refused forms fail locally with the typed error and the admitted forms pass; the TypeScript refusal messages equal the Rust ones in meaning.

## Do not

Change the canonical format or any surviving vector's bytes, the journal/fence protocol, the wire protocol, unrelated DDL families, or the server's index semantics; add a dependency; use fixed sleeps.

## Layout and test lessons

Count every folder you add to with `ls` (several are at the 7-entry limit; `python -I scripts/ci/check_layout.py` counts tracked files only: recheck after the orchestrator stages); at most 1000 lines per file, one primary item per file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, builder-only construction (no hand-built wire JSON). Debug-test stacks are small: never call the giant `execute_admin` from a test thread, call the concrete handler through `Box::pin`, one `execute_operation` call site, deep real-server tests on a roomy thread (`on_big_stack` pattern of the `shamir-db` schema-migration tests). Persistent stores for apply tests, every `TempDir` kept alive for the whole test, one runtime per test, every await bounded, assert on invariants and sets.

## Rules that bind the change

* Machine memory: at most ONE cargo command at a time machine-wide, ALWAYS as `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1` (never `-j 2`), output redirected to a file outside the repository, NEVER piped through `tail`/`head`; every worker you spawn must be told the same. Leave no scratch files in the tree (and none outside it: use only paths inside the worktree for edits).
* TypeScript cannot be type-checked, linted or tested locally (npm/tsc/vitest are banned): read every changed `.ts` file twice for imports, `bigint`/`number` mixing, strict nullability and exhaustive switches.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits. (For file moves use a plain filesystem move and say so in the report;
the orchestrator stages them so git detects the renames.)

## Observable acceptance

For every allowlisted functional operation the report states, with code evidence, whether the server builds and uses it; the model, canonical layer and BOTH clients admit exactly the buildable subset and refuse the rest locally with a typed error before any send; a server-side test proves every admitted form builds and serves lookups through the real apply path and that every refused form is refused before any journal intent; the vector file's surviving cases are byte-identical; `cargo fmt`/`clippy --all-targets -- -D warnings` are clean for every touched crate (expected: `shamir-query-types`, `shamir-query-builder`, `shamir-client`, `shamir-db`); layout limits hold with counts reported. The final report lists files with folders, every widening, the evidence table, the decisions, and what is not covered.
