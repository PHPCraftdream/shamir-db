# P2-04 - shamir-engine: numeric membership parity, schema one_of, cache/token costs, cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 13 findings below in one coherent change set for shamir-engine (effort units 32: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. Do not edit pre_commit/prelock.rs (owned by P2-02).

## Scope

Crates: `shamir-engine`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: 3425FDE6.

- (none yet)

## Findings (ordered by risk, then size)

### RV-ENGINE-7CA0C179 - ContainsAllSet's seen-index optimization loses coercing numeric membership

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine-query/src/query/filter/evaluation/filter_node/membership.rs:292-304,364-378; crates/shamir-engine-query/src/query/filter/evaluation/filter_node.rs:603-613
- Recipe: Mark every coercion-equivalent requirement and add literal/dynamic parity regressions for both bitmask sizes.
- Note: Exact-first single-index marking still loses equivalent mixed numeric requirements.
- Second pass: Both mask paths mark only one exact-first required index; coercion-equivalent mixed numeric requirements remain unseen.

### RV-ENGINE-3425FDE6 - Engine boundary performs no authorization — enforcement is a single upstream wrapper

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-engine/src/query/batch/control/authorized.rs:57,99-116; crates/shamir-engine/src/query/batch/exec/batch_execute.rs:91-96; crates/shamir-engine/src/db_instance/db_instance.rs:99
- Recipe: Ratify and document trusted embedding APIs versus authorization-enforced batch execution.
- Note: Normal execution requires authorization; raw embedding remains an explicit trust boundary.
- Second pass: Normal execution requires authorization; remaining raw APIs are an explicit embedding-policy boundary, not a demonstrated server bypass.

### RV-ENGINE-3C71057D - FK exists_in_table fallback: full parent-table scan per child record

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`.
- Evidence at `7f04e096`: crates/shamir-engine/src/validator/engine_bound/validator_db.rs:291-304,328-360
- Recipe: Reuse batch-scoped parent membership while preserving staged transaction visibility.
- Note: Warning and indexed bypass exist; unindexed per-child parent scans remain.
- Second pass: Indexed staging-aware bypass exists; unindexed existence checks independently scan parents per child.

### RV-ENGINE-62C6F2B1 - Content-key cache remediation introduces per-evaluation formatting allocation and AST traversal

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine-query/src/query/filter/caches/cond_cache.rs:56-57,98-99; crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:337
- Recipe: Use stable query-plan-local cache slots instead of formatting content on each evaluation.
- Note: Cache hits still allocate and traverse Debug representations.

### RV-ENGINE-95ECC2FE - Precomputed one_of rejects allowed NaN values accepted by the original equality contract

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine-validator/src/validator/schema/rules/one_of_set.rs:80-83; crates/shamir-types/src/types/value.rs:381-386
- Recipe: Preserve QueryValue NaN equality in precomputed membership and add optimized/fallback parity tests.
- Note: Primitive float equality still differs from the existing enum equality contract.

### RV-ENGINE-B442D16F - Shadow drain applies entries via per-entry set/remove round-trips

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine/src/db_instance/migration/coordinator.rs:400-422
- Recipe: Consume owned shadow pages and apply ordered mixed batches without cloning Put payloads.
- Note: Ordered adjacent-run batching exists; remaining costs do not establish corruption.
- Second pass: Adjacent-run batching preserves ordering, but borrowed pages force Put payload clones and alternating kinds fragment batches.

### RV-ENGINE-BE59E793 - doctor::repair deep-clones the materialized table per index rebuild

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine/src/table/table_manager/diagnostics/doctor.rs:635-660
- Recipe: Provide streaming or borrowed unique-index backfill under the existing repair barrier.
- Note: Regular indexes stream; unique indexes still clone the materialized table sequentially.
- Second pass: Regular backfills stream; unique backfills still clone the materialized table once per definition sequentially.

### RV-ENGINE-BE70B015 - one_of rule: linear Vec::contains with materializing clone per record

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine-validator/src/validator/schema/rules/one_of_set.rs:73-90; crates/shamir-engine-validator/src/validator/schema/rules/field_rule.rs:340-350
- Recipe: Preserve equality and introduce hashed residual membership plus borrowed probes where available.
- Note: Common scalar probes improved; residual and legacy scans remain.
- Second pass: Borrowed scalar hashing exists; residual and legacy scans remain and equality must be preserved.

### RV-ENGINE-CEBE3101 - read_history issues one awaited history_of per matched record

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `L/code`.
- Evidence at `7f04e096`: crates/shamir-engine/src/table/read/planning/read_temporal.rs:684-698
- Recipe: Add true batched history lookup preserving ID ordering and deterministic error selection.
- Note: Chunk concurrency exists; one independent history lookup per ID remains.
- Second pass: Chunk concurrency preserves ordered errors but still performs one independent history lookup per ID.

### RV-ENGINE-06B39B8F - table_token re-derives a hash of the immutable table name

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:132-133; crates/shamir-engine/src/table/table_manager.rs:68
- Recipe: Compute and store the existing table token at construction without changing its hash algorithm.
- Note: Immutable names are still hashed on every token call.
- Second pass: Reachable transaction calls still hash immutable names; cache across constructors and cloning while preserving the existing hash algorithm.

### RV-ENGINE-6CCD199D - Grouped per-row small-allocation nits

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:335-345; crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:132-133; crates/shamir-engine-query/src/query/read/projection/select_projection.rs:338,376
- Recipe: Consolidate token caching with 06B39B8F and retire fixed path-clone claims without changing owned outputs.
- Note: FieldRef paths are borrowed; token rehashing and owned output-key clones remain.
- Second pass: Cached paths are borrowed; consolidate token caching while retaining clones needed for owned output keys.

### RV-ENGINE-931318A1 - Per-record/per-rule small allocations across schema validation

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine-validator/src/validator/schema/schema_validator.rs:146; crates/shamir-engine-validator/src/validator/schema/rules/cross_field.rs:83; crates/shamir-engine-validator/src/validator/execution/record_fields.rs:130-139
- Recipe: Use inline cross-field paths and retain borrowed cached ID snapshots through probes.
- Note: SmallVec helps common paths; cross-field collection and cached Vec cloning remain.
- Second pass: SmallVec improves main paths; cross-field Vec collection and cached ID cloning remain.

### RV-ENGINE-D0CF9DFA - FkReverseCache::get_or_build_by_parent — unbounded CAS-loss retry while holding build_lock

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-engine/src/repo/instance/support/catalog/fk_reverse_cache.rs:349-361,379-392
- Recipe: Release and reacquire the build lock between failed publication attempts and recheck cached state.
- Note: Snapshot publication improved, but unbounded retries retain the single-flight lock.
- Second pass: CAS-loss retries still retain the single-flight lock indefinitely; release and recheck between attempts.

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

`cargo fmt -p shamir-engine -- --check`; `cargo clippy -p shamir-engine --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
