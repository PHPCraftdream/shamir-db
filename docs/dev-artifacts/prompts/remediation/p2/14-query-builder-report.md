# P2-14 implementation report

Tests were not run locally; all behavioral assertions require CI execution. Contract tests for FE2003D1 and 320AD085 pin existing behavior and are expected to pass old code; they are not red fixes. Predicted old-code failures below were not observed locally. Paths below are relative to the repository root.

| Card | Status | Evidence |
|---|---|---|
| FE2003D1 | Implemented, CI pending | `crates/shamir-query-builder/src/read/val/filter_value.rs` documents representation limits. `lit_u64_order_range_and_membership_storage_contract` in `crates/shamir-engine-query/src/query/filter/tests/eval_tests/predicates/u64_big_filter_match_tests.rs` pins equality, reversed lexical versus numeric ordering, inclusive equal bounds, literal InSet and dynamic In on tree Big, tree string and raw lens for both specified values. Unsupported Big/lens range and membership are asserted false. This is contract coverage, not a behavior fix. |
| 8BEB53C0 | Implemented, CI pending | Typed Call and subscription scope handling in `crates/shamir-query-builder/src/batch/batch_refs.rs`; `subscription_scoped_refs_match_planner` covers Records, Keys, Batch and Call plus source-filter references; old codec fallback is expected to reject these inner references. Planner `crates/shamir-query-types/src/ops/batch/plan/planner.rs:369-390` uses a wildcard for Subscribe. `call_missing_reference_matches_planner` checks planner rejection (and is expected to pass old code). |
| 9E172C69 | Implemented, CI pending | `try_into_request` shares validation; `execute_owned` in `crates/shamir-sdk/src/api/db.rs` shares execution. `owned_request_matches_reusable_borrowed_request_and_moves_insert_payload` and `owned_request_preserves_deferred_error_precedence` pin bytes, reuse, actual Insert payload pointer and precedence. `execute_owned_maps_validation_error_like_borrowed_without_host_call` compares error kind and exact message through both public SDK calls. Old API lacks consuming method. SDK host execution requires CI. |
| MACROS-02993B77 | Implemented, CI pending | Facade wrappers and complete public docs in `crates/shamir-query-builder/src/lib.rs`; renamed-only `crates/shamir-query-builder-macros/tests/alias_consumer.rs` would fail compilation on old absolute paths. |
| MACROS-13A0D25A | Incomplete | Spanned token tests in `crates/shamir-query-builder-macros/src/tests/diagnostics_tests.rs`; no compiler diagnostic underline fixture was found or added, so actual rustc underline remains unverified. |
| 2A648617 | Implemented | `Batch::switch` documents quadratic lowering and separate wire-design decision in `crates/shamir-query-builder/src/batch/batch_refs.rs`. |
| 320AD085 | Implemented, CI pending | `switch_without_cases_executes_default_exactly_once` in `crates/shamir-engine/src/query/batch/tests/executor/ops/when_skip_tests.rs` executes the default insert then verifies exactly one row. |
| 0E30EB25 | Implemented, review pending | `read/filter/leaf.rs`, `write/ops/insert.rs`, `write/mod.rs`, and `batch/batch.rs` correct identified stale references. |
| 202E00CD | Implemented | Private sibling imports retained public manifest re-exports in `read/select/select_item.rs` and `write/ops/update.rs`. |
| 21560B4A | Implemented | Qualified import guidance in `read/val/mod.rs`. |
| 7A0F3500 | Incomplete layout | Independent builders split, public routes retained, but owner files currently reside under `ddl/schema/config/schema_builders` and `ddl/schema/config/access_builders`. No deletion-capable edit tool is available; removing tracked original files using the shell is prohibited and the layout gate reads tracked files. Natural owner directories would exceed seven entries while shims remain. Requires orchestrator removal/index coordination and a proper layout decision, not silently accepted. |
| A548E006 | Implemented | Four macro definitions in `src/macros/{bind,doc,subscribe,vals}.rs`, wired in manifest. |
| C861C539 | Implemented with legacy placeholder | `src/read/cursor.rs` points at `cursor/tests/mod.rs`, which registers `cursor_tests.rs`. Tracked `cursor/tests.rs` is empty solely because layout checker reads tracked files; orchestrator may remove it when git index changes are permitted. |
| DE20E96B | Implemented, CI pending | Exactly five derives and exhaustive message/source compatibility tests in `src/wire/response/tests/error_compatibility_tests.rs`. Convention cleanup, not an error-handling defect. Root workspace has no thiserror dependency entry; builder pins existing locked 2.0.21 directly. |
| E2602921 | Implemented | Hoisted identified builder production/test imports; no indented use statements found in builder and macro source. |
| MACROS-1AF72B7B | Implemented, CI pending | Helper visibility and shared field emitter in macro lowering modules. |
| MACROS-A690BEA8 | Implemented, CI pending | Predicate lowering borrows identifier and argument storage in `filter_lower.rs`. |
| MACROS-EC1B86B3 | Implemented, CI pending | Parsing changes and `rv_query_builder_macros_ec1b86b3_clause_keywords_in_expressions` cover keyword disambiguation. |
| MACROS-83FE5F55 | Implemented | Public `q!` docs distinguish six forms and fallible writes. |
| MACROS-E78E1666 | Implemented | Public docs list 17 predicate calls and direct vector-option users to builder APIs. |

## Gates

Most recent scoped formatting, format check, check and clippy for builder, builder-macros, engine-query and engine: exit 0. `python -I scripts/ci/check_layout.py`: exit 0; it ignores untracked files. Earlier dependent check for client, server, db, engine and engine-query: exit 0. Earlier SDK check and clippy: exit 0. Stage 2 earlier check failures (import/module wiring) and layout failure were repaired before these successful gates. 
## Final quality gates and directory counts

Builder and SDK: `cargo fmt`, `cargo fmt -- --check`, `cargo check --all-targets`, and `cargo clippy --all-targets -- -D warnings` each exited 0. Layout exited 0. Direct entries including untracked: `ddl/schema` 7, `ddl/policy` 7, `ddl/schema/config` 5, `config/access_builders` 3, its `groups` 6 and `resources` 5, `config/schema_builders` 6, `src/macros` 6, `src/macros/tests` 6, `src/read/cursor` 2, `src/read/cursor/tests` 2, `src/wire/response/tests` 3, and `shamir-query-builder-macros/tests` 1. The pre-existing prompts directory has 20 entries including this report. No compiler fixture harness (`trybuild` or `compiletest_rs`) was found in the repository; exact rustc underlines remain unverified.
