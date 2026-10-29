# P2-13 - shamir-query-types: conversion loss, equality, serializer/deserializer parity, pagination, planner allocations, cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 18 findings below in one coherent change set for shamir-query-types (effort units 41: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. Preserve the RU7 decimal-string large-integer projection; reuse the H5 decoder instead of adding another conversion oracle.

## Scope

Crates: `shamir-query-types`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: 182C703E, 4F767AA5, 5F9D5468, F05184D8, CC015A3C.

- (none yet)

## Findings (ordered by risk, then size)

### RV-QUERY-TYPES-0A0D9092 - Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/batch/ops/query_entry.rs:94-122; crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:712-716; crates/shamir-engine/src/query/batch/control/authorized.rs:109-114
- Recipe: Compute reusable request classification, repository and access metadata once at the execution boundary.
- Note: Shared iterative traversal and gate deduplication are present; separate passes remain.
- Second pass: Shared iterative traversal and deduplicated checks exist; request classification, repository and access metadata still use separate passes.

### RV-QUERY-TYPES-182C703E - thiserror convention deviation: hand-rolled Display/Error impls and `Result<(), String>` public APIs

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/batch/core/batch_error.rs:300; crates/shamir-query-types/src/admin/types/db/retention.rs:41; crates/shamir-query-types/src/read/filter/filter_enum.rs:234
- Recipe: Migrate formatting to thiserror and adopt typed validation errors under an explicit API compatibility plan.
- Note: Manual errors and public String error APIs remain.

### RV-QUERY-TYPES-4F767AA5 - `From<QueryValue> for FilterValue` silently substitutes `Null` in release builds

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-types/src/read/filter/filter_value.rs:180-201; crates/shamir-query-types/src/read/filter_value_decoder.rs:95-102
- Recipe: Introduce fallible QueryValue conversion, migrate callers and reject unsupported maps without Null substitution.
- Note: The strict H5 decoder does not remove the infallible conversion fallback.
- Second pass: Strict map decoding still feeds the infallible conversion's debug-panic or release-Null fallback; migrate to a fallible public conversion.

### RV-QUERY-TYPES-3D922433 - `Pagination::eq` (`After`) — two msgpack encodes per equality comparison

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/read/clause/limit.rs:84-88,123,130-132
- Recipe: Implement allocation-free encoded-value equivalence with numeric, float and map-order regression oracles.
- Note: Two encodes remain after matching limit and ID; hot use is unestablished.

### RV-QUERY-TYPES-4E49851D - Plan-time marker decode pays a msgpack round-trip per `$query`/`$fn`/`$cond`/`$expr` marker

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/batch/plan/planner.rs:451-471
- Recipe: Reuse buffered-value deserialization with the authoritative FilterValue decoder, preserving fallback and Cond validation.
- Note: Depth guarding was added but marker codec round-trips remain.

### RV-QUERY-TYPES-5F9D5468 - QueryRecord PartialEq violates transitivity through the opaque-row Null sentinel

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-types/src/read/core/query_record.rs:195,289-296
- Recipe: Make opaque-versus-nonopaque comparisons false and add a three-value transitivity regression.
- Note: Null normalization still conflicts with bytewise opaque equality.

### RV-QUERY-TYPES-C000A440 - `Pagination`'s `PartialEq` can panic via `key_bytes`' `expect`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/read/clause/limit.rs:123,132; rmp-serde 1.3.1 src/encode.rs:1264 (registry source)
- Recipe: Replace encoding-based equality with allocation-free wire-equivalent comparison.
- Note: Upgraded: encoder allocation failure can reach expect; not a proven depth-limit panic.

### RV-QUERY-TYPES-D1C4DEAB - Batch planner — redundant alias-set clone and repeated String re-cloning through the plan

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/batch/plan/planner.rs:185-186,224,248,260-261
- Recipe: Use the query map for alias membership and remove redundant ownership while preserving provenance and order.
- Note: Repeated alias ownership remains; per-iteration planning is not alleged.

### RV-QUERY-TYPES-F05184D8 - `InsertedRecord` with a non-map `fields` and no `id` serializes to a shape its own deserializer rejects

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/write/inserted_record.rs:70-72,102
- Recipe: Accept existing non-map wire values in the decoder and add scalar/list round-trip tests.
- Note: Serializer output remains incompatible with the map-only decoder.

