# P1 stage F12 — p1-db-ddl-fk

## Goal

DDL serialization and FK guards on corrupt rows. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F12): Table DDL and database/repo DROP/RENAME are serialized with the create locks; FK guards fail on decode-corrupt parent rows instead of skipping them; FK rename/drop checks are not O(tables^2).

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-DB-30AC7F3A — FK guards silently skip decode-corrupt parent rows — the RESTRICT/refuse guarantee silently evaporates

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:251; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:169; changed by: none
- R0 note (includes the fix site): OBSERVED: both reverse-FK guards continue on sibling catalogue Err/None and non-list schemas; successful destructive DDL therefore does not establish reference absence after a failed sibling read. Fix sites: crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs and crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs, fail closed on unreadable reference metadata.

### RV-DB-370D8AC1 — Table-level DDL (CREATE/DROP/RENAME TABLE) has unserialized check-then-act races; the race is acknowledged by a `debug_assert!`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:205; crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:268; changed by: none
- R0 note (includes the fix site): OBSERVED: facade existence checks and awaited catalogue reads precede engine admission; a completed competing drop can remove the config before rename acquires admission, returning false into the facade debug assertion. Fix site: crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs, serialize the full namespace check/mutation/catalogue sequence.

### RV-DB-C57EA68C — DROP/RENAME of databases and repos bypass the #546 create locks — asymmetric serialization axes

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:78; crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:396; changed by: 3116812e (#270)
- R0 note (includes the fix site): OBSERVED: database creation now shares replication_control with database drop/rename, closing that mutation/publication race; repository creation still takes only its separate create lock and add_repo_as_with_durability does not acquire replication_control, unlike repo drop/rename. Fix site: crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs, interlock repository creation with the same namespace lifecycle guard.

### RV-DB-72E8717C — DDL FK guards re-scan the table catalogue once per sibling table — O(tables²) per rename/drop

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:246; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:164; changed by: none
- R0 note (includes the fix site): OBSERVED: rename and drop still call load_table_record separately for each sibling, and that helper performs a filtered read of the unindexed global catalogue. Structural cost remains siblings multiplied by scanned catalogue/history size. Fix sites: these two guard files, use a shared catalogue snapshot protected through mutation.

### RV-ENGINE-BEC5D158 — FK parent-value scans silently skip rows that fail to decode

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-engine/src/query/batch/fk/fk_restrict.rs:308; crates/shamir-types/src/record_view/lens/view.rs:41; changed by: none
- R0 note (includes the fix site): OBSERVED: both-decoder failures still error, but RecordView construction validates only the header and extraction wraps scalar_at absence in Ok; malformed field access and classifier decode failures still become absence/nonmatch. Numeric equality and completed-span changes do not close this path. Fix site: crates/shamir-engine/src/query/batch/fk/fk_restrict.rs needs fallible extraction/classification that preserves malformed-body errors.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
