# P34-06 - shamir-db + shamir-engine(+query, validator): catalogue lookups, boot diagnostics, builder migration, pattern contract, FK membership, access-gate allocations

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Replace full-catalogue scans by keyed lookups where the corruption semantics allow, make boot reconstruction failures visible, move the facade onto the query builder, close the validator gap behind the infallible pattern adapters, share FK parent membership per batch, drop per-evaluation formatting from the condition cache, and remove the remaining allocations on the access-gate path.

This is one change set for shamir-db, shamir-engine, shamir-engine-query, shamir-engine-validator: 11 active findings and 2 P2 carry-overs, effort units 30 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-db`, `shamir-engine`, `shamir-engine-query`, `shamir-engine-validator`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-types/src/core/interner/interner.rs` (39178D2D)
- `crates/shamir-types/src/entries/resource_path_ref.rs` (A7B1FCEC)

## Acceptance

- Every facade request built by the builder equals the old hand-written request field for field (one equality test per call site); then shamir-query-builder is a production dependency of shamir-db (C2DCAC96).
- Boot logs one warning per skipped repository with identity, backend and reason (A7A29B02-FACET); a missing group name does not scan the catalogue (5977A782); keyed repository lookup keeps the corruption errors (23A03103).
- Invalid regex/like patterns are rejected at validation time (2A18B647); FK parent membership is reused per batch and still sees transaction-staged parents (3C71057D); condition-cache hits no longer format the AST (62C6F2B1).
- Cost-discriminating regressions (counter or seam) for 39178D2D and 3EB43AC7 and a deep-chain allocation oracle for A7B1FCEC.

## Coordination with other tasks

Wave 2; starts after P34-01, P34-03, P34-08 are merged. The hub crates go first: their cards cite engine and db files (wal A25EDE9D drain step, storage 9478B37E catalog.rs, types A7B1FCEC access gate), so db/engine edits land on top of them.
- A7B1FCEC (a shamir-types card) is implemented here because its edits are in shamir-db access/authorize.rs and access/meta.rs; the borrowed accessor it needs in shamir-types is a small additive edit.
- RV-DB-C2DCAC96: set every request field explicitly; the builder defaults (return_all, id) differ from the hand-written requests. SystemStore/replication construction sites are out of scope and only get a one-line exception comment.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-ENGINE-2A18B647, RV-DB-C2DCAC96, RV-DB-0065D2A2, RV-DB-7C59E01B, ENGINE-STYLE-DELIVERY-PROVENANCE. The full decision paragraph sits in the finding below.

- L3-RV-ENGINE-2A18B647: **IMPLEMENT** (change as decided).
- L3-RV-DB-C2DCAC96: **IMPLEMENT** (change as decided).
- L4-RV-DB-0065D2A2: **SKIP** (no change).
- L4-RV-DB-7C59E01B: **IMPLEMENT** (change as decided).
- L4-ENGINE-STYLE-DELIVERY-PROVENANCE: **SKIP** (no change).

## Findings (ordered by risk, then size)

### L3-RV-ENGINE-2A18B647 - Regex/Like patterns have no size cap and invalid patterns silently compile to False

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Option 1, approve the infallible adapter contract and close the validator gap. (a) Document compile_filter and compile_filter_with_context (shamir-engine-query/src/query/filter/evaluation/planning/compile.rs:28-39) as adapters for pre-validated filters: any invalid or over-cap pattern folds the WHOLE filter to FilterNode::False (match nothing). Errors propagate through Not/And/Or, so a NOT cannot invert the fold, and wire input is validated earlier by validate_executable_guards. Keep the signatures and every current caller. (b) In validate_entry_surfaces (shamir-engine/src/query/batch/exec/batch_validate.rs:151-163), also collect: BatchOp::Call params as value surfaces; every BatchOp::Subscribe source filter as a filter surface; and computed field `default` FilterValues carried by SetTableSchema/AddSchemaRule as value surfaces (confirm how the op carries them; skip any op that does not). Run them through the existing depth and pattern guards, with the same QueryError mapping. Regression tests (rv_engine_2a18b647_*): an invalid regex (and one over MAX_FILTER_PATTERN_LENGTH) inside a $cond in a Call param, in a Subscribe filter and in a schema default is rejected at validation with a QueryError naming the alias; old code accepts all three. A valid pattern in each surface still passes. Out of scope: moving the ~20 internal compile_filter callers to try_*, RLS row_filter, and stored schemas already on disk (no re-validation of persisted data).
- Evidence at `07bc804c`: crates/shamir-engine-query/src/query/filter/evaluation/semantics/pattern_guard.rs:30-79; crates/shamir-engine-query/src/query/filter/evaluation/planning/compile.rs:28-38,58-86
- Recipe: Approve the infallible adapter contract or migrate direct compilation consumers to fallible APIs with invalid-pattern oracles.
- Verifier note: Caps and checked Boolean recursion are repaired; public infallible adapters still erase invalid-pattern errors into False.