### RV-QUERY-TYPES-02448485 - `QueryRecord::get_value_{i64,u64,bool}` — deep-clones the whole `Inserted` record per scalar lookup

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/read/core/query_record.rs:192,223,249,264,278
- Recipe: Borrow Inserted fields directly in scalar accessors, matching the string accessor.
- Note: Whole-record clones still precede scalar lookup.
- Second pass: Inserted scalar accessors clone the entire fields tree through get_value_owned; direct borrowing matches the existing string accessor.

### RV-QUERY-TYPES-CC015A3C - Per-construction `"main"` String allocations

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/table_ref.rs:23; crates/shamir-query-types/src/ops/call/call_op.rs:7-8
- Recipe: Decide whether measured cost justifies changing public String ownership or explicitly accept default-name allocations.
- Note: Owned main allocations remain without demonstrated bottleneck.

### RV-QUERY-TYPES-0172CFB1 - Mid-function `use` statements in implementation files (imports-at-top breach)

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/batch/plan/planner.rs:433,619,752; crates/shamir-query-types/src/ops/write/inserted_record.rs:96; crates/shamir-query-types/src/read/filter/filter_enum.rs:391
- Recipe: Hoist function-local implementation imports while retaining genuine module-header and cfg exceptions.
- Note: Downgraded: named import-policy class remains despite changed HMAC code.

### RV-QUERY-TYPES-4B4C84BF - Mid-function `use` statements in standalone test files

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/batch/tests/plan/planner_tests.rs:459; crates/shamir-query-types/src/wire/tests/repl_tests.rs:22; crates/shamir-query-types/src/read/tests/core/query_record_tests.rs:78
- Recipe: Hoist standalone test imports to file headers without changing test logic.
- Note: Downgraded: organization debt, not missing tests.

### RV-QUERY-TYPES-881BC94F - Doc/wire mismatches and stale references

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/admin/types/ddl/validator_ops.rs:63-78; crates/shamir-query-types/src/read/clause/limit.rs:346; crates/shamir-query-types/src/wire/db_message.rs:380-382
- Recipe: Correct wire examples, stale method names and default documentation against current DTOs and HMAC v2.
- Note: Concrete doc/wire mismatches survive the remediation waves.

### RV-QUERY-TYPES-9223F400 - `is_false` helper defined four times with three visibilities and two referencing conventions

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/admin/types/db/db_ops.rs:6; crates/shamir-query-types/src/admin/types/db/schema_ops.rs:222; crates/shamir-query-types/src/admin/types/repl/repl_ops.rs:47; crates/shamir-query-types/src/read/core/read_query.rs:53
- Recipe: Consolidate internal helper uses while retaining the public schema helper as a forwarding compatibility wrapper.
- Note: Four equivalent helpers persist without demonstrated wire divergence.

### RV-QUERY-TYPES-D687DBA3 - `FieldPath` type alias defined in `filter/mod.rs`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/read/filter/mod.rs:24
- Recipe: Move FieldPath into a sibling definition file and preserve its existing public re-export.
- Note: Downgraded: manifest-only rule remains violated.

### RV-QUERY-TYPES-BFFB2B3A - `InsertedRecord::serialize` — per-record `Vec` collect + sort + base58, contradicting the "allocation-free" module claim

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/write/inserted_record.rs:1,32,39-40
- Recipe: Narrow allocation-free wording to the actual no-intermediate-map guarantee.
- Note: Downgraded: allocations are present but harmful cost is unmeasured.

### RV-QUERY-TYPES-D319B6E3 - `InsertOp` carries two parallel record channels with unspecified result ordering

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-query-types/src/ops/write/types.rs:85-105; crates/shamir-engine/src/table/write/insert.rs:420-444
- Recipe: Document existing values-first then idmsgpack ordering on InsertOp.
- Note: Runtime ordering is deterministic; DTO documentation still omits it.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- Cards marked `OWNER DECISION` are implemented only if the "Decisions" section below contains the answer; otherwise skip them and say so in the final report.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-query-types -- --check`; `cargo clippy -p shamir-query-types --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
