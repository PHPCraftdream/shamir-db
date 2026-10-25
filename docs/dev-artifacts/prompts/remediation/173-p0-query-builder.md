# P0 stage C4 — p0-query-builder

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Duplicate aliases and unknown handles are errors; nested batch dependencies are accepted; ref collection matches the planner; `Doc::set` is fallible and does not round-trip per field; SELECT/HAVING refs reach the production dependency path.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-QUERY-BUILDER-63B52B5C — Re-registering an alias silently replaces the earlier op — a database operation vanishes

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/batch/batch_refs.rs:243; crates/shamir-query-builder/src/batch/batch_refs.rs:252
- Revalidation note: AGREE — OBSERVED: alias registration still ignores the replaced map entry and installs a new operation with a reset guard and replacement ordering edges.

### RV-QUERY-BUILDER-2FD16C76 — `Batch::after` and `Batch::when` silently no-op when the handle's alias is not registered in this batch

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/batch/batch_refs.rs:154; crates/shamir-query-builder/src/batch/runtime/handle.rs:11
- Revalidation note: AGREE — OBSERVED: after and when still discard lookup misses without recording an error, and handles contain only an alias rather than batch ownership.

### RV-QUERY-BUILDER-F3ED5A33 — `Batch::try_build` falsely rejects `sub_batch`/`for_each` entries whose inner batch has internal `$query` dependencies

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/batch/batch_refs.rs:491; crates/shamir-query-types/src/ops/batch/plan/planner.rs:353
- Revalidation note: AGREE — OBSERVED: builder fallback still recursively scans serialized container bodies against outer aliases, unlike the planner's bind-only and over-only outer dependency rules.

### RV-QUERY-BUILDER-224EF7B1 — `collect_query_refs` uses the loose pre-#641 marker rule, diverging from the planner's exact marker-map convention

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/batch/batch_refs.rs:272; crates/shamir-query-types/src/ops/batch/plan/planner.rs:427
- Revalidation note: AGREE — OBSERVED: builder discovery accepts a string $query in any map, while planner marker recognition requires the exact reserved-key shape, preserving false alias rejection for literal maps.

### RV-QUERY-BUILDER-20D9732A — `Doc::set` `.expect()`s the `FilterValue` -&gt; `QueryValue` msgpack round-trip in a public setter

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder/src/write/ops/doc.rs:43; crates/shamir-query-types/src/read/filter/filter_value.rs:46
- Revalidation note: AGREE — OBSERVED: the public setter still expects the MessagePack decode to succeed and accepts recursively nested FilterValue arrays without a local depth guard, preserving the named codec-failure panic path.

### RV-QUERY-BUILDER-7B79D840 — `Doc::set` does a full msgpack round-trip per field -- the crate's per-row hot loop

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder/src/write/ops/doc.rs:43; crates/shamir-query-types/src/read/filter/filter_value.rs:322
- Revalidation note: OBSERVED: Every set still serializes and deserializes its value instead of using the available literal converter, establishing avoidable work rather than measured runtime harm.

### RV-QUERY-BUILDER-E8D88CD7 — Accepted SELECT/HAVING query references are not supplied by the production dependency path

- Status at HEAD: `partially-fixed`, risk `medium`; evidence: crates/shamir-query-types/src/ops/batch/plan/planner.rs:288; crates/shamir-engine/src/query/batch/runner/query_runner/helpers.rs:35
- Revalidation note: AGREE — OBSERVED: the fixing diffs supply SELECT dependencies and execution bindings, but the current Read dependency arm still omits HAVING and result binding still requires a data-flow edge.

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