### L4-RV-ENGINE-3C71057D - FK exists_in_table fallback: full parent-table scan per child record

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`.
- Evidence at `07bc804c`: crates/shamir-engine-validator/src/validator/schema/schema_validator.rs:178-184; crates/shamir-engine/src/validator/engine_bound/validator_db.rs:269-277,339-375
- Recipe: Reuse batch-scoped parent membership while preserving transaction-staged parent visibility.
- Verifier note: Per-record FK probes still reach fresh full parent scans when indexes are absent or unavailable; batch membership must preserve staged visibility.

### L3-RV-DB-23A03103 - `SystemStore::load_repository_record` full-catalogue scan + linear filter for a keyed lookup

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/system_store/catalogue.rs:165-170,319-346; crates/shamir-db/src/shamir_db/system_store/store.rs:138-141
- Recipe: Provide a maintained composite identity index or keyed repository lookup while preserving corruption semantics.
- Verifier note: Filtered lookup removes full-result materialization but still uses an unindexed catalogue; keyed remediation must preserve corruption errors.

### L3-RV-DB-C2DCAC96 - Builder-only query-construction rule bypassed across the facade (30+ hand-assembled wire ops, no exception comments; builder is dev-dep only)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `medium`). Move shamir-query-builder from [dev-dependencies] to [dependencies] of shamir-db (crates/shamir-db/Cargo.toml:99). It is a workspace crate, no new third-party crate, no version bump; the Cargo.lock change is only the new dependency edge. Rewrite FacadeDbGateway::get/insert/query (db/db_gateway.rs:123-330), including key_to_filter/qv_to_filter_value (:43-95), to use shamir_query_builder (Batch, Query, filter/val constructors, write builders). Set EVERY batch field explicitly to today's value: id 'db_get' and the siblings, name None, transactional false, return_all false, return_only Some([alias]), default limits/encoding/epochs. Batch::new() defaults to return_all=true and id=Null (shamir-query-builder/src/batch/batch.rs:61-77), so relying on defaults is a bug. Must stay identical: the BatchRequest value sent to execute_as, error classification (classify_batch_error), the actor/authorization path, and response extraction. Where the builder cannot express a gateway field, keep the literal with a one-line comment saying why. Regression test (rv_db_c2dcac96_*): for each of get/insert/query, the builder-built BatchRequest equals the pre-change literal, compared with PartialEq or rmp-serde bytes for fixed inputs (Int, Str and composite keys; repo main and non-main). Out of scope: SystemStore (system_store/*.rs), replication and seed-catalogue sites. Those are typed table-API arguments, not wire batches, and SetOp has no builder; they get exactly one comment line per file stating that exception and are not migrated. No behaviour change anywhere.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:136-179; crates/shamir-db/Cargo.toml:99
- Recipe: Approve the production builder dependency and migrate facade/SystemStore construction while preserving defaults and batch semantics.
- Verifier note: Production gateway directly constructs and executes DTOs; approve builder dependency and preserve existing semantics.

### L3-RV-ENGINE-62C6F2B1 - Content-key cache remediation introduces per-evaluation formatting allocation and AST traversal

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-engine-query/src/query/filter/caches/cond_cache.rs:56-57,93-100; crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:334-344,424-438
- Recipe: Use prepared plan-local condition slots instead of formatting the AST on cache hits.
- Verifier note: Borrowed field-path lookup is fixed; reachable condition-cache hits still format the AST, requiring lifetime-safe plan-local slots.

### L3-RV-TYPES-A7B1FCEC - Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:65-67; crates/shamir-types/src/entries/resource_path_ref.rs:105-109; crates/shamir-db/src/shamir_db/shamir_db/access/meta.rs:141-148
- Recipe: Reuse a borrowed canonical folder key across metadata traversal and add a deep-chain CI allocation oracle.
- Verifier note: Borrowed ancestor iteration removes Vec/clones, but slice-backed folder metadata lookups still join each prefix.

### L4-RV-DB-5977A782 - `resolve_group_id(GroupRef::Name)` is a full `load_groups()` scan per call

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/shamir_db/access/groups.rs:388-398; crates/shamir-db/src/shamir_db/system_store/identities.rs:168-175
- Recipe: Make snapshot absence authoritative with generation-safe invalidation on every group mutation.
- Verifier note: Positive cache hits avoid scans; missing names still perform full catalogue reads and need generation-safe negative lookup.

### L3-RV-DB-A7A29B02-FACET - Boot catalogue skip diagnostics, not a claim that recovery always succeeds

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/code`.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/shamir_db/core.rs:385-427,888-900
- Recipe: Warn with repository identity, backend and failure reason when factory reconstruction returns None.
- Verifier note: Attach and missing-parent failures propagate, but None factory reconstruction silently omits persisted repositories; add contextual diagnostics.

