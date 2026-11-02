# P34-03c — Phase 0 verification

## Phase 0

**Decision: STOPPED at Phase 0: stop condition A fired.** This note records source observations, inferences, and a proposed alternative/status only. No production outcome, runtime failure, or crash result is claimed. The repository-rename failure matrix below is source-based analysis, not an implemented or runtime-verified matrix.

## Q1 — Writer exclusion — STOP A

**OBSERVED:** Public `create_unique_index` is at `crates/shamir-engine/src/table/table_manager/operations/table_manager_index_mgmt.rs:232-235`; it then reaches the admitted core at `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_core.rs:126-155`. The normal write barrier does not reject migration freeze. Diagnostics/doctor repair and `set_buffer_config` are also DDL paths inspected at `crates/shamir-engine/src/table/table_manager/diagnostics/doctor.rs:557-603,605-714` and `crates/shamir-engine/src/table/table_manager/operations/table_manager_buffer.rs:29-44`; buffer persistence is `crates/shamir-engine/src/table/support/model/buffer_config.rs:56-61`.

Freeze owns only the migration-freeze bit: `crates/shamir-engine/src/table/support/guards/migration_freeze_guard.rs:8-26`; its observed DDL barrier order is `crates/shamir-engine/src/table/table_manager/barriers/ddl_barriers.rs:72-88`. The writer-drain evidence is `crates/shamir-engine/src/table/support/guards/writer_drain_barrier.rs:228-250`. Existing tests at `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs:43-77,103-135,190-255` were inspected, not run. The barrier path is `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:51-87`; the subsequent mutation is `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_core.rs:224-259`.

CRUD and transaction gates do work for the inspected paths: `crates/shamir-engine/src/table/table_manager/operations/table_manager_crud.rs:213,325,506,600` and `crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs:263-325` (preparation/pre-commit/prelock). This does not establish universal exclusion: the DDL paths above disprove that broader claim. A rename-held repository permit excludes competing admitted DDL only while held. InDoubt releases that permit while retaining freeze; a stale `TableManager` can then admit and perform DDL. **INFERENCE:** such metadata changes could be lost when the copied destination is selected. Seed is an explicit non-goal in `docs/dev-artifacts/prompts/remediation/p34/03c-rename-in-doubt-barrier.md:60-62`, not the basis for stop A.

**Proposed alternative/status:** Do not proceed on the current exclusion premise. Require all-writer exclusion for the full transition, including stale-handle DDL. A freeze-aware design must cover all admitted writers or use a dedicated lifecycle gate. Retaining DDL admission across permit reacquisition can deadlock; this needs an explicit lock-order design, not a prescription here.

## Q2 — Catalogue transaction commit point

**OBSERVED:** `rename_table_row` validates/builds operations, then persists the interner before its implicit transaction: `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:266-367`. That pre-call interner persistence stores dictionary state only, not the rename row. Ordered transaction stages are begin, destination set, filter context, source delete, and commit: `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:367-389`; transaction wrapper: `crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs:410-433`.

Pre-commit wounding, expiry gate, WAL, prelock, and validation-error flow are at `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:605-678` and `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs:296-317`. WAL begin is `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs:341-348`; completion is `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs:350-363`. `WrittenNotDurable` ultimately publishes then reports `WalDurability` at `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs:434-439`, but receipts convert it to `Ok` at `crates/shamir-engine/src/query/batch/control/receipts/commit.rs:60-70`; `run_implicit_batch_tx` discards that information at `crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs:432`. Unknown errors propagate from receipts at `crates/shamir-engine/src/query/batch/control/receipts/commit.rs:71-75`. Thus gate 2 cannot assert that every returned error includes `WalDurability`.

The interner pre-call persistence proves only the dictionary write, not rename completion. Stop B was not found in the inspected path; classifying only before invocation is sound for **this attempt**, not for prior retry attempts. Recovery has analogous ordering at `crates/shamir-db/src/shamir_db/system_store/catalogue/rename_recovery.rs:135-162`. These are code observations; no runtime behavior is proven. The actual pre-call dictionary persist is `crates/shamir-db/src/shamir_db/system_store/store.rs:432-455`. WAL completion distinguishes proven NotWritten from Unknown/Indeterminate at `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/commit_policy.rs:28-48`. After the implicit transaction the caller still performs the fallible durability flush at `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:389`.

## Q3 — Permit and open / deadlock

**OBSERVED:** The admitted `get_table_admitted` path passes the existing permit through `get_table_ctx` and does not re-admit: `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:77-95`. The permit owns the repository mutex guard and is non-cloneable; `admit()` waits on that mutex: `crates/shamir-engine/src/repo/journal/changelog_store/write_domain.rs:50-64,131-165`. The table admission method acquires the repository domain at `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:39-43`. CRUD takes writer-drain before flag and does not repo-admit; transaction prelock does not repo-admit, as inspected in Q1. No reverse lock cycle was found in these inspected paths. This is not a universal deadlock proof.

## Q4 — Shared flags / manager identity

**OBSERVED:** `TableManager::clone` copies manager state at `crates/shamir-engine/src/table/table_manager.rs:500-528`. Flags are backed by `Arc` at `crates/shamir-index/src/base_index/infra/write_barrier_flags.rs:176-192`. The constructor is `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:174-179`. Standard `OnceCell` membership entries for a manager share flags at `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:112-137`. `add_table_inner` at `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:301-303` creates an entry guard, not a fresh cell. `remove_table_inner` evicts at `crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:320-345`. The explicit test-util install exception is `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:219-225`. No claim beyond the standard registered-manager path.

## Q5 — Boot registration

