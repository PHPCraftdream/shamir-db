# P1 stage F11 — p1-db-catalogue

## Goal

Catalogue integrity: rename keeps schema, strict ACL reads, durable flush, boot. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F11): Repository rename keeps every declarative schema field; corrupt catalogue metadata fails closed (never default-open ACLs); namespace Create cannot replace another owner's private function; database/replication catalogue writes take the durable DDL flush; boot surfaces re-attach failures and does not overwrite colliding function meta; `drop_function_as` reports `existed` correctly; system-store failures are injected in tests.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-DB-B26124D9 — Successful repository rename discards declarative schema catalogue fields

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:661; crates/shamir-db/src/shamir_db/system_store/catalogue.rs:185; changed by: none
- R0 note (includes the fix site): OBSERVED: repository rename still rewrites each full child row using save_table, whose constructed record omits schema, schema_validator_id and schema_version; boot compilation still skips schema-less rows. Propagating write errors does not preserve these fields. Fix site: crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs, re-key the complete child record instead of rebuilding its restricted projection.

### RV-DB-DEC185C5 — Relaxed catalogue reads can turn corrupt metadata into default-open ACLs

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-db/src/shamir_db/system_store/catalogue.rs:292; crates/shamir-db/src/shamir_db/shamir_db/access/meta.rs:45; changed by: none
- R0 note (includes the fix site): OBSERVED: catalogue lookup consumes only result.records, while read execution skips malformed non-map rows and resource_meta maps absence to open metadata; propagated storage errors do not guard this successful-but-incomplete read. Fix site: crates/shamir-db/src/shamir_db/system_store/catalogue.rs must distinguish corrupt/incomplete metadata reads from genuine absence.

### RV-DB-BBA04407 — Namespace Create permits replacement of another owner's private function

- R0 verdict: `unverified`, risk `high`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:142; crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:252; changed by: none
- R0 note (includes the fix site): OBSERVED: namespace Create still permits replace=true and caller-owned metadata overwrite without target authorization; the server coarse gate still excludes ForEach bodies. Missing premise: the accepted cross-owner/builtin replacement and ownership-transfer policy. Fix-brief starting site: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs, encode that policy before artifact/owner replacement.

### RV-DB-0486E3F9 — `save_database` / `remove_database` (and all replication-catalogue writes) skip the "Durable DDL" flush; a test comment asserts they don't skip it

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/system_store/catalogue.rs:28; crates/shamir-db/src/shamir_db/system_store/catalogue.rs:46; changed by: none
- R0 note (includes the fix site): OBSERVED: database save still ends after interner persistence and removal after implicit commit, without an immediate history/WAL durability barrier; the default commit policy remains Buffered. Error propagation in #270 does not add that barrier. Fix site: crates/shamir-db/src/shamir_db/system_store/catalogue.rs, make database DDL acknowledgement enforce the promised durability tail.

### RV-DB-A7A29B02 — Boot path: repo re-attach failure is `warn!` + `continue`, and repo rows for unknown databases are skipped without any log

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/core.rs:370; crates/shamir-db/src/shamir_db/shamir_db/core.rs:409; changed by: none
- R0 note (includes the fix site): OBSERVED: attachment/recovery errors still propagate, preserving the prior fix, but a repository row whose parent database is absent still falls through without a diagnostic. Fix site: crates/shamir-db/src/shamir_db/shamir_db/core.rs, explicitly diagnose or reject missing-parent catalogue rows.

### RV-DB-D378C582 — Boot "skipping" a builtin-name-colliding function row still overwrites its `function_meta`, and the live artifact diverges from the catalogue after restart

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/core.rs:463; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:201; changed by: none
- R0 note (includes the fix site): OBSERVED: failed builtin-colliding registration still falls through to function_meta insertion, while effective authority independently reads the persisted replacement row; live replacement still accepts replace=true. Fix site: crates/shamir-db/src/shamir_db/shamir_db/core.rs must reconcile rejected catalogue artifacts with metadata and authority resolution, not merely add a continue.

### RV-DB-499FE365 — No error-path test injects a system-store failure into the lifecycle paths above

- R0 verdict: `fixed` — CLOSED, change nothing for it. OBSERVED: a write-boundary countdown now returns an injected SystemStore error before mutation; registered lifecycle tests arm it, invoke real rename/create/drop paths, and assert propagated errors plus relevant retained state. This closes the claimed absence of lifecycle SystemStore-failure injection, not a claim of exhaustive WAL/fsync fault coverage or passing tests.

### RV-DB-55B0A5D9 — `drop_function_as` returns `existed = false` while deleting a durable catalogue-only function

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:298; crates/shamir-db/src/shamir_db/execute/admin_meta/admin_function.rs:138; changed by: none
- R0 note (includes the fix site): OBSERVED: durable deletion now precedes live removal, but the returned boolean still reflects only live-registry removal; IF EXISTS still skips catalogue-only functions. Fix sites: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs and crates/shamir-db/src/shamir_db/execute/admin_meta/admin_function.rs, reconcile durable existence with result and early-exit semantics.

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