### L4-RV-DB-0065D2A2 - Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Already resolved by the P2 decision for RV-DB-0065D2A2, which is in master: wasm_hash is documented as a noncryptographic diagnostic fingerprint and function version=1 as legacy informational metadata (db/function_management.rs:196,218; db/validator_management.rs:243). Nothing in the code reads wasm_hash. Removing or deprecating the fields would change persisted catalogue rows for no benefit. Record: supported informational metadata, explicitly not an integrity guarantee or replacement counter; no code change.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:195-246; crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs:242-246,272-273
- Recipe: Explicitly retain informational fields as supported metadata or deprecate/remove them under a compatibility policy.
- Verifier note: Write-only diagnostic hashes and constant function version remain; choose supported metadata or compatible deprecation.

### L4-RV-DB-7C59E01B - Nit: intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count is unbounded by unique-name volume

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Docs-only, continuing the P2 SKIP of RV-DB-7C59E01B. Rewrite the doc comments of ShamirDb::admin_user_locks, group_member_locks and repo_create_locks: the fields (shamir_db/core.rs:83-100,128-131) and their pub accessors (core.rs:666-712). State the accepted tradeoff: one retained entry per distinct key ever locked, inserted only after authorization, at admin/DDL frequency, never evicted, because eviction can split synchronization for a live key and the maps are reachable through public accessors. Remove the 'memory cost is negligible' claims. Correct the stale admin_user_locks description: grant/revoke no longer take it (execute/admin_ddl/admin_users_roles.rs:138-142); its live use is the per-table schema RMW lock keyed by schema_lock_key (execute/admin_ddl/admin_schema.rs:64-77; execute/repo_admin/admin_schema_admission.rs:14-16). Do not change types, visibility, field names, lock order or any code. No test (comment-only). Out of scope: striping, a key budget, encapsulating the accessors.
- Evidence at `07bc804c`: crates/shamir-db/src/shamir_db/shamir_db/core.rs:83-100; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:70-76,380-396
- Recipe: Choose bounded striped locking or an explicit retained-key budget without naive live-entry eviction.
- Verifier note: Schema operations insert permanently retained distinct-key locks; use bounded striping or an explicit admission budget.

### L4-ENGINE-STYLE-DELIVERY-PROVENANCE - Style-delivery provenance remains partial after the structural code edits

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted historical exception. 6c286cc9 (#1173-#1205 remediation campaign) mixed style and substantive edits and is deliberately absent from .git-blame-ignore-revs:1-14. History cannot be rewritten, and listing a substantive commit there would hide real authorship. The forward rule already exists (CLAUDE.md:370-374, AGENTS.md:130-134). Record that in the audit ledger only; no source, ignore-revs or CI change.
- Evidence at `07bc804c`: docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md:13491; .git-blame-ignore-revs:1-14
- Recipe: Record an accepted historical delivery exception and require isolated style commits prospectively.
- Verifier note: Historical mixed delivery remains; accept an explicit exception and enforce isolated style delivery prospectively without hiding substantive commits.

## Carried over from the P2 sprint

These P2 cards stayed open after the sprint (state from `P2-SPRINT-REPORT-2026-10-30.md`). Their evidence and recipe are the P2 revalidation at master `7f04e096`; re-locate the code by symbol.

### RV-DB-39178D2D - `InternerTouch` computes the epoch via a full interner traversal per touch

- Sprint state: PARTIAL: implemented, no cost-discriminating regression.
- Risk `medium`; fix size/kind `L/code`.
- Evidence: crates/shamir-db/src/shamir_db/execute/admin_meta/admin_interner.rs:174-179; crates/shamir-types/src/core/interner/interner.rs:454-462
- Recipe: Provide allocation-free published-ID maximum access while preserving explicit-ID and publication-gap semantics.
- Verifier note: Successful touch materializes the complete published dictionary to calculate its maximum ID; preserve explicit-ID and gap semantics.

### RV-DB-3EB43AC7 - Boot path pairs repos with their tables via an O(repos × tables) nested scan

- Sprint state: PARTIAL: implemented, no cost-discriminating regression.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-db/src/shamir_db/shamir_db/core.rs:361-397
- Recipe: Group table records once by database/repository while preserving registration-before-recovery order.
- Verifier note: Repository loop still scans the full table catalogue.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-db -p shamir-engine -p shamir-engine-query -p shamir-engine-validator -- --check`; `cargo clippy -p shamir-db -p shamir-engine -p shamir-engine-query -p shamir-engine-validator --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
