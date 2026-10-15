# Round 10 remediation brief

## Scope

- Migrate integration-test helpers in `crates/shamir-db/tests/changefeed_e2e.rs` and `crates/shamir-db/tests/temporal_e2e.rs` from raw Tokio broadcast receiver annotations to the public `shamir_db::shamir_db::ChangelogReceiver` returned by `ShamirDb::subscribe_changelog`; preserve helper semantics and assertions.
- Ensure `crates/shamir-db/benches/engine_perf.rs` unwraps both `Result` and `Option` from the facade subscription so its with-subscriber benchmark holds an actual receiver.
- Preserve facade read errors in `crates/shamir-server/tests/repl_convergence_e2e.rs` while retaining its missing-repository polling behavior.
- Fix the two `clippy::match_result_ok` patterns in `crates/shamir-server/src/db_handler/repl_handler.rs` and `crates/shamir-server/src/replication/in_process.rs` by matching `Result` directly (`if let Ok(hook) = ...`).
- Derive the full set of changed facade changelog method names from the actual `ShamirDb` definitions in `changelog.rs` and `table_management.rs`. Search all workspace `crates/*/tests`, `benches`, and `examples`, including nested directories, for each name; inspect every hit and distinguish facade invocations from raw engine/repository methods. Migrate stale facade call sites, receiver annotations, and `Result<Option<_>>` handling only where required, without weakening failures/assertions or swapping facade calls for raw APIs.
- Keep prior unrelated clippy fixes intact: quarantine-cache needless return, ReceiverGates alias registry, and the `is_ok` assertion in gate tests.

## Constraints

- Do not run local builds, tests, clippy, rustc, nextest, cargo, or formatting. Parent will run final formatting/verification.
- Do not mutate Git state or commit. Edit files only; orchestrator commits.
- No design rewrite, dependencies/config/lockfile edits, or test removal. Existing-file growth must remain within the stated 1,000-line ratchet; new-folder source-entry limit is seven.
- Read the final diff and report the complete method-name search inventory, every matching file/site examined, and changed files with line references. Do not claim compilation/test success; no compile execution is permitted.

## Expected changes

Only the two targeted receiver helpers, the two `match_result_ok` expressions, plus any additional stale facade test/bench/example callsites proven by the exhaustive search and actual definitions.
