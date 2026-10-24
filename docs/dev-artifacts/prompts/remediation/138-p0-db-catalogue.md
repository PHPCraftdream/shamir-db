# P0 stage A4 — p0-db-catalogue

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Renames write the new durable record before removing the old one; catalogue persistence errors propagate; cascade drops report per-table errors; removing a member of a missing group fails without creating it.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-DB-093EAA95 — `rename_function_as` / `rename_validator_as` / `rename_function_folder_as` destroy the durable record *before* writing the new one (remove-before-write)

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs:419; crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:570
- Revalidation note: AGREE — OBSERVED: validator and function renames still remove the old durable row before saving its replacement, and folder rename removes all source keys before entering the destination-save loop.

### RV-DB-6540DC7B — Catalogue-persistence failures are swallowed (`warn!` + continue) across the DB/repo/table lifecycle, so multi-step mutations can return `Ok(())` half-migrated

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:226; crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:322
- Revalidation note: AGREE — OBSERVED: database and table renames still continue from a warn-only destination-save failure to old-row removal; catalogue WAL recovery does not make these separate operations atomic or conditional.

### RV-DB-42E3530E — Cascade-drop paths discard per-table drop errors with `let _ =` — not even a log line

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs:480; crates/shamir-engine/src/table/table_manager/admission/domain/barrier/admitted_ops.rs:68
- Revalidation note: AGREE — OBSERVED: repository and database cascades still discard table-cleanup Results and the admitted index sweep discards individual drop Results, although newer parent-removal guard errors do propagate.

### RV-DB-E301E1EF — `remove_group_member` on a nonexistent group *creates* a phantom group record (and the wire remove path lacks the existence guard the add path has)

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/system_store/identities.rs:207; crates/shamir-db/src/shamir_db/shamir_db/access/groups.rs:313
- Revalidation note: AGREE — OBSERVED: root authorization returns before group existence is checked, and removal of a missing numeric group defaults its fields and unconditionally calls save_group.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