**OBSERVED:** The actual boot function is `ShamirDb::init_inner` at `crates/shamir-db/src/shamir_db/shamir_db/core.rs:238,349-403`; it loads catalogue records, groups tables by repo, builds configs, and registers repos. A destination-only row creates only a destination `TableConfig`, with its `store_id` under the destination `table_name`. Rename provenance is checked but not used to resurrect source config: `crates/shamir-db/src/shamir_db/system_store/catalogue/table_catalogue_config.rs:22-37`. No existing rename-fence field is present in `crates/shamir-engine/src/repo/instance/repo_instance.rs:41-164`; new constructors have no memory of a prior process. Restart safety is therefore conditional on exclusion, which is not fulfilled by the observed DDL paths.

## Q6 — `rename_repo_as` failure matrix (not implemented)

**OBSERVED sequence:** Memory re-key, save destination repository row, remove source repository row, then for each child load metadata, save under destination, and remove under source: `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:603-730`. The engine removes old, applies `with_name`, then inserts new at `crates/shamir-engine/src/db_instance/db_instance.rs:184-191`. Every failure after `db_management.rs:669` leaves memory keyed only by destination; no rollback. Same-process retry from→to then returns `NotFound` at `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:626-630`—not “not safely classifiable.” Failures before staging or after WAL mean rows can be runtime/durable differently on each call; the table distinguishes possibilities, not observed outcomes.

| Failure point | Runtime rows / children after failure | Restart implications / retry |
|---|---|---|
| Pre-rekey guards or metadata load fail | Memory remains source-only; this attempt has performed no rename-row writes. | Retry can start normally; existing catalogue remains authoritative. |
| `save_repository(to)` error | Old repo row remains; new row may be absent, replayable, or visible. Children remain old-keyed. | Restart may see old only or both; new repo may have an empty child list. Same-process retry gets `NotFound`. |
| `remove_repository(from)` error | Destination save completed; old row may remain. Children remain old-keyed. | Restart may see destination without children and old repo may or may not register. Same-process retry gets `NotFound`. |
| Child load error or non-map | Repository catalogue switched after successful remove call; earlier children moved, failing child and suffix remain old-keyed. | Restart groups only destination-keyed children; old-key rows are stranded without the old repo row. Missing child is warning/skip, not error. |
| `save_table_meta` error | Existing child remains old-keyed; new row may be absent or uncertain. Earlier children moved; later children untouched. | Restart attaches only destination-keyed rows to new repo; old-key rows stranded. Same-process retry gets `NotFound`. |
| `remove_table` error | New child save completed; old child row may remain. Earlier moved, suffix untouched. | Restart may expose both old/new child rows; destination grouping sees only destination-keyed child. Same-process retry gets `NotFound`. |
| End success | Rows are new-keyed and old rows removed by successful calls. | Disk durability remains subject to backend semantics; not proven. |

Exact durable helpers: `save_repository` `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:89-120`; `remove_repository` `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:124-147`; `save_table_meta` `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:608-645` (not `save_table`); `remove_table` `crates/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:392-425`. Each helper has an implicit transaction and `data_store_read_view.flush`; repository/table saves additionally persist dictionary, removes do not persist interner. There is no shared transaction or rollback. Restart aliasing of the same backing store is an inference, not observed. Missing child rows warn/skip at `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:717-730`.

## Error-code wiring caveat

`DbError` codes exist at `crates/shamir-storage/src/core/error.rs:292-294`, but admin mappings at `crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:255-259,294` construct errors with `code: None`. No production changes were made.

## Progress

- Item 1: **DONE** — gate answered; stop A fired.
- Items 2–9: **PARTIAL — not implemented**; names are planned acceptance only, not claimed work. No tests added or run.
- Item 7: source review completed, **NOTE ONLY**; no recovery implementation.
- Item 10: **PARTIAL** — layout exit 0 observed by main; formatting N/A, CI not run.

## Final report

1. DONE — Phase 0 Q1–Q6 source evidence and Q6 matrix recorded above; stop A exercised as allowed by `docs/dev-artifacts/prompts/remediation/p34/03c-rename-in-doubt-barrier.md:20-29`.
2. PARTIAL — concurrent writer acceptance not implemented; no tests added or run (acceptance 2, brief line 50).
3. PARTIAL — roll-forward not implemented; no tests added or run (acceptance 3, brief line 51).
4. PARTIAL — in-doubt fencing/retry not implemented; no tests added or run (acceptance 4, brief line 52).
5. PARTIAL — not-committed behavior not implemented; no tests added or run (acceptance 5, brief line 53).
6. PARTIAL — restart coverage not implemented; no tests added or run (acceptance 6, brief line 54).
7. PARTIAL — recovery-branch coverage not implemented; no tests added or run (acceptance 7, brief line 55).
8. PARTIAL — contract regressions not implemented; existing tests untouched and unexecuted (acceptance 8, brief line 56).
9. PARTIAL — engine outcome/fence/retire coverage not implemented; no tests added or run (acceptance 9, brief line 57).
10. PARTIAL — layout check passed in main review; Rust formatting N/A because no crate changed; compilation, clippy, tests and CI not run (acceptance 10, brief line 58).

STOPPED at Phase 0: stop condition A fired. Q1-Q6 and the repository-rename failure matrix are recorded in DESIGN.md. Only the audit note changed; production code and tests remain untouched. Layout check passed; compilation and tests were not run. Next step: revise all-writer exclusion to cover stale-handle DDL, then repeat Phase 0.

Editor-child rules observed: no commands, code/test mutation, or further delegation. Main used read-only tools and ran date and the permitted layout checker. No Rust imports, signatures, or literals changed; Rust lint checklist N/A. The main review reported the parent note directory has one entry and is docs-exempt from layout limits. Main-observed layout result: `python -I scripts/ci/check_layout.py` exit 0, `layout check: OK (0 baselined, 0 hint(s))`. No other changes.
