# TOKEN-T7 - end-to-end matrix of token channels, rights, revocation, races and budgets (server crate)

Base: T2-T6, AUTH1/AUTH2, T3b tails are in this branch. The design is `docs/dev-artifacts/design/token-api-rest-ws.md`, section 15 "Test plan" (integration part); the protocol specs are in `docs/guide-docs/client-server-protocol-spec/` (`TOKEN_API.md`, `TRANSPORT_WS.md`, `SECURITY_MODEL.md`). CLAUDE.md applies (layout: at most 7 entries per folder, 1000 lines per file; one primary export per file; tests only through the project conventions; queries only through `shamir-query-builder`).

## Goal

Every integration item of design section 15 is covered by a real-socket test in `crates/shamir-server/tests/e2e/` (loopback, OS ephemeral ports, Windows and Linux). Production code is NOT changed by this task: a missing observability seam or a contradiction between a test and the server stops that item and goes into the report with evidence (never weaken an assertion to match a bug).

## Step 1 - coverage audit (do it first, write it down)

Build a table in `docs/dev-artifacts/audits/2026-11-03-token-t7/DESIGN.md`: one row per cell of the design section 15 integration list (matrix channels 1-4 x cases; cross-listener replay; privilege sweep; subscription after revoke; race; accounting; budget; issuance; channels; log redaction; restore bump; subscription push and cursors through a token session; `not_supported_over_rest`), with the existing test that covers it (`file::test_name`, found under `crates/shamir-server/tests/e2e/{http_rest,http_ws,http_login}/`, `crates/shamir-server/src/**/tests/`, `crates/shamir-transport-http`) or `GAP`. Only GAP rows are implemented; do not duplicate a covered cell.

## Step 2 - implement the gaps

New tests go in a new folder `crates/shamir-server/tests/e2e/token_matrix/` (`mod.rs` is a manifest; topical files such as `channel_matrix.rs`, `cross_listener.rs`, `privilege_sweep.rs`, `revoke_race.rs`, `accounting_budget.rs`, `issuance.rs`), registered in `crates/shamir-server/tests/e2e/main.rs` with `#[path = "token_matrix/mod.rs"] mod token_matrix;` in the existing alphabetical position (the e2e folder then has 7 entries - do not add more). Reuse the fixtures of `http_rest/common.rs` and `http_ws/common.rs` (`seed_owner`, listener start helpers, grants: a token session is a non-superuser principal that needs grants on `db://default` mode 0o757 and `default/main` 0o711; interactive transaction commit authorizes Write on the database); if a helper must be shared, move it to a shared support file instead of copying. Rules for the tests:
- no `sleep`-based waiting; wait on observable state with a bounded deadline, use the lifecycle seams the existing e2e files use; every network wait has an explicit timeout;
- the failure limiter of the listener (2 failures/s per subnet during the first 60 s, then 10) breaks tests that make many consecutive failing attempts: use distinct source behavior or the existing test knob the http_rest/http_login e2e files use; do not raise production limits;
- privilege sweep: enumerate admin `DbRequest` and `BatchOp` variants through the scope classifier's own exhaustive lists (do not hand-copy names that can drift); superuser-owned and replicator-owned tokens must be denied for every admin op, `Repl`, `ChangePassword*`, `List::{Databases,Users}`, `AccessTree`;
- race: revoke while requests are in flight; no request is authorized after the revoke call returned;
- accounting: 16 active token sockets of one owner do not block that owner's SCRAM login; budget: pre-auth body flood stays under the global body budget.

## Rules

- NO LOCAL COMPILATION OR TESTS: the only commands allowed are `cargo fmt -p shamir-server` (and `-- --check`), and `python -I scripts/ci/check_layout.py`. No `cargo check/clippy/build/test`, no rustc, no npm/tsc/vitest, no server start. A child that runs them violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt. Do not edit `Cargo.toml`, `package.json`, lockfiles or workflows; a new dependency means stop and report.
- Because nothing is compiled locally, finish with a compiler-style read of every file you wrote: imports used and not duplicated, privacy of every path (`pub(crate)` / `pub` across `#[path]` modules), exhaustive `match`, struct literals with all fields, `rustfmt` alphabetical `mod` order, clippy classes (`type_complexity`, `useless_vec`, `needless_borrows_for_generic_args`, `result_large_err`), unused imports in test targets.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. No machine-local absolute paths in any file you write.
- Children only edit files, are awaited, and every touched file is re-read as the compiler would. Never finish with a child outstanding.
- Time: run `date` first; stop editing at the time in the run file; report per row DONE / PARTIAL / BLOCKED with `file:line` and the list of statements you could not verify, in the audit file above.
