# P1 stage F13 — p1-db-errors-perf

## Goal

DB error taxonomy, ambient interner authz, catalogue scans. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F13): The ambient interner delta requires Store-level authorization; serialization failures are errors, not `Null`; malformed client input is not `Internal`; no panics in library paths; wire error codes come from typed errors; dead `api` shim removed; ACL and catalogue lookups are keyed, not full scans; nits applied.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-DB-4D93F623 — Ambient interner delta exposes any repo's field-name dictionary without Store-level authorization

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/execute/core/ambient_interner.rs:30; crates/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:122; changed by: none
- R0 note (includes the fix site): OBSERVED: attachment accepts arbitrary advertised repo epochs without an actor or Store Read check; batch authorization checks database visibility and operation paths, not these advertised repositories. An empty authorized batch still reaches attachment. Fix site: crates/shamir-db/src/shamir_db/execute/core/ambient_interner.rs, authorize each requested repository before exposing its dictionary.

### RV-DB-3FBFC9A8 — `to_qv` converts serialization failure into `QueryValue::Null` inside Ok responses

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/execute/core/helpers.rs:75; crates/shamir-db/src/shamir_db/execute/admin_db/admin_retention.rs:240; changed by: none
- R0 note (includes the fix site): OBSERVED: either codec error still becomes Null, and ChangesSince maps every event through this helper before returning success; no currently failing event DTO was established. Fix site: crates/shamir-db/src/shamir_db/execute/core/helpers.rs, propagate conversion errors through response callers rather than substituting Null.

### RV-DB-48DA8907 — Malformed client input mapped to `DbError::Internal` in `get_ddl_op_status`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/shamir_db/core.rs:948; crates/shamir-db/src/shamir_db/shamir_db/core.rs:956; changed by: none
- R0 note (includes the fix site): OBSERVED: malformed RecordId input and all table-resolution failures are still wrapped in Internal, with no classification-preserving guard. Fix site: crates/shamir-db/src/shamir_db/shamir_db/core.rs, distinguish invalid input and preserve underlying resolution error categories.

### RV-DB-33CEC62D — Panic paths in library code: `.expect("interner touch_ind")` and `.unwrap()` on `SystemTime`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/execute/core/helpers.rs:61; crates/shamir-types/src/core/interner/interner.rs:138; changed by: none
- R0 note (includes the fix site): OBSERVED: pre-UNIX_EPOCH time still reaches unwrap in successful DDL response construction; touch_ind still has only Ok return branches, refuting the alleged returned-error panic independently. Fix site: crates/shamir-db/src/shamir_db/execute/core/helpers.rs, handle pre-epoch time explicitly.

### RV-DB-FA80A7B8 — Wire error-code classification by substring on the stringified `PortError`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_users_roles.rs:146; crates/shamir-db/src/shamir_db/gateway/ports.rs:29; changed by: none
- R0 note (includes the fix site): OBSERVED: grant/revoke still classify boxed errors by the substring user not found; the current directory emits that wording, but the port guarantees no typed classification. Fix site: crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_users_roles.rs needs a port-supported stable discriminator instead of message matching.

### RV-DB-49FED152 — Dead, exported `api::{Command, Request, Response}` wire shim that no server speaks

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/lib.rs:26; crates/shamir-db/src/api/types.rs:22; changed by: none
- R0 note (includes the fix site): OBSERVED: the public legacy DTO module retains client/server protocol prose although its README explicitly disclaims production use; workspace references found only its own tests. This remains API/documentation debt, not an active transport mismatch. Fix site: crates/shamir-db/src/api/types.rs, align public documentation with the compatibility-only contract.

### RV-DB-C58706F9 — ACL gate runs full catalogue scans per ancestor per op — O(ops × ancestors × catalogue) per request

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-engine/src/query/batch/control/authorized.rs:109; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:56; changed by: none
- R0 note (includes the fix site): OBSERVED: authorization deduplicates action/path pairs, but distinct targets still reload common ancestors through filtered reads of unindexed system tables. Fix site: crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs needs request-scoped ancestor reuse or genuinely keyed metadata resolution.

### RV-DB-F19EE56C — False "O(1) point lookup" comments encode a scan-based cost model (and hide an O(N²) introspection path)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:385; crates/shamir-db/src/shamir_db/system_store/identities.rs:84; changed by: none
- R0 note (includes the fix site): OBSERVED: listing still performs one filtered, unindexed catalogue read per live function and explicitly calls each lookup O(1); persistence/offloading changes do not alter this loop. Fix site: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs should eliminate repeated catalogue scans and correct the cost claim.

### RV-DB-23A03103 — `SystemStore::load_repository_record` full-catalogue scan + linear filter for a keyed lookup

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/system_store/catalogue.rs:131; crates/shamir-db/src/shamir_db/system_store/catalogue.rs:152; changed by: none
- R0 note (includes the fix site): OBSERVED: load_repository_record still materializes load_repositories and linearly matches both names; rename_repo_as still calls it. The separate filtered lookup does not replace this path. Fix site: crates/shamir-db/src/shamir_db/system_store/catalogue.rs, replace the full-materialization lookup without hiding corruption diagnostics.

### RV-DB-5977A782 — `resolve_group_id(GroupRef::Name)` is a full `load_groups()` scan per call

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-db/src/shamir_db/shamir_db/access/groups.rs:360; crates/shamir-db/src/shamir_db/shamir_db/access/groups.rs:384; changed by: none
- R0 note (includes the fix site): OBSERVED: the Name arm still loads every group and searches linearly on every call; the cost is explicitly documented, and new membership existence guards do not change it. Fix site: crates/shamir-db/src/shamir_db/shamir_db/access/groups.rs, resolve names through an indexed/keyed catalogue path.

### RV-DB-9257B8E1 — Nits

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/execute/core/db_tx.rs:159; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:835; changed by: none
- R0 note (includes the fix site): OBSERVED: unknown isolation remains rejected, preserving the prior fix; missing-rule removal still sets removed=false yet recompiles, increments schema_version and persists. Fix site: crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs, decide and enforce the no-op removal/versioning policy before mutation.

### RV-DB-FF77D3CB — Nits

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-db/src/shamir_db/execute/core/helpers.rs:61; crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:89; changed by: none
- R0 note (includes the fix site): OBSERVED: pre-epoch time still reaches unwrap, gateway errors still use Debug strings, the codec-demo main remains, and the ports introduction remains malformed; no production-server identity is attributed to the demo. Fix sites: crates/shamir-db/src/shamir_db/execute/core/helpers.rs and the other named files retain the respective nits.

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
