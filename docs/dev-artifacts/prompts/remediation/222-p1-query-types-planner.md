# P1 stage H7 — p1-query-types-planner

## Goal

Planner budgets, pagination overflow, tests and layout. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H7): ForEach products count against the parent budget; pagination cannot overflow and `page: 0` is defined; planner/error paths tested; vacuous tests replaced; depth boundary pinned; mod.rs and inline-test layout fixed.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-QUERY-TYPES-959AE4BE — ForEach sibling products are not accumulated into the documented parent operation budget

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-types/src/ops/batch/plan/planner.rs:141; crates/shamir-query-types/src/ops/batch/plan/planner.rs:157; changed by: none
- R0 note (includes the fix site): OBSERVED: Each literal ForEach product is checked separately against max_queries; whole-tree accumulation checks distinct max_total_queries/max_total_work limits. INFERRED: Two 30-unit siblings still pass max_queries=50 under ordinary other defaults, contrary to the documented parent-total composition. Fix site: ops/batch/plan/planner.rs, accumulate virtual units into the parent max_queries check.

### RV-QUERY-TYPES-B5653A74 — `Pagination::resolve` multiplication can overflow; `page: 0` silently behaves as page 1 while `current_page` echoes 0

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-query-types/src/read/clause/limit.rs:182; crates/shamir-query-types/src/read/clause/limit.rs:320; changed by: none
- R0 note (includes the fix site): OBSERVED: Saturating arithmetic and checked resolution retain PREV's overflow repair, but both resolution methods accept page zero as offset zero and compute echoes current_page=0. INFERRED: The remaining page-zero metadata inconsistency persists. Fix site: read/clause/limit.rs, consistently reject or normalize page zero across resolution and metadata.

### RV-QUERY-TYPES-B53EC5CE — Planner error paths, `PaginationInfo::compute`, `QueryReference::parse`, and `collect_required_access` are owned here but tested only in other crates

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-query-types/src/ops/batch/tests/plan/planner_tests.rs:729; crates/shamir-query-types/src/ops/batch/tests/plan/tree_visitor_tests.rs:161; changed by: df72cadc (#283)
- R0 note (includes the fix site): OBSERVED: Required-access assertions remain local, and #283 additionally adds an exact dependency-depth TooDeep oracle; searches still find no local PaginationInfo::compute or QueryReference::parse cases and no direct UnknownAlias/CircularDependency error oracles. Fix sites: read/tests/clause and ops/batch/tests/plan, add the remaining owner-local discriminating cases.

### RV-QUERY-TYPES-283D65CC — Bare-string FieldPath regression test never decodes a bare string

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:186; crates/shamir-query-types/src/read/filter/filter_enum.rs:397; changed by: none
- R0 note (includes the fix site): OBSERVED: The named test constructs a field vector and round-trips its canonical array encoding; it never supplies a bare string to the One branch. The depth-wrapper change does not alter this oracle. Fix site: read/filter/tests/filter_enum_tests.rs, decode an independent bare-string fixture.

### RV-QUERY-TYPES-952F5AB7 — Vacuous test: `fts_default_mode_is_and` asserts a value the test itself supplies

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:29; crates/shamir-query-types/src/read/filter/filter_enum.rs:162; changed by: none
- R0 note (includes the fix site): OBSERVED: The default-mode test explicitly supplies and and checks that supplied value, bypassing serde default_fts_mode. Fix site: read/filter/tests/filter_enum_tests.rs, deserialize an FTS fixture omitting mode.

### RV-QUERY-TYPES-3BFD1858 — `check_filter_depth` boundary (exactly `MAX_FILTER_DEPTH` passes, +1 fails) is not pinned

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-query-types/src/read/filter/filter_enum.rs:259; crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:213; changed by: none
- R0 note (includes the fix site): OBSERVED: The checker still rejects depth greater than 64; local cases use 100 Not wrappers or MAX_FILTER_DEPTH+10 value wrappers, not exact cap/cap-plus-one acceptance and rejection. Fix site: read/filter/tests/filter_enum_tests.rs, add discriminating 64/65-depth oracles.

### RV-QUERY-TYPES-A125F638 — Types defined inside mod.rs (re-export-only rule breach)

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/security/validator/mod.rs:11; crates/shamir-query-types/src/ops/call/mod.rs:32; changed by: none
- R0 note (includes the fix site): OBSERVED: validator/mod.rs defines WriteOp and ValidationError; call/mod.rs defines CallOp and default_repo. These still violate the manifest/re-export-only rule without an established runtime consequence. Fix sites: those two mod.rs files, relocate definitions into sibling implementation files.

### RV-QUERY-TYPES-98401770 — Inline `#[cfg(test)] mod tests { ... }` embedded in implementation files

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-query-types/src/read/core/query_record.rs:303; crates/shamir-query-types/src/ops/write/inserted_record.rs:135; changed by: none
- R0 note (includes the fix site): OBSERVED: Both implementation files retain inline cfg(test) modules despite the separate-test-directory rule; this establishes layout debt, not missing registration. Fix sites: read/core/query_record.rs and ops/write/inserted_record.rs, move the existing suites into registered sibling test files.

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
