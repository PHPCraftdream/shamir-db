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

## Phase 0 (revision 1)

### Q7 — Visibility before the drain — D NOT established

**OBSERVED:** The initial STOP D story is refuted for data-bearing rows. `RepoTxGate::assign_next_version` returns `fetch_add(1) + 1` (`crates/shamir-tx/src/gate/repo_tx_gate.rs:688-689`); the accessor at `:441-442` is the largest version already allocated, despite nearby next-version wording. `await_visible` waits on completion-floor progress (`crates/shamir-tx/src/gate/repo_tx_gate/progress.rs:185-189,289-315`). In a committing tx, `apply_committed_visible` inserts the row into the overlay and raises the published floor (`crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:493-551`); `VersionGuard::commit` marks completion then `fetch_max`s last committed (`crates/shamir-tx/src/version/version_guard.rs:150-164`). `CompletionTracker` CASes and notifies before `fetch_max` (`crates/shamir-tx/src/tx/support/core/completion_tracker.rs:217-234`). Materialization drops writer/drain guards only after its Phase 5c writes (`crates/shamir-engine/src/tx/commit/pipeline/phase/materialize.rs:190-198`). The non-transactional publication path persists history/overlay before `guard.commit()` (`crates/shamir-tx/src/mvcc_store/store.rs:579-604`).

**QUALIFICATION:** Universal `await_visible(v) => last_committed() >= v` is false: completion can be marked for an aborted or empty/no-source-write version, so this does not establish that the source has a row to drain. Conversely, source rows from the inspected paths are eagerly materialized/published or durable before the guards drop. Therefore D is **not established**; neither a runtime test nor a positive visibility guarantee is claimed. This amends the original revision's proposed universal inference.

### Q8 — Token lookup callers — STOP E

**OBSERVED caller inventory:** `table_by_token_if_live`: `crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs:263-284` (tx prelock; stages writer-drain participation only for `tx.write_set` keys); `crates/shamir-engine/src/tx/preparation/pre_commit/stale_value.rs:147`, `crates/shamir-engine/src/tx/preparation/pre_commit/index2.rs:162,428`, and `crates/shamir-engine/src/tx/preparation/pre_commit/base_index.rs:53` (operation/read/staging contexts; these are not evidence that every subsequent durable mutation is freeze-gated). `table_by_token`: `prelock.rs:295-317` (unique-lock prelock); `crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:231,501,604` (orphan resolution, data replay, posting replay); `crates/shamir-engine/src/tx/commit/pipeline/phase/materialize.rs:289,620,681` (counter, graph, delta materialization); `crates/shamir-engine/src/tx/commit/pipeline/phase/commit_phases.rs:771-824` (index posting apply). `table_by_token_admitted` occurs at `crates/shamir-engine/src/tx/recovery/v2.rs:92,123,189,253` (admitted recovery callers). `prelock.rs:577,677` are normal lookup/read info-store paths, not admitted. The cited callers do not themselves perform catalogue DDL; that observation does not imply their target stores cannot be mutated.

**STOP E — concrete source-reachable hole, not runtime reproduced:** `prelock.rs:261-284` derives drained/prelocked table tokens only from `tx.unique_guards` and `tx.write_set.keys()`. The public `TxContext.index_write_set` is at `crates/shamir-tx/src/tx/tx_context.rs:140-147`; `is_empty` counts index-only work at `:644-650`. `materialize.rs:150-174` consumes those index ops, and `commit_phases.rs:754-835` resolves the token then applies posting operations directly to `tbl.info_store()` at `:771-824`, with no freeze check or writer-drain admission at this mutation. `crates/shamir-engine/src/tx/preparation/pre_commit/locked.rs:150-174` does not reject index-only transactions: `is_empty` is false, allocation proceeds and a WAL entry is built. `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:498-516` serializes posting ops independently of the data loop at `:523`. `prelock.rs:508-538` validates unique provenance only; nonunique/default manually staged operations pass, and no per-a generation map exists for re-derivation loops to catch a. `crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:607-614` independently documents that manually staged index-only operations do not participate in writer drain. Thus a supported engine tx may stage `SetPosting` for frozen table `a` while its data write set contains only unrelated `b` (or is empty) and unique guards are empty: preparation does not prelock/drain `a`, yet Phase 5c can mutate `a`'s info store. That version may be allocated after a rename's sampled visibility target. R1 gates DDL admission, but this commit path does not call `TableManager::admit`; R1/R2 as proposed do not close E. This is source reachability analysis only, not a reproduced runtime result.

Cold recovery's public `replay_v2_op` and admitted/raw info-store application are at `crates/shamir-engine/src/tx/recovery/v2.rs:189-203,253-268`; it too is not a freeze/detach check. Recovery refs are in that same module. Distinguish these recovery paths from live drainer routing: `drain_step.rs:365` routes non-batched/broadcast entries through `replay_v2_op`, while ordinary postings are batched at `:604-674`; the orphan path takes `begin_write_barrier` at `:617`, which a proposed detach check would reject. Legitimate replay of already accepted work must be reconciled so it can settle without deadlocking or re-admitting writes. Do not conflate that with a new post-freeze index-only transaction.

**Proposed alternative/status:** STOP. Before code, amend the design/brief: prelock the union of touched table tokens from data, index, counter, and vector write sets; require freeze and writer-drain/UWL participation for every durable mutation, not merely DDL. Define replay semantics for already accepted versions so they can settle visibility without allowing new admissions or blocking the retry. No such change is implemented.

### Q9 — Rename cancellation/call chain

**OBSERVED:** Server handling is `crates/shamir-server/src/engine/db_handler/handler.rs:721-730` -> `crates/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:51-71` -> `crates/shamir-db/src/shamir_db/execute/core/admin_dispatch.rs:46-50` -> `crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:268-294` -> `rename_table_as_locked`. Public library wrapper: `crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:253-263`. Connection request-loop EOF teardown is `crates/shamir-server/src/sync/connection/request_loop.rs:148-153,483,491-494`; server shutdown cancellation is `crates/shamir-server/src/engine/server/server_handle.rs:142-148` -> `crates/shamir-server/src/sync/connection/session/conn_tasks.rs:90-117`. No production timeout around rename was identified in this source review. A library caller can drop/timeout its future in principle; this is not an observed cancellation event. R5 remains mandatory.

### Q10 — TableManager mutators / admission inventory — STOP F not established

The inventory below covers the Q10-relevant public wrappers and admitted twins from source review. “Wrapper” means it reaches `TableManager::admit`; the proposed R1 would gate that route. “Explicit” means the admitted mutation requires an `ensure_attached` check after permit validation. These are proposals, not implemented checks; STOP E independently stops implementation.

**Wrapper entry points proposed gated by R1 via `admit`:** Under `crates/shamir-engine/src/table/table_manager/`: `operations/table_manager_index_mgmt.rs` owns `create_index_v2` (`:34,55`), `create_index` (`:211-212`), `create_unique_index` (`:232-233`), `drop_index` (`:253-254`), `drop_unique_index` (`:268-269`), `drop_index2` (`:344-345`), and `rename_index` (`:982-989`). `operations/table_manager_replication.rs` owns `replicate_interner_from` (`:35-37`), `replicate_index2_descriptors_from` (`:91-94`), `seed_log_from_data_store` (`:176-178`), and `bulk_populate_index2` (`:219-221`). `operations/table_manager_sorted_index.rs` owns `create_sorted_index` (`:37-40`), `create_sorted_index_with_include` (`:45-59`), and `drop_sorted_index` (`:354-362`). `operations/table_manager_buffer.rs` owns `set_buffer_config` (`:29-31`) and `alter_buffer_config` (`:56-63`). `diagnostics/doctor.rs:557-559` owns `repair`; `barriers/ddl_barriers.rs:49-55,67-69` owns `begin_write_barrier` and `begin_migration_freeze`; `accessors/table_manager_validators.rs:38-45,73-80` owns `add_validator_binding` and `remove_validator_binding`.

**Admitted operations proposed to get explicit detach check:** `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_core.rs:9,126,263,289,312` — `create_index_admitted`, `create_unique_index_admitted`, `drop_index_admitted`, `drop_unique_index_admitted`, `drop_index2_admitted`; `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_admitted.rs:16` — `create_index_v2_admitted`; `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_rename_admitted.rs:217` — `rename_index_admitted`; `crates/shamir-engine/src/table/table_manager/operations/table_manager_sorted_index.rs:62,365` — `create_sorted_index_with_include_admitted`, `drop_sorted_index_admitted`; `crates/shamir-engine/src/table/table_manager/operations/table_manager_buffer.rs:35` — `set_buffer_config_admitted`; `crates/shamir-engine/src/table/table_manager/diagnostics/doctor.rs:563` — `repair_admitted` (no `debug_check_permit` today; identity then attachment check needed); validators `crates/shamir-engine/src/table/table_manager/accessors/table_manager_validators.rs:49,84` — `add_validator_binding_admitted`, `remove_validator_binding_admitted`; `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/admitted_ops.rs:35,52,132,160,245,285` — `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, and both migration-coordinator forms (check both captured source and destination). Replication methods are in `crates/shamir-engine/src/table/table_manager/operations/table_manager_replication.rs:41,98,182,225`: `replicate_interner_from_admitted`, `replicate_index2_descriptors_from_admitted`, `seed_log_from_data_store_admitted`, `bulk_populate_index2_admitted`.

**Exemptions / scope distinctions (not unreviewed claims of coverage):** `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:51` `begin_write_barrier_admitted` is infallible; audit its callers, which resolve under the same permit. Add explicit detach check to permit-taking `begin_write_barrier_with_permit` at `:98-105`. `write_domain_barrier.rs:115,132` create/open-with-domain are construction. `write_domain_rename_admitted.rs:14` recovery of hash renames; identity migration at `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/identity_migration.rs:126` (`ensure_base_identity_current`) and `:144` (`rerun_identity_migration_admitted`) are separate recovery/identity setup methods. `admitted_ops.rs:206,222` `persist_interner_admitted` / `intern_query_value_admitted` mutate repo-shared interner state, not a table store. `barriers/ddl_barriers.rs:72` migration-freeze acquisition uses the freeze bit itself (repeat claim fails). Do not blanket-gate `RecordCounter`: `crates/shamir-engine/src/table/support/storage/record_counter.rs:129,136,191,198,216,223,322-335` has separate counter APIs/trait; the table accessor is `crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:226` and its guarded CRUD/repair routes plus R4's intentional rename flush need separate treatment. `table_manager_index_mgmt.rs:351` recovery status helper is static and only called from `write_domain_barrier.rs:189`; deprecated no-op `:578` is not a mutation; recovery constructors `:483,642` need caller-specific review. `accessors/table_manager_validators.rs:106` registry setter is local clone setup; `:115-128` scalar resolver is runtime dependency injection, not persisted DDL. These scope distinctions do not establish blanket detach coverage.

**Completeness caveat:** The Q10 source-derived inventory is not exhaustive: residual classification remains for recovery helpers and wrapper routing at cited sites. STOP E establishes a concrete blocker; exhaustive caller classification is unfinished. Acceptance 1 is **PARTIAL** for that reason.

## STOP E report and status

STOP E fires: token-path index-only transaction materialization can mutate a frozen source's info store without participating in its prelock/drain set. The initial STOP D hypothesis is refuted for data-bearing rows and D is not independently established as a stop. Q9 records cancellation reachability; Q10 records proposed checks and unresolved classification limits. No runtime result is claimed.

No visibility wait, fence, detach gate, outcome/fence code, public API change, or test was implemented. Follow-up DDL during online migration remains unchanged as the stated non-goal. No outcome/crash-point matrix is asserted as lossless: the source hole above invalidates that premise.

## Progress (revision 1)

- Item 1: **PARTIAL — STOP E**; Q7-Q10 source review recorded, but Q10 exhaustive caller classification unfinished; implementation halted.
- Items 2–6: **NOT STARTED**; no production/test edits.
- Item 7: **DONE — doc-only read review**; original sections plus revision were reviewed; no commands run by this editor. Main-observed layout result: `python -I scripts/ci/check_layout.py` exit 0, `layout check: OK (0 baselined, 0 hint(s))`.

## Final report (revision 1)

1. PARTIAL — STOP E established by Q8 above; Q7/Q9 recorded, Q10 inventory present but exhaustive caller classification unfinished. No production code written, as required by the stop condition (`docs/dev-artifacts/prompts/remediation/p34/03c-revision-1.md:38-43`).
2. PARTIAL — detach gate and engine DDL tests not implemented; no tests added or run.
3. PARTIAL — successful-rename stale-handle DDL coverage not implemented.
4. PARTIAL — in-doubt stale-handle DDL coverage not implemented.
5. PARTIAL — concurrent non-tx/tx writer coverage not implemented.
6. PARTIAL — visibility-wait implementation and tests not added.
7. PARTIAL — cancellation-safe attempt and cancellation tests not implemented.
8. PARTIAL — roll-forward reconciliation/tests not implemented.
9. PARTIAL — name fencing, token-write exclusion, retry convergence and repo-rename guard/tests not implemented.
10. PARTIAL — NotCommitted retry classification/tests not implemented.
11. PARTIAL — on-disk restart tests and nextest group changes not added.
12. PARTIAL — recovery-branch classification/tests not implemented.
13. PARTIAL — wire-code/contract changes and coverage not implemented.
14. PARTIAL — engine outcome/guard/mirror tests not added.
15. PARTIAL — main ran `python -I scripts/ci/check_layout.py`: exit 0, `layout check: OK (0 baselined, 0 hint(s))`. Parent audit directory contains only `DESIGN.md` and is docs-exempt. Rust formatting, import/visibility/signature/struct-literal/exhaustive-match/Send/result_large_err checks are N/A because no Rust file changed. No tests, compilation, clippy or CI ran; future implementation needs CI.

STOPPED at revision Phase 0: stop condition E fires for index-only transaction postings that bypass the source freeze. Only DESIGN.md changed; production code and tests are untouched. No local compilation or tests ran. Next step: amend the prelock/token-write design to cover every mutated table before implementing R1–R11.
## Progress (revision 2)

- Item 1: **DONE — Phase 0 Q11–Q13** recorded above; H was not established (source analysis only).
- Item 2: **DONE — S1 and its three sidecar-commit tests** recorded at line 209 above.
- Item 3: **PARTIAL — S2/S3** as described at line 177; parent-reported correction review accepted core receiver/duplicate-check corrections. No tests/builds run.
- Item 4: **DONE — S4/S5** as described at lines 210–212. No tests/builds run.
- Item 5: **PARTIAL — acceptance tests 3–9** as listed at line 213 and in the appended test note below. Rust review/format/CI remain.

## Design (revision 2) — implementation record and remaining scope

### S1 — prelock token union

`prelock.rs` now derives the touched-token set from `write_set`, `index_write_set`, `counter_deltas`, `staged_vectors`, and `staged_vector_deletes`, then sorts/deduplicates. The loop retains existing writer-drain/slow-path logic. Tests recorded at line 209 check frozen index-only and counter-only commits plus an unfrozen index-only control. No runtime result is claimed.

### S2 — detach gate

`TableManager::detached` is `pub(super)` and shared through Clone; constructors initialize false. `detach_for_rename` / `reattach_after_rename` store Release; `ensure_attached` loads Acquire and returns `MigrationFrozen`. `admit` gates public wrappers; explicit gates are in the 24 brief-listed admitted methods, including both migration coordinator twins on source and destination, plus `begin_write_barrier_with_permit`. CRUD/commit do not read detached.

The explicit methods are `add_validator_binding_admitted`, `remove_validator_binding_admitted`, `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, `migration_coordinator_admitted`, `migration_coordinator_recovered_admitted`, `rename_index_admitted`, `create_index_v2_admitted`, `create_index_admitted`, `create_unique_index_admitted`, `drop_index_admitted`, `drop_unique_index_admitted`, `drop_index2_admitted`, `repair_admitted`, `set_buffer_config_admitted`, `replicate_interner_from_admitted`, `replicate_index2_descriptors_from_admitted`, `seed_log_from_data_store_admitted`, `bulk_populate_index2_admitted`, `create_sorted_index_with_include_admitted`, `drop_sorted_index_admitted`, and `begin_write_barrier_with_permit`.

**Exemptions (12 named APIs/types and reasons):** (1) `begin_write_barrier_admitted`, (2) `begin_write_barrier_admitted_impl` — infallible; callers resolve the table under the same permit or are checked twins; (3) `begin_migration_freeze_admitted` — the freeze bit rejects a second claimant; (4) `create_with_domain_admitted`, (5) `open_with_domain_admitted` — construction/open; (6) `recover_hash_renames_admitted`, (7) `rerun_identity_migration_admitted` — open-time recovery/identity setup; (8) `persist_interner_admitted`, (9) `intern_query_value_admitted` — repo-shared interner, not a table store; (10) `RecordCounter::set_admitted`, (11) `RecordCounter::set_to_admitted`, (12) `RecordCounter::persist_admitted` — separate metric/counter APIs; counter persistence is intentionally called on the detached source during rename preparation. No visibility of existing items was widened by S2.

Rejected alternatives: holding `ddl_admission` across the attempt can deadlock; checking `MIGRATION_FREEZE` in DDL would block migration coordinator DDL on its frozen source (`admin_migration.rs:460`); adding a barrier bit is unavailable because the u8 selector space is full. Unmounted accidental file `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_core.rs` remains untouched per no-delete instruction; current directory listing had four entries, not more than the seven-entry cap. This file is not mounted.

### S3 — guard retirement

`MigrationFreezeGuard` now has private `armed`; `try_set` arms it, `retire` disarms it, and `Drop` clears only while armed. The acceptance tests named above were added, not run.

### S4 — rename freeze RAII and visibility wait

New crate-private type/methods: `RenameFreeze::{begin, await_visible, source, retire}` and `Drop`. It begins the admitted freeze, detaches the source, bounds gate initialization plus high-water visibility wait, matches `Visible` and `ReopenRequired` exhaustively, returns the underlying gate error or a timeout `Conflict`, and on abort/cancel reattaches before explicitly dropping its freeze guard. Successful `retire` keeps source detached/frozen. No existing item visibility was widened.

### S5 — transition integration

`rename_table_transition` and `rename_table_stores_admitted` precheck configuration after admission, resolve source with that permit, freeze and await visibility before prepare, drain history, flush the counter, copy, persist and commit, then retire only on success. Private `prepare_table_rename_admitted` now takes `&RenameFreeze`; its callers remain within `catalog.rs`. `commit_table_rename_admitted` has no freeze parameter. Recovery freezes only when source identity matches, before persistence, and retires after successful source removal; it does not await visibility or flush the counter because it does not copy stores. Public rename/recovery signatures remain unchanged. Counter source review: increment is cache-only (`record_counter.rs:149-185`); dirty `persist_admitted` writes through (`:223-243`), so the counter test is expected to distinguish lazy persistence on master, but this is not a runtime result.

### Test evidence and clarification

The held-version visibility test cannot use strict `insert`: `insert_returning_version` calls `write_receipt` with `VisibilityWait::Strict` (`table_manager/admission/crud.rs:101-110`), which awaits the unsettled version. It instead uses `insert_with_receipt` with a bounded wait, asserts a pending receipt and reads back the row while keeping the version guard alive, then checks admitted buffer-config DDL. Parent source review established `open_index_build_snapshot` uses `snapshot_at` (`shamir-tx/src/mvcc_store/gate.rs:492-505`), which pins without waiting for the settled floor. Therefore the earlier inference that `create_index` must block behind a held gate guard was incorrect; no such claim is made. The test uses buffer-config DDL as a writable probe, a deviation from the brief's suggested index-create probe, for parent review. No test was run.

### M1–M7 remaining failure modes (brief table)

| # | Situation | Sub-stage behaviour | Master behaviour |
|---|---|---|---|
| M1 | `persist` fails after the catalogue tx committed (WAL outcome unknown, `WalDurability` turned into `Ok` and then the flush fails, flush error) | Source reattached and unfrozen, `Err` returned. Writes and DDL accepted by `a` afterwards are lost on restart (boot registers `b` on the stores copied at the failure) or on a retry that takes the recovery branch (switch without re-copy). | Identical |
| M2 | After M1, `b` is free in memory | Create / drop / rename touching `b` can overwrite or remove the committed destination row. | Identical |
| M3 | Rename future cancelled during `persist` | As M1 (abort on drop). | Identical (no freeze at all) |
| M4 | Repo rename during M1 | The committed destination row can be stranded (revision-1 R9 analysis). | Identical |
| M5 | Tx staged through a pre-rename clone, committed after a successful rename | Not gated (the token no longer resolves); rows land in orphaned source stores. | Identical (also DROP TABLE) |
| M6 | Wire code of rename errors | `code: None` (`admin_table_index.rs:255-259,294`). | Identical |
| M7 | `RecordCounter` writes through a stale handle | Land in the orphaned `__info__`; metric only. | Identical |

M1 explicitly remains a data-loss window: after the catalogue transaction may have committed but durability flush fails, abort restores source usability although destination registration may persist; subsequent source mutations can be absent from copied destination stores. No lossless recovery claim is made.

### S6 — deferred to 03d

R2 name fence and mirror; `RenamePersistOutcome` and three-state outcome; R5 phases and into-doubt-on-drop; R6, R7, R9 repo-rename guard, R10 wire code, R11; original decisions 2/3/5/6/7 (catalogue split, `resolve_table_rename`, retry convergence, durability-tail seam); on-disk restart test; M5 stale pre-rename transaction fix; stage-before-staging vector shape from stop-E analysis. No production implementation of these items is claimed.

No positive parked-commit test variant is required; no commit-pipeline seam was added. No positive parked-commit test variant is required; no commit-pipeline seam was added. The new production visibility timeout and test timeout have not been exercised locally. All tests, format and CI remain unverified.

## Audit append — correction and authoritative revision-2 status

The original revision-1 final paragraph has been restored verbatim at line 145. Revision-1 progress items 1 and 2 remain at lines 123–124; they are historical revision-1 status, not the current stage status. Current revision-2 progress is recorded under `## Progress (revision 2)` beginning at line 146. The first `## Design (revision 2)` beginning at line 154 is the canonical added implementation record; repeated identical copies later in this file resulted from an append-edit mistake and are preserved rather than deleted under the no-deletion instruction. No pre-existing sections were removed by this documentation correction.

Acceptance 3–9 test files changed: `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs`, `crates/shamir-engine/src/repo/tests/mod.rs`, and `crates/shamir-engine/src/repo/tests/rename/{mod.rs,freeze_tests.rs,visibility_wait_tests.rs,cancel_tests.rs}`. Current test names are listed under Item 5 at line 213. The abort error assertion is variant/message matching, not equality. The visibility test uses a bounded receipt because strict insert waits on the held version; it now verifies the row and attachment, then performs `create_index` while retaining the unresolved version guard. Parent verified `open_snapshot_at` does not wait for settled-floor progress, so this is not expected to block for that reason. The multi-thread writer loops until the first error rather than using a fixed iteration cap; its whole loop is bounded at 30 seconds, with rename/rejection, start signal, join, setup and destination reads also bounded. Early start/rename failures abort and reap the writer before panic. No commands, tests, formatter, layout script, build, or CI were run; full end-to-end review and CI remain outstanding.

Layout observations are from file-tool listing only: the unauthorized unmounted `write_domain_core.rs` remains untouched, and its directory showed four entries, under the seven-entry limit. No Rust test, formatter, layout script, build, or CI was run. No positive parked-commit test variant is required. Acceptance/test code and S4/S5 implementation still require parent review and CI.

- Item 5 follow-up: fixed the three sidecar-map `contains_key` calls in `touched_table_tokens` to dereference filter keys; preserved the index-op token reference and restored the blank separator before `Ok(PreLockResult)`. Concurrent writer acceptance now asserts destination count equals captured successful IDs; `count()` returns `usize` (`table_manager_crud.rs:698-701`). Spawned writer has a 30-second bound and is joined through `&mut JoinHandle`, with abort/reap on start/rename failure. No commands or tests run.
## Phase 0 (revision 2)

### Q11 — permit waits after version allocation; H is not established

**Observed ordering:** ordinary single-row CRUD does not allocate a repository MVCC version and then request the repository write permit: `TableManager::insert_unacknowledged` enters writer-drain / optional `unique_write_lock` at `crates/shamir-engine/src/table/table_manager/operations/table_manager_crud.rs:206-223`, then writes through MVCC at `:244-258`; the other CRUD families similarly check freeze before data mutation (`:318-332,499-515,593-609`). The transactional batch CRUD routes through an implicit transaction rather than allocating its own version (`:421-430`). `TableManager::admit` is explicitly documented as DDL/registry-flush-only and not called by leader write/commit (`crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:34-44`). Thus there is no ordinary CRUD path evidenced here that can hold its own allocated gate version while waiting on `admit()`.

The leader commit allocator is reached only after validation/claim and the empty-tx fast path, at `crates/shamir-engine/src/tx/preparation/pre_commit/locked.rs:150-170` (and `:333-353` for the alternate path). Thereafter it builds WAL ops and starts WAL, not a repository permit (`:173-203`; `:355-405`). Its version guard settles/materialization path is handled by the commit pipeline; no commit code was found acquiring `write_domain.admit()` after allocation. This supports refuting H for ordinary CRUD and leader tx commits, without claiming a universal proof from a search inventory.

**Candidate analysis:** follower receipt application does acquire the repository permit (`crates/shamir-engine/src/tx/replication/follower_durability/receipt/apply.rs:95-147`), but allocation occurs later, after admission and setup (`:349-379`); its worker ownership and parked-guard policy are documented in `crates/shamir-engine/src/tx/replication/follower_durability/receipt/resume/worker.rs:6-33`. It therefore does not evidence a permit wait after allocation. The tx-gate initializer does store/WAL/provenance reads but no write-domain admission (`crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs:51-88`; provenance helper `crates/shamir-engine/src/repo/meta/repl/gate_provenance.rs:15-27`). `PersistRegistry::flush_all` merely awaits registered `Persistable`s (`crates/shamir-engine/src/table/support/storage/persistable.rs:22-63`); the registered interner and counter are installed at `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:147-154`, and `flush_metadata` is a separate explicit facade (`crates/shamir-engine/src/table/table_manager/accessors/table_manager_validators.rs:387-395`), not a post-allocation commit step. In particular `RecordCounter::persist` can admit when dirty (`crates/shamir-engine/src/table/support/storage/record_counter.rs:321-336`), but that is a separate flush call; no inspected call chain places it after a version allocation and before settling that version. The orphan replay barrier takes `begin_write_barrier` (`crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:607-624`), but this operation does not request repository permit: its barrier implementation uses the admitted primitive directly (`crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:51-87`).

**H verdict:** Stop condition H is **not established / refuted for ordinary CRUD and normal tx commit paths**: neither family necessarily waits for the permit after allocation, and the receipt worker acquires before allocation. This is source analysis, not runtime proof. A parked version can still make the S4 visibility wait time out by design; that is distinct from proving every ordinary write/commit blocks on the permit. No production/test edits are authorized by Phase 0.

### Q12 — matching tests inspected; no stale-handle success assertion found

The in-scope engine search for `rename_table_stores` / `rename_table_as` found these tests (and the `catalog.rs` implementation):

- `crates/shamir-engine/src/repo/tests/core/repo_instance_tests.rs:37-55`, test `rename_membuffer_wrapped_table_flushes_copied_history`: it resolves `source` before rename (`:39-44`), but performs no write or DDL through that old handle after rename; it reads through a newly resolved destination (`:51-55`). No change.
- `crates/shamir-engine/src/tx/replication/follower_durability/receipt/tests/ddl_interlock_tests.rs:84-167`, test `parked_apply_blocks_ddl_via_the_write_domain`: its rename of `plain` occurs at `:125-151`; no pre-rename table handle is retained or mutated after the rename. No change.

Also read the full existing `migration_freeze_tests.rs` (lines 1-308): `r6_freeze_waits_for_parked_fast_committer_and_observes_row` (`:40-78`), `r7_second_freeze_rejected_without_clearing_first_owner` (`:80-98`), `r8_unique_slow_committer_rechecks_freeze_under_lock` (`:100-136`), `cancelled_freeze_clears_bit_at_both_preparation_awaits` (`:138-166`), `implicit_commit_maps_freeze_to_table_migrating` (`:168-185`), `r8_non_tx_crud_queued_before_freeze_rejects_without_mutation` (`:187-257`), and `tx_error_conversion_preserves_error_categories` (`:259-308`). These test a migration freeze, not post-successful-rename stale-handle behavior, so none requires a Q12 edit.

**Q12 conclusion:** among the matching tests inspected, none writes or runs DDL through a handle resolved before rename and expects success afterward. Therefore no Q12 test change is currently identified. The prompt's literal requested exhaustive inventory is `rg -ln` across `crates`; only file tools were allowed, and available grep reports matching locations but not a guaranteed exhaustive list for a compound expression spanning every test module. Tests requiring Q12 change: **none identified**, not a claim that unrelated DB-level test code was exhaustively opened.

### Q13 — migration coordinator twins mutate self and destination

At `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/admitted_ops.rs:245-275`, `migration_coordinator_admitted` checks the source permit against `self` and the destination permit against `dst` (`:254-255`), then constructs a coordinator from both source (`self`) and destination (`dst`) stores (`:260-268`). Under S2 both managers need attachment checks: one on `self`, one on `dst`; checks against the permits alone do not establish attachment.

At `admitted_ops.rs:285-312`, `migration_coordinator_recovered_admitted` also validates both domains (`:292-293`) and captures both managers' data stores (`:300-303`). Despite the recovery factory not taking a new freeze (`:277-284,305-307`), it mutates/retains coordinator state for both source and destination, so S2 likewise applies to both `self` and `dst`, not only the receiver.
## Progress (revision 2)

- Item 1: **DONE — Phase 0 Q11–Q13** recorded above; H was not established (source analysis only).
- Item 2: **DONE — S1 and its three sidecar-commit tests** recorded at line 209 above.
- Item 3: **PARTIAL — S2/S3** as described at line 177; parent-reported correction review accepted core receiver/duplicate-check corrections. No tests/builds run.
- Item 4: **DONE — S4/S5** as described at lines 210–212. No tests/builds run.
- Item 5: **PARTIAL — acceptance tests 3–9** as listed at line 213 and in the appended test note below. Rust review/format/CI remain.

## Design (revision 2) — implementation record and remaining scope

### S1 — prelock token union

`prelock.rs` now derives the touched-token set from `write_set`, `index_write_set`, `counter_deltas`, `staged_vectors`, and `staged_vector_deletes`, then sorts/deduplicates. The loop retains existing writer-drain/slow-path logic. Tests recorded at line 209 check frozen index-only and counter-only commits plus an unfrozen index-only control. No runtime result is claimed.

### S2 — detach gate

`TableManager::detached` is `pub(super)` and shared through Clone; constructors initialize false. `detach_for_rename` / `reattach_after_rename` store Release; `ensure_attached` loads Acquire and returns `MigrationFrozen`. `admit` gates public wrappers; explicit gates are in the 24 brief-listed admitted methods, including both migration coordinator twins on source and destination, plus `begin_write_barrier_with_permit`. CRUD/commit do not read detached.

The explicit methods are `add_validator_binding_admitted`, `remove_validator_binding_admitted`, `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, `migration_coordinator_admitted`, `migration_coordinator_recovered_admitted`, `rename_index_admitted`, `create_index_v2_admitted`, `create_index_admitted`, `create_unique_index_admitted`, `drop_index_admitted`, `drop_unique_index_admitted`, `drop_index2_admitted`, `repair_admitted`, `set_buffer_config_admitted`, `replicate_interner_from_admitted`, `replicate_index2_descriptors_from_admitted`, `seed_log_from_data_store_admitted`, `bulk_populate_index2_admitted`, `create_sorted_index_with_include_admitted`, `drop_sorted_index_admitted`, and `begin_write_barrier_with_permit`.

**Exemptions (12 named APIs/types and reasons):** (1) `begin_write_barrier_admitted`, (2) `begin_write_barrier_admitted_impl` — infallible; callers resolve the table under the same permit or are checked twins; (3) `begin_migration_freeze_admitted` — the freeze bit rejects a second claimant; (4) `create_with_domain_admitted`, (5) `open_with_domain_admitted` — construction/open; (6) `recover_hash_renames_admitted`, (7) `rerun_identity_migration_admitted` — open-time recovery/identity setup; (8) `persist_interner_admitted`, (9) `intern_query_value_admitted` — repo-shared interner, not a table store; (10) `RecordCounter::set_admitted`, (11) `RecordCounter::set_to_admitted`, (12) `RecordCounter::persist_admitted` — separate metric/counter APIs; counter persistence is intentionally called on the detached source during rename preparation. No visibility of existing items was widened by S2.

Rejected alternatives: holding `ddl_admission` across the attempt can deadlock; checking `MIGRATION_FREEZE` in DDL would block migration coordinator DDL on its frozen source (`admin_migration.rs:460`); adding a barrier bit is unavailable because the u8 selector space is full. Unmounted accidental file `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_core.rs` remains untouched per no-delete instruction; current directory listing had four entries, not more than the seven-entry cap. This file is not mounted.

### S3 — guard retirement

`MigrationFreezeGuard` now has private `armed`; `try_set` arms it, `retire` disarms it, and `Drop` clears only while armed. The acceptance tests named above were added, not run.

### S4 — rename freeze RAII and visibility wait

New crate-private type/methods: `RenameFreeze::{begin, await_visible, source, retire}` and `Drop`. It begins the admitted freeze, detaches the source, bounds gate initialization plus high-water visibility wait, matches `Visible` and `ReopenRequired` exhaustively, returns the underlying gate error or a timeout `Conflict`, and on abort/cancel reattaches before explicitly dropping its freeze guard. Successful `retire` keeps source detached/frozen. No existing item visibility was widened.

### S5 — transition integration

`rename_table_transition` and `rename_table_stores_admitted` precheck configuration after admission, resolve source with that permit, freeze and await visibility before prepare, drain history, flush the counter, copy, persist and commit, then retire only on success. Private `prepare_table_rename_admitted` now takes `&RenameFreeze`; its callers remain within `catalog.rs`. `commit_table_rename_admitted` has no freeze parameter. Recovery freezes only when source identity matches, before persistence, and retires after successful source removal; it does not await visibility or flush the counter because it does not copy stores. Public rename/recovery signatures remain unchanged. Counter source review: increment is cache-only (`record_counter.rs:149-185`); dirty `persist_admitted` writes through (`:223-243`), so the counter test is expected to distinguish lazy persistence on master, but this is not a runtime result.

### Test evidence and clarification

The held-version visibility test cannot use strict `insert`: `insert_returning_version` calls `write_receipt` with `VisibilityWait::Strict` (`table_manager/admission/crud.rs:101-110`), which awaits the unsettled version. It instead uses `insert_with_receipt` with a bounded wait, asserts a pending receipt and reads back the row while keeping the version guard alive, then checks admitted buffer-config DDL. Parent source review established `open_index_build_snapshot` uses `snapshot_at` (`shamir-tx/src/mvcc_store/gate.rs:492-505`), which pins without waiting for the settled floor. Therefore the earlier inference that `create_index` must block behind a held gate guard was incorrect; no such claim is made. The test uses buffer-config DDL as a writable probe, a deviation from the brief's suggested index-create probe, for parent review. No test was run.

### M1–M7 remaining failure modes (brief table)

| # | Situation | Sub-stage behaviour | Master behaviour |
|---|---|---|---|
| M1 | `persist` fails after the catalogue tx committed (WAL outcome unknown, `WalDurability` turned into `Ok` and then the flush fails, flush error) | Source reattached and unfrozen, `Err` returned. Writes and DDL accepted by `a` afterwards are lost on restart (boot registers `b` on the stores copied at the failure) or on a retry that takes the recovery branch (switch without re-copy). | Identical |
| M2 | After M1, `b` is free in memory | Create / drop / rename touching `b` can overwrite or remove the committed destination row. | Identical |
| M3 | Rename future cancelled during `persist` | As M1 (abort on drop). | Identical (no freeze at all) |
| M4 | Repo rename during M1 | The committed destination row can be stranded (revision-1 R9 analysis). | Identical |
| M5 | Tx staged through a pre-rename clone, committed after a successful rename | Not gated (the token no longer resolves); rows land in orphaned source stores. | Identical (also DROP TABLE) |
| M6 | Wire code of rename errors | `code: None` (`admin_table_index.rs:255-259,294`). | Identical |
| M7 | `RecordCounter` writes through a stale handle | Land in the orphaned `__info__`; metric only. | Identical |

M1 explicitly remains a data-loss window: after the catalogue transaction may have committed but durability flush fails, abort restores source usability although destination registration may persist; subsequent source mutations can be absent from copied destination stores. No lossless recovery claim is made.

### S6 — deferred to 03d

R2 name fence and mirror; `RenamePersistOutcome` and three-state outcome; R5 phases and into-doubt-on-drop; R6, R7, R9 repo-rename guard, R10 wire code, R11; original decisions 2/3/5/6/7 (catalogue split, `resolve_table_rename`, retry convergence, durability-tail seam); on-disk restart test; M5 stale pre-rename transaction fix; stage-before-staging vector shape from stop-E analysis. No production implementation of these items is claimed.

No positive parked-commit test variant is required; no commit-pipeline seam was added. No positive parked-commit test variant is required; no commit-pipeline seam was added. The new production visibility timeout and test timeout have not been exercised locally. All tests, format and CI remain unverified.

## Audit append — correction and authoritative revision-2 status

The original revision-1 final paragraph has been restored verbatim at line 145. Revision-1 progress items 1 and 2 remain at lines 123–124; they are historical revision-1 status, not the current stage status. Current revision-2 progress is recorded under `## Progress (revision 2)` beginning at line 146. The first `## Design (revision 2)` beginning at line 154 is the canonical added implementation record; repeated identical copies later in this file resulted from an append-edit mistake and are preserved rather than deleted under the no-deletion instruction. No pre-existing sections were removed by this documentation correction.

Acceptance 3–9 test files changed: `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs`, `crates/shamir-engine/src/repo/tests/mod.rs`, and `crates/shamir-engine/src/repo/tests/rename/{mod.rs,freeze_tests.rs,visibility_wait_tests.rs,cancel_tests.rs}`. Current test names are listed under Item 5 at line 213. The abort error assertion is variant/message matching, not equality. The visibility test uses a bounded receipt because strict insert waits on the held version; it now verifies the row and attachment, then performs `create_index` while retaining the unresolved version guard. Parent verified `open_snapshot_at` does not wait for settled-floor progress, so this is not expected to block for that reason. The multi-thread writer loops until the first error rather than using a fixed iteration cap; its whole loop is bounded at 30 seconds, with rename/rejection, start signal, join, setup and destination reads also bounded. Early start/rename failures abort and reap the writer before panic. No commands, tests, formatter, layout script, build, or CI were run; full end-to-end review and CI remain outstanding.

Layout observations are from file-tool listing only: the unauthorized unmounted `write_domain_core.rs` remains untouched, and its directory showed four entries, under the seven-entry limit. No Rust test, formatter, layout script, build, or CI was run. No positive parked-commit test variant is required. Acceptance/test code and S4/S5 implementation still require parent review and CI.

- Item 5 follow-up: fixed the three sidecar-map `contains_key` calls in `touched_table_tokens` to dereference filter keys; preserved the index-op token reference and restored the blank separator before `Ok(PreLockResult)`. Concurrent writer acceptance now asserts destination count equals captured successful IDs; `count()` returns `usize` (`table_manager_crud.rs:698-701`). Spawned writer has a 30-second bound and is joined through `&mut JoinHandle`, with abort/reap on start/rename failure. No commands or tests run.
- Item 3: **PARTIAL — S2/S3 edits in progress; not fully reviewed or tested.** Added the `detached` lifecycle field/checks and S3 armed retirement in the authorized files. The 24 specified explicit checks were found in the intended mounted files; both migration twins validate source and destination permit domains and attachment. The five required checks are in the mounted `write_domain_index_core.rs`; its unique-index twin retains both duplicate permit checks. Public `begin_write_barrier` relies on `admit()` for attachment. Exemptions: the two admitted barrier helpers are infallible and callers resolve under the permit; freeze acquisition is protected by the freeze bit; create/open and recovery methods are construction/open-time work; interner persistence/interning mutate repo-shared state; `RecordCounter::{set_admitted, set_to_admitted, persist_admitted}` are metric APIs, with persistence intentionally used on detached source. Rejected alternatives: hold `ddl_admission` across attempt (deadlock risk), check `MIGRATION_FREEZE` in DDL (would reject migration coordinator DDL on its frozen source), or allocate a new barrier bit (selector space full). No commands, builds, or tests run. **Blocker:** an unauthorized, unmounted sibling `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_core.rs` is present and untouched; the directory has four entries, under the stated seven-entry cap, but that file’s content/state has not been modified here. S2/S3 completion is not claimed pending full end-to-end read-through and parent disposition of the stray file.
## Progress (revision 2)

- Item 1: **DONE — Phase 0 Q11–Q13** recorded above; H was not established (source analysis only).
- Item 2: **DONE — S1 and its three sidecar-commit tests** recorded at line 209 above.
- Item 3: **PARTIAL — S2/S3** as described at line 177; parent-reported correction review accepted core receiver/duplicate-check corrections. No tests/builds run.
- Item 4: **DONE — S4/S5** as described at lines 210–212. No tests/builds run.
- Item 5: **PARTIAL — acceptance tests 3–9** as listed at line 213 and in the appended test note below. Rust review/format/CI remain.

## Design (revision 2) — implementation record and remaining scope

### S1 — prelock token union

`prelock.rs` now derives the touched-token set from `write_set`, `index_write_set`, `counter_deltas`, `staged_vectors`, and `staged_vector_deletes`, then sorts/deduplicates. The loop retains existing writer-drain/slow-path logic. Tests recorded at line 209 check frozen index-only and counter-only commits plus an unfrozen index-only control. No runtime result is claimed.

### S2 — detach gate

`TableManager::detached` is `pub(super)` and shared through Clone; constructors initialize false. `detach_for_rename` / `reattach_after_rename` store Release; `ensure_attached` loads Acquire and returns `MigrationFrozen`. `admit` gates public wrappers; explicit gates are in the 24 brief-listed admitted methods, including both migration coordinator twins on source and destination, plus `begin_write_barrier_with_permit`. CRUD/commit do not read detached.

The explicit methods are `add_validator_binding_admitted`, `remove_validator_binding_admitted`, `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, `migration_coordinator_admitted`, `migration_coordinator_recovered_admitted`, `rename_index_admitted`, `create_index_v2_admitted`, `create_index_admitted`, `create_unique_index_admitted`, `drop_index_admitted`, `drop_unique_index_admitted`, `drop_index2_admitted`, `repair_admitted`, `set_buffer_config_admitted`, `replicate_interner_from_admitted`, `replicate_index2_descriptors_from_admitted`, `seed_log_from_data_store_admitted`, `bulk_populate_index2_admitted`, `create_sorted_index_with_include_admitted`, `drop_sorted_index_admitted`, and `begin_write_barrier_with_permit`.

**Exemptions (12 named APIs/types and reasons):** (1) `begin_write_barrier_admitted`, (2) `begin_write_barrier_admitted_impl` — infallible; callers resolve the table under the same permit or are checked twins; (3) `begin_migration_freeze_admitted` — the freeze bit rejects a second claimant; (4) `create_with_domain_admitted`, (5) `open_with_domain_admitted` — construction/open; (6) `recover_hash_renames_admitted`, (7) `rerun_identity_migration_admitted` — open-time recovery/identity setup; (8) `persist_interner_admitted`, (9) `intern_query_value_admitted` — repo-shared interner, not a table store; (10) `RecordCounter::set_admitted`, (11) `RecordCounter::set_to_admitted`, (12) `RecordCounter::persist_admitted` — separate metric/counter APIs; counter persistence is intentionally called on the detached source during rename preparation. No visibility of existing items was widened by S2.

Rejected alternatives: holding `ddl_admission` across the attempt can deadlock; checking `MIGRATION_FREEZE` in DDL would block migration coordinator DDL on its frozen source (`admin_migration.rs:460`); adding a barrier bit is unavailable because the u8 selector space is full. Unmounted accidental file `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_core.rs` remains untouched per no-delete instruction; current directory listing had four entries, not more than the seven-entry cap. This file is not mounted.

### S3 — guard retirement

`MigrationFreezeGuard` now has private `armed`; `try_set` arms it, `retire` disarms it, and `Drop` clears only while armed. The acceptance tests named above were added, not run.

### S4 — rename freeze RAII and visibility wait

New crate-private type/methods: `RenameFreeze::{begin, await_visible, source, retire}` and `Drop`. It begins the admitted freeze, detaches the source, bounds gate initialization plus high-water visibility wait, matches `Visible` and `ReopenRequired` exhaustively, returns the underlying gate error or a timeout `Conflict`, and on abort/cancel reattaches before explicitly dropping its freeze guard. Successful `retire` keeps source detached/frozen. No existing item visibility was widened.

### S5 — transition integration

`rename_table_transition` and `rename_table_stores_admitted` precheck configuration after admission, resolve source with that permit, freeze and await visibility before prepare, drain history, flush the counter, copy, persist and commit, then retire only on success. Private `prepare_table_rename_admitted` now takes `&RenameFreeze`; its callers remain within `catalog.rs`. `commit_table_rename_admitted` has no freeze parameter. Recovery freezes only when source identity matches, before persistence, and retires after successful source removal; it does not await visibility or flush the counter because it does not copy stores. Public rename/recovery signatures remain unchanged. Counter source review: increment is cache-only (`record_counter.rs:149-185`); dirty `persist_admitted` writes through (`:223-243`), so the counter test is expected to distinguish lazy persistence on master, but this is not a runtime result.

### Test evidence and clarification

The held-version visibility test cannot use strict `insert`: `insert_returning_version` calls `write_receipt` with `VisibilityWait::Strict` (`table_manager/admission/crud.rs:101-110`), which awaits the unsettled version. It instead uses `insert_with_receipt` with a bounded wait, asserts a pending receipt and reads back the row while keeping the version guard alive, then checks admitted buffer-config DDL. Parent source review established `open_index_build_snapshot` uses `snapshot_at` (`shamir-tx/src/mvcc_store/gate.rs:492-505`), which pins without waiting for the settled floor. Therefore the earlier inference that `create_index` must block behind a held gate guard was incorrect; no such claim is made. The test uses buffer-config DDL as a writable probe, a deviation from the brief's suggested index-create probe, for parent review. No test was run.

### M1–M7 remaining failure modes (brief table)

| # | Situation | Sub-stage behaviour | Master behaviour |
|---|---|---|---|
| M1 | `persist` fails after the catalogue tx committed (WAL outcome unknown, `WalDurability` turned into `Ok` and then the flush fails, flush error) | Source reattached and unfrozen, `Err` returned. Writes and DDL accepted by `a` afterwards are lost on restart (boot registers `b` on the stores copied at the failure) or on a retry that takes the recovery branch (switch without re-copy). | Identical |
| M2 | After M1, `b` is free in memory | Create / drop / rename touching `b` can overwrite or remove the committed destination row. | Identical |
| M3 | Rename future cancelled during `persist` | As M1 (abort on drop). | Identical (no freeze at all) |
| M4 | Repo rename during M1 | The committed destination row can be stranded (revision-1 R9 analysis). | Identical |
| M5 | Tx staged through a pre-rename clone, committed after a successful rename | Not gated (the token no longer resolves); rows land in orphaned source stores. | Identical (also DROP TABLE) |
| M6 | Wire code of rename errors | `code: None` (`admin_table_index.rs:255-259,294`). | Identical |
| M7 | `RecordCounter` writes through a stale handle | Land in the orphaned `__info__`; metric only. | Identical |

M1 explicitly remains a data-loss window: after the catalogue transaction may have committed but durability flush fails, abort restores source usability although destination registration may persist; subsequent source mutations can be absent from copied destination stores. No lossless recovery claim is made.

### S6 — deferred to 03d

R2 name fence and mirror; `RenamePersistOutcome` and three-state outcome; R5 phases and into-doubt-on-drop; R6, R7, R9 repo-rename guard, R10 wire code, R11; original decisions 2/3/5/6/7 (catalogue split, `resolve_table_rename`, retry convergence, durability-tail seam); on-disk restart test; M5 stale pre-rename transaction fix; stage-before-staging vector shape from stop-E analysis. No production implementation of these items is claimed.

No positive parked-commit test variant is required; no commit-pipeline seam was added. No positive parked-commit test variant is required; no commit-pipeline seam was added. The new production visibility timeout and test timeout have not been exercised locally. All tests, format and CI remain unverified.

## Audit append — correction and authoritative revision-2 status

The original revision-1 final paragraph has been restored verbatim at line 145. Revision-1 progress items 1 and 2 remain at lines 123–124; they are historical revision-1 status, not the current stage status. Current revision-2 progress is recorded under `## Progress (revision 2)` beginning at line 146. The first `## Design (revision 2)` beginning at line 154 is the canonical added implementation record; repeated identical copies later in this file resulted from an append-edit mistake and are preserved rather than deleted under the no-deletion instruction. No pre-existing sections were removed by this documentation correction.

Acceptance 3–9 test files changed: `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs`, `crates/shamir-engine/src/repo/tests/mod.rs`, and `crates/shamir-engine/src/repo/tests/rename/{mod.rs,freeze_tests.rs,visibility_wait_tests.rs,cancel_tests.rs}`. Current test names are listed under Item 5 at line 213. The abort error assertion is variant/message matching, not equality. The visibility test uses a bounded receipt because strict insert waits on the held version; it now verifies the row and attachment, then performs `create_index` while retaining the unresolved version guard. Parent verified `open_snapshot_at` does not wait for settled-floor progress, so this is not expected to block for that reason. The multi-thread writer loops until the first error rather than using a fixed iteration cap; its whole loop is bounded at 30 seconds, with rename/rejection, start signal, join, setup and destination reads also bounded. Early start/rename failures abort and reap the writer before panic. No commands, tests, formatter, layout script, build, or CI were run; full end-to-end review and CI remain outstanding.

Layout observations are from file-tool listing only: the unauthorized unmounted `write_domain_core.rs` remains untouched, and its directory showed four entries, under the seven-entry limit. No Rust test, formatter, layout script, build, or CI was run. No positive parked-commit test variant is required. Acceptance/test code and S4/S5 implementation still require parent review and CI.

- Item 5 follow-up: fixed the three sidecar-map `contains_key` calls in `touched_table_tokens` to dereference filter keys; preserved the index-op token reference and restored the blank separator before `Ok(PreLockResult)`. Concurrent writer acceptance now asserts destination count equals captured successful IDs; `count()` returns `usize` (`table_manager_crud.rs:698-701`). Spawned writer has a 30-second bound and is joined through `&mut JoinHandle`, with abort/reap on start/rename failure. No commands or tests run.
## Phase 0 (revision 2) errata — corrected Q11/Q12 inventory

This errata supplements the preceding Phase 0 text; it does not erase or rewrite the earlier observations. The earlier Q11 statement that orphan replay's public `begin_write_barrier` did not request repository admission was incorrect. The earlier Q12 conclusion was based on an incomplete inventory. These corrections supersede those specific statements.

### Q11 correction — orphan replay can hold an allocated version while waiting for permit; H remains not universal

`drain_step.rs:607-624` identifies orphan posting replay and calls `tbl.begin_write_barrier(SCHEMA_ACTIVATION)` at `:615-620`. Contrary to the earlier note, this public wrapper admits: `ddl_barriers.rs:49-55` calls `self.admit().await?` before its barrier sequence, and `TableManager::admit` obtains the repo permit (`write_domain_barrier.rs:39-44`). Therefore orphan replay can indeed wait for the repository permit while processing already allocated/unsettled WAL versions. If a rename owns that permit and awaits visibility of such a version, this is a plausible obstruction until the bounded visibility wait expires. The `drain_step.rs:607-614` comments also establish that replay holds no commit lock or map guard across the await; they do not negate this permit wait.

This candidate does not establish H. H asserts every ordinary CRUD write or every transaction commit on a table without in-flight DDL takes that path. The permit-waiting site is conditional orphan replay, not universal ordinary write/commit behavior. Ordinary CRUD code has no post-allocation repo admission in the inspected sequence: `table_manager_crud.rs:206-258` checks/drains before writing, and `RecordCounter::increment` is a local counter update (parent independently verified it does not admit; the separate persistence API is `record_counter.rs:321-336`). Main review also independently checked normal commit materialization settles before cleanup and that `gate_provenance` only reads provenance (`gate_provenance.rs:15-27`); the gate initializer itself reads stores/WAL and has no permit call (`transactions.rs:51-88`). The follower receipt path explicitly admits before allocation (`receipt/apply.rs:95-147,349-379`), so it is not evidence of the post-allocation wait candidate.

**Corrected verdict:** H is **not established**: a conditional orphan replay wait is possible and may force a visibility timeout, but it does not show that every ordinary CRUD write or every transaction commit on a table without in-flight DDL must wait for the permit. This is source analysis only, not runtime proof. The timeout remains material to S4.

### Q12 correction — complete parent-supplied matching-file inventory read

The parent supplied the result of its repository-wide `rg -l` inventory. Every listed file was opened/read in full (using successive view ranges for long files). Findings about pre-rename handles and rename outcomes:

1. `crates/shamir-engine/src/repo/tests/core/repo_instance_tests.rs` (725 lines): `rename_membuffer_wrapped_table_flushes_copied_history` resolves and writes through `source` before rename (`:37-49`), then resolves destination afresh and reads it (`:51-55`). It does not mutate the old source handle after success. No stale-handle success assertion.
2. `crates/shamir-engine/src/tx/replication/follower_durability/receipt/tests/ddl_interlock_tests.rs` (227 lines): `parked_apply_blocks_ddl_via_the_write_domain` renames `plain` while receipt application owns the permit (`:84-167`); it does not resolve or retain a pre-rename `plain` handle, nor write/DDL through one after rename. Remaining test covers paused reservation (`:174-227`), unrelated. No stale-handle success assertion.
3. `crates/shamir-db/tests/rename_repo_e2e/main.rs` (277 lines): tests logical repository re-key, data and index reads/inserts via repo names, destination conflict and missing source (`:53-277`). It explicitly says repo rename uses no `rename_table_stores` (`:10-13`). No table handle retained across table rename; no stale-handle assertion.
4. `crates/shamir-db/tests/rename_migration/renames/rename_table_e2e.rs` (323 lines): empty and populated table rename coverage, subsequent old-name inserts fail and new-name inserts/queries succeed (`:64-137,224-323`); destination-exists and schema-bearing refusals (`:139-214`). These are name-based API operations, not writes/DDL through a pre-rename `TableManager`. No stale-handle success assertion.
5. `crates/shamir-db/src/shamir_db/shamir_db/tests/catalogue/unique_store_names_tests.rs` (617 lines): `drop_recreate_separates_store_incarnations` uses an old handle only before drop (`:41-91`). `rename_recreate_and_swap_names_preserves_each_incarnation` writes before rename and checks the renamed/recreated names afterward (`:93-157`), without using its old manager after successful rename. `durability_tail_failure_retry_reconciles_registration_and_is_idempotent` uses a retained source handle after a *failed* rename, before retry, to verify source rows (`:160-250`); this is not post-success stale-handle behavior. `precommit_persist_failure_retry_uses_fresh_destination_id` likewise checks source after failed persist (`:252-318`). The remaining tests cover source/destination identity conflicts, stale catalogue rows and idless legacy retry (`:320-575), not a post-success write/DDL through an old handle. No Q12 change required.
6. `crates/shamir-db/src/shamir_db/shamir_db/tests/catalogue/catalogue_rename_order_tests.rs` (820 lines): the entire file tests catalogue-write ordering/error propagation, function/validator and database/repository renames, creation/removal, and duplicate DB registration (`:1-820`). The table-rename failure test injects destination-save failure and checks the source catalogue row remains (`:332-386`); it does not retain a pre-rename table handle or expect stale-handle mutation after successful rename. No Q12 change required.
7. `crates/shamir-tx/src/mvcc_store/tests/write_tests/drain_tests.rs` (589 lines): unit coverage of overlay/history drain, durable watermark and drain lock/error retry (`:1-589`). It discusses rename as a caller of `drain_to_history` (`:1-5,220-231`), but does not call a table-rename API or use a pre-rename manager. In particular its allocated visible-but-undurable guard setup and drain retry (`:532-590`) are local MVCC unit cases, not successful rename stale-handle writes. No Q12 change required.

**Corrected Q12 verdict:** complete against the parent-provided seven-file matching inventory: **no test asserts successful post-rename write or DDL through a handle resolved before the rename; no Q12 test changes are required.** This is distinct from saying stale-handle behavior is tested; none of these seven files supplies that new acceptance coverage.

## Progress (revision 2) errata
## Progress (revision 2)

- Item 1: **DONE — Phase 0 Q11–Q13** recorded above; H was not established (source analysis only).
- Item 2: **DONE — S1 and its three sidecar-commit tests** recorded at line 209 above.
- Item 3: **PARTIAL — S2/S3** as described at line 177; parent-reported correction review accepted core receiver/duplicate-check corrections. No tests/builds run.
- Item 4: **DONE — S4/S5** as described at lines 210–212. No tests/builds run.
- Item 5: **PARTIAL — acceptance tests 3–9** as listed at line 213 and in the appended test note below. Rust review/format/CI remain.

## Design (revision 2) — implementation record and remaining scope

### S1 — prelock token union

`prelock.rs` now derives the touched-token set from `write_set`, `index_write_set`, `counter_deltas`, `staged_vectors`, and `staged_vector_deletes`, then sorts/deduplicates. The loop retains existing writer-drain/slow-path logic. Tests recorded at line 209 check frozen index-only and counter-only commits plus an unfrozen index-only control. No runtime result is claimed.

### S2 — detach gate

`TableManager::detached` is `pub(super)` and shared through Clone; constructors initialize false. `detach_for_rename` / `reattach_after_rename` store Release; `ensure_attached` loads Acquire and returns `MigrationFrozen`. `admit` gates public wrappers; explicit gates are in the 24 brief-listed admitted methods, including both migration coordinator twins on source and destination, plus `begin_write_barrier_with_permit`. CRUD/commit do not read detached.

The explicit methods are `add_validator_binding_admitted`, `remove_validator_binding_admitted`, `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, `migration_coordinator_admitted`, `migration_coordinator_recovered_admitted`, `rename_index_admitted`, `create_index_v2_admitted`, `create_index_admitted`, `create_unique_index_admitted`, `drop_index_admitted`, `drop_unique_index_admitted`, `drop_index2_admitted`, `repair_admitted`, `set_buffer_config_admitted`, `replicate_interner_from_admitted`, `replicate_index2_descriptors_from_admitted`, `seed_log_from_data_store_admitted`, `bulk_populate_index2_admitted`, `create_sorted_index_with_include_admitted`, `drop_sorted_index_admitted`, and `begin_write_barrier_with_permit`.

**Exemptions (12 named APIs/types and reasons):** (1) `begin_write_barrier_admitted`, (2) `begin_write_barrier_admitted_impl` — infallible; callers resolve the table under the same permit or are checked twins; (3) `begin_migration_freeze_admitted` — the freeze bit rejects a second claimant; (4) `create_with_domain_admitted`, (5) `open_with_domain_admitted` — construction/open; (6) `recover_hash_renames_admitted`, (7) `rerun_identity_migration_admitted` — open-time recovery/identity setup; (8) `persist_interner_admitted`, (9) `intern_query_value_admitted` — repo-shared interner, not a table store; (10) `RecordCounter::set_admitted`, (11) `RecordCounter::set_to_admitted`, (12) `RecordCounter::persist_admitted` — separate metric/counter APIs; counter persistence is intentionally called on the detached source during rename preparation. No visibility of existing items was widened by S2.

Rejected alternatives: holding `ddl_admission` across the attempt can deadlock; checking `MIGRATION_FREEZE` in DDL would block migration coordinator DDL on its frozen source (`admin_migration.rs:460`); adding a barrier bit is unavailable because the u8 selector space is full. Unmounted accidental file `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_core.rs` remains untouched per no-delete instruction; current directory listing had four entries, not more than the seven-entry cap. This file is not mounted.

### S3 — guard retirement

`MigrationFreezeGuard` now has private `armed`; `try_set` arms it, `retire` disarms it, and `Drop` clears only while armed. The acceptance tests named above were added, not run.

### S4 — rename freeze RAII and visibility wait

New crate-private type/methods: `RenameFreeze::{begin, await_visible, source, retire}` and `Drop`. It begins the admitted freeze, detaches the source, bounds gate initialization plus high-water visibility wait, matches `Visible` and `ReopenRequired` exhaustively, returns the underlying gate error or a timeout `Conflict`, and on abort/cancel reattaches before explicitly dropping its freeze guard. Successful `retire` keeps source detached/frozen. No existing item visibility was widened.

### S5 — transition integration

`rename_table_transition` and `rename_table_stores_admitted` precheck configuration after admission, resolve source with that permit, freeze and await visibility before prepare, drain history, flush the counter, copy, persist and commit, then retire only on success. Private `prepare_table_rename_admitted` now takes `&RenameFreeze`; its callers remain within `catalog.rs`. `commit_table_rename_admitted` has no freeze parameter. Recovery freezes only when source identity matches, before persistence, and retires after successful source removal; it does not await visibility or flush the counter because it does not copy stores. Public rename/recovery signatures remain unchanged. Counter source review: increment is cache-only (`record_counter.rs:149-185`); dirty `persist_admitted` writes through (`:223-243`), so the counter test is expected to distinguish lazy persistence on master, but this is not a runtime result.

### Test evidence and clarification

The held-version visibility test cannot use strict `insert`: `insert_returning_version` calls `write_receipt` with `VisibilityWait::Strict` (`table_manager/admission/crud.rs:101-110`), which awaits the unsettled version. It instead uses `insert_with_receipt` with a bounded wait, asserts a pending receipt and reads back the row while keeping the version guard alive, then checks admitted buffer-config DDL. Parent source review established `open_index_build_snapshot` uses `snapshot_at` (`shamir-tx/src/mvcc_store/gate.rs:492-505`), which pins without waiting for the settled floor. Therefore the earlier inference that `create_index` must block behind a held gate guard was incorrect; no such claim is made. The test uses buffer-config DDL as a writable probe, a deviation from the brief's suggested index-create probe, for parent review. No test was run.

### M1–M7 remaining failure modes (brief table)

| # | Situation | Sub-stage behaviour | Master behaviour |
|---|---|---|---|
| M1 | `persist` fails after the catalogue tx committed (WAL outcome unknown, `WalDurability` turned into `Ok` and then the flush fails, flush error) | Source reattached and unfrozen, `Err` returned. Writes and DDL accepted by `a` afterwards are lost on restart (boot registers `b` on the stores copied at the failure) or on a retry that takes the recovery branch (switch without re-copy). | Identical |
| M2 | After M1, `b` is free in memory | Create / drop / rename touching `b` can overwrite or remove the committed destination row. | Identical |
| M3 | Rename future cancelled during `persist` | As M1 (abort on drop). | Identical (no freeze at all) |
| M4 | Repo rename during M1 | The committed destination row can be stranded (revision-1 R9 analysis). | Identical |
| M5 | Tx staged through a pre-rename clone, committed after a successful rename | Not gated (the token no longer resolves); rows land in orphaned source stores. | Identical (also DROP TABLE) |
| M6 | Wire code of rename errors | `code: None` (`admin_table_index.rs:255-259,294`). | Identical |
| M7 | `RecordCounter` writes through a stale handle | Land in the orphaned `__info__`; metric only. | Identical |

M1 explicitly remains a data-loss window: after the catalogue transaction may have committed but durability flush fails, abort restores source usability although destination registration may persist; subsequent source mutations can be absent from copied destination stores. No lossless recovery claim is made.

### S6 — deferred to 03d

R2 name fence and mirror; `RenamePersistOutcome` and three-state outcome; R5 phases and into-doubt-on-drop; R6, R7, R9 repo-rename guard, R10 wire code, R11; original decisions 2/3/5/6/7 (catalogue split, `resolve_table_rename`, retry convergence, durability-tail seam); on-disk restart test; M5 stale pre-rename transaction fix; stage-before-staging vector shape from stop-E analysis. No production implementation of these items is claimed.

No positive parked-commit test variant is required; no commit-pipeline seam was added. No positive parked-commit test variant is required; no commit-pipeline seam was added. The new production visibility timeout and test timeout have not been exercised locally. All tests, format and CI remain unverified.

## Audit append — correction and authoritative revision-2 status

The original revision-1 final paragraph has been restored verbatim at line 145. Revision-1 progress items 1 and 2 remain at lines 123–124; they are historical revision-1 status, not the current stage status. Current revision-2 progress is recorded under `## Progress (revision 2)` beginning at line 146. The first `## Design (revision 2)` beginning at line 154 is the canonical added implementation record; repeated identical copies later in this file resulted from an append-edit mistake and are preserved rather than deleted under the no-deletion instruction. No pre-existing sections were removed by this documentation correction.

Acceptance 3–9 test files changed: `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs`, `crates/shamir-engine/src/repo/tests/mod.rs`, and `crates/shamir-engine/src/repo/tests/rename/{mod.rs,freeze_tests.rs,visibility_wait_tests.rs,cancel_tests.rs}`. Current test names are listed under Item 5 at line 213. The abort error assertion is variant/message matching, not equality. The visibility test uses a bounded receipt because strict insert waits on the held version; it now verifies the row and attachment, then performs `create_index` while retaining the unresolved version guard. Parent verified `open_snapshot_at` does not wait for settled-floor progress, so this is not expected to block for that reason. The multi-thread writer loops until the first error rather than using a fixed iteration cap; its whole loop is bounded at 30 seconds, with rename/rejection, start signal, join, setup and destination reads also bounded. Early start/rename failures abort and reap the writer before panic. No commands, tests, formatter, layout script, build, or CI were run; full end-to-end review and CI remain outstanding.

Layout observations are from file-tool listing only: the unauthorized unmounted `write_domain_core.rs` remains untouched, and its directory showed four entries, under the seven-entry limit. No Rust test, formatter, layout script, build, or CI was run. No positive parked-commit test variant is required. Acceptance/test code and S4/S5 implementation still require parent review and CI.

- Item 5 follow-up: fixed the three sidecar-map `contains_key` calls in `touched_table_tokens` to dereference filter keys; preserved the index-op token reference and restored the blank separator before `Ok(PreLockResult)`. Concurrent writer acceptance now asserts destination count equals captured successful IDs; `count()` returns `usize` (`table_manager_crud.rs:698-701`). Spawned writer has a 30-second bound and is joined through `&mut JoinHandle`, with abort/reap on start/rename failure. No commands or tests run.
- Item 4: **DONE — S4/S5 implementation** in `crates/shamir-engine/src/repo/instance/repo_instance/catalog/rename_freeze.rs` and `catalog.rs`. `RenameFreeze` takes and owns the freeze, detaches source, awaits `VisibleOutcome` under bounded timeout, exposes source for counter flush, retires on successful transition, and restores attachment before explicitly dropping its guard on abort/cancellation. Normal transitions perform configured-source precheck, freeze/visibility wait before prepare, drain then counter flush then copy; both recovery branches freeze only on source-incarnation match before `persist` and retire only after source removal. The source counter is lazily persisted: increments are cache-only (`record_counter.rs:149-185`), dirty flush writes through (`:223-243`). No visibility widening; no commands/builds/tests run.

- Defect review follow-up: the abort test compares the exact `DbError::Storage` variant/message with `matches!` (no `PartialEq` assumption). The visibility timeout test retains the unresolved guard until its end; strict CRUD acknowledgement would block behind it, so it uses a bounded receipt and confirms the inserted row, then uses admitted buffer-config DDL as a non-snapshot-dependent writable-DDL probe. Its timeout is driven by awaiting the operation's own bounded timeout, not by polling a completed future. The multi-thread writer test now bounds each insert, uses a start signal to prove writer activity, waits for rejection via oneshot inside bounded rename, bounds rename and join, and asserts exact `MigrationFrozen { table: "a" }`. Cancellation recovery checks attachment explicitly and verifies CRUD plus index DDL can proceed.
** Tests: `retire_keeps_freeze_bit_after_drop`, `dropped_freeze_guard_clears_bit`; `successful_rename_rejects_stale_clone_crud_and_ddl`, `failed_rename_reattaches_and_unfreezes_source`, `writer_is_drained_before_successful_rename`, `rename_flushes_live_record_counter`; `visibility_wait_times_out_before_persist_and_restores_source`; `cancelled_persist_reattaches_and_unfreezes_source`, `cancelled_visibility_wait_reattaches_and_unfreezes_source`. Corrections: storage-error equality uses exact `matches!`; held-version test performs a bounded-receipt insert (and verifies pending receipt/value) instead of strict insert, and probes DDL with `set_buffer_config_admitted` because it is a non-snapshot-dependent writable DDL path; timeout is exercised by awaiting the rename directly, not a poll loop; concurrent writer uses bounded inserts, first-success/rejection oneshots, bounded outer rename/join; freeze errors assert table name. Test files were read individually, not all end-to-end after final correction. No commands/builds/tests run; parent review/format and CI validation remain. S6 deferrals remain for 03d: R2 name fence/mirror; `RenamePersistOutcome` and three-state outcome; R5 phases/into-doubt-on-drop; R6, R7, R9 repo-rename guard, R10 wire code, R11; decisions 2/3/5/6/7 (catalogue split, `resolve_table_rename`, retry convergence, durability-tail seam); on-disk restart test; M5 stale pre-rename transaction path; stage-before-staging vector shape.
## Progress (revision 2)

- Item 1: **DONE — Phase 0 Q11–Q13** recorded above; H was not established (source analysis only).
- Item 2: **DONE — S1 and its three sidecar-commit tests** recorded at line 209 above.
- Item 3: **PARTIAL — S2/S3** as described at line 177; parent-reported correction review accepted core receiver/duplicate-check corrections. No tests/builds run.
- Item 4: **DONE — S4/S5** as described at lines 210–212. No tests/builds run.
- Item 5: **PARTIAL — acceptance tests 3–9** as listed at line 213 and in the appended test note below. Rust review/format/CI remain.

## Design (revision 2) — implementation record and remaining scope

### S1 — prelock token union

`prelock.rs` now derives the touched-token set from `write_set`, `index_write_set`, `counter_deltas`, `staged_vectors`, and `staged_vector_deletes`, then sorts/deduplicates. The loop retains existing writer-drain/slow-path logic. Tests recorded at line 209 check frozen index-only and counter-only commits plus an unfrozen index-only control. No runtime result is claimed.

### S2 — detach gate

`TableManager::detached` is `pub(super)` and shared through Clone; constructors initialize false. `detach_for_rename` / `reattach_after_rename` store Release; `ensure_attached` loads Acquire and returns `MigrationFrozen`. `admit` gates public wrappers; explicit gates are in the 24 brief-listed admitted methods, including both migration coordinator twins on source and destination, plus `begin_write_barrier_with_permit`. CRUD/commit do not read detached.

The explicit methods are `add_validator_binding_admitted`, `remove_validator_binding_admitted`, `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, `migration_coordinator_admitted`, `migration_coordinator_recovered_admitted`, `rename_index_admitted`, `create_index_v2_admitted`, `create_index_admitted`, `create_unique_index_admitted`, `drop_index_admitted`, `drop_unique_index_admitted`, `drop_index2_admitted`, `repair_admitted`, `set_buffer_config_admitted`, `replicate_interner_from_admitted`, `replicate_index2_descriptors_from_admitted`, `seed_log_from_data_store_admitted`, `bulk_populate_index2_admitted`, `create_sorted_index_with_include_admitted`, `drop_sorted_index_admitted`, and `begin_write_barrier_with_permit`.

**Exemptions (12 named APIs/types and reasons):** (1) `begin_write_barrier_admitted`, (2) `begin_write_barrier_admitted_impl` — infallible; callers resolve the table under the same permit or are checked twins; (3) `begin_migration_freeze_admitted` — the freeze bit rejects a second claimant; (4) `create_with_domain_admitted`, (5) `open_with_domain_admitted` — construction/open; (6) `recover_hash_renames_admitted`, (7) `rerun_identity_migration_admitted` — open-time recovery/identity setup; (8) `persist_interner_admitted`, (9) `intern_query_value_admitted` — repo-shared interner, not a table store; (10) `RecordCounter::set_admitted`, (11) `RecordCounter::set_to_admitted`, (12) `RecordCounter::persist_admitted` — separate metric/counter APIs; counter persistence is intentionally called on the detached source during rename preparation. No visibility of existing items was widened by S2.

Rejected alternatives: holding `ddl_admission` across the attempt can deadlock; checking `MIGRATION_FREEZE` in DDL would block migration coordinator DDL on its frozen source (`admin_migration.rs:460`); adding a barrier bit is unavailable because the u8 selector space is full. Unmounted accidental file `crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_core.rs` remains untouched per no-delete instruction; current directory listing had four entries, not more than the seven-entry cap. This file is not mounted.

### S3 — guard retirement

`MigrationFreezeGuard` now has private `armed`; `try_set` arms it, `retire` disarms it, and `Drop` clears only while armed. The acceptance tests named above were added, not run.

### S4 — rename freeze RAII and visibility wait

New crate-private type/methods: `RenameFreeze::{begin, await_visible, source, retire}` and `Drop`. It begins the admitted freeze, detaches the source, bounds gate initialization plus high-water visibility wait, matches `Visible` and `ReopenRequired` exhaustively, returns the underlying gate error or a timeout `Conflict`, and on abort/cancel reattaches before explicitly dropping its freeze guard. Successful `retire` keeps source detached/frozen. No existing item visibility was widened.

### S5 — transition integration

`rename_table_transition` and `rename_table_stores_admitted` precheck configuration after admission, resolve source with that permit, freeze and await visibility before prepare, drain history, flush the counter, copy, persist and commit, then retire only on success. Private `prepare_table_rename_admitted` now takes `&RenameFreeze`; its callers remain within `catalog.rs`. `commit_table_rename_admitted` has no freeze parameter. Recovery freezes only when source identity matches, before persistence, and retires after successful source removal; it does not await visibility or flush the counter because it does not copy stores. Public rename/recovery signatures remain unchanged. Counter source review: increment is cache-only (`record_counter.rs:149-185`); dirty `persist_admitted` writes through (`:223-243`), so the counter test is expected to distinguish lazy persistence on master, but this is not a runtime result.

### Test evidence and clarification

The held-version visibility test cannot use strict `insert`: `insert_returning_version` calls `write_receipt` with `VisibilityWait::Strict` (`table_manager/admission/crud.rs:101-110`), which awaits the unsettled version. It instead uses `insert_with_receipt` with a bounded wait, asserts a pending receipt and reads back the row while keeping the version guard alive, then checks admitted buffer-config DDL. Parent source review established `open_index_build_snapshot` uses `snapshot_at` (`shamir-tx/src/mvcc_store/gate.rs:492-505`), which pins without waiting for the settled floor. Therefore the earlier inference that `create_index` must block behind a held gate guard was incorrect; no such claim is made. The test uses buffer-config DDL as a writable probe, a deviation from the brief's suggested index-create probe, for parent review. No test was run.

### M1–M7 remaining failure modes (brief table)

| # | Situation | Sub-stage behaviour | Master behaviour |
|---|---|---|---|
| M1 | `persist` fails after the catalogue tx committed (WAL outcome unknown, `WalDurability` turned into `Ok` and then the flush fails, flush error) | Source reattached and unfrozen, `Err` returned. Writes and DDL accepted by `a` afterwards are lost on restart (boot registers `b` on the stores copied at the failure) or on a retry that takes the recovery branch (switch without re-copy). | Identical |
| M2 | After M1, `b` is free in memory | Create / drop / rename touching `b` can overwrite or remove the committed destination row. | Identical |
| M3 | Rename future cancelled during `persist` | As M1 (abort on drop). | Identical (no freeze at all) |
| M4 | Repo rename during M1 | The committed destination row can be stranded (revision-1 R9 analysis). | Identical |
| M5 | Tx staged through a pre-rename clone, committed after a successful rename | Not gated (the token no longer resolves); rows land in orphaned source stores. | Identical (also DROP TABLE) |
| M6 | Wire code of rename errors | `code: None` (`admin_table_index.rs:255-259,294`). | Identical |
| M7 | `RecordCounter` writes through a stale handle | Land in the orphaned `__info__`; metric only. | Identical |

M1 explicitly remains a data-loss window: after the catalogue transaction may have committed but durability flush fails, abort restores source usability although destination registration may persist; subsequent source mutations can be absent from copied destination stores. No lossless recovery claim is made.

### S6 — deferred to 03d

R2 name fence and mirror; `RenamePersistOutcome` and three-state outcome; R5 phases and into-doubt-on-drop; R6, R7, R9 repo-rename guard, R10 wire code, R11; original decisions 2/3/5/6/7 (catalogue split, `resolve_table_rename`, retry convergence, durability-tail seam); on-disk restart test; M5 stale pre-rename transaction fix; stage-before-staging vector shape from stop-E analysis. No production implementation of these items is claimed.

No positive parked-commit test variant is required; no commit-pipeline seam was added. No positive parked-commit test variant is required; no commit-pipeline seam was added. The new production visibility timeout and test timeout have not been exercised locally. All tests, format and CI remain unverified.

## Audit append — correction and authoritative revision-2 status

The original revision-1 final paragraph has been restored verbatim at line 145. Revision-1 progress items 1 and 2 remain at lines 123–124; they are historical revision-1 status, not the current stage status. Current revision-2 progress is recorded under `## Progress (revision 2)` beginning at line 146. The first `## Design (revision 2)` beginning at line 154 is the canonical added implementation record; repeated identical copies later in this file resulted from an append-edit mistake and are preserved rather than deleted under the no-deletion instruction. No pre-existing sections were removed by this documentation correction.

Acceptance 3–9 test files changed: `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs`, `crates/shamir-engine/src/repo/tests/mod.rs`, and `crates/shamir-engine/src/repo/tests/rename/{mod.rs,freeze_tests.rs,visibility_wait_tests.rs,cancel_tests.rs}`. Current test names are listed under Item 5 at line 213. The abort error assertion is variant/message matching, not equality. The visibility test uses a bounded receipt because strict insert waits on the held version; it now verifies the row and attachment, then performs `create_index` while retaining the unresolved version guard. Parent verified `open_snapshot_at` does not wait for settled-floor progress, so this is not expected to block for that reason. The multi-thread writer loops until the first error rather than using a fixed iteration cap; its whole loop is bounded at 30 seconds, with rename/rejection, start signal, join, setup and destination reads also bounded. Early start/rename failures abort and reap the writer before panic. No commands, tests, formatter, layout script, build, or CI were run; full end-to-end review and CI remain outstanding.

Layout observations are from file-tool listing only: the unauthorized unmounted `write_domain_core.rs` remains untouched, and its directory showed four entries, under the seven-entry limit. No Rust test, formatter, layout script, build, or CI was run. No positive parked-commit test variant is required. Acceptance/test code and S4/S5 implementation still require parent review and CI.

- Item 5 follow-up: fixed the three sidecar-map `contains_key` calls in `touched_table_tokens` to dereference filter keys; preserved the index-op token reference and restored the blank separator before `Ok(PreLockResult)`. Concurrent writer acceptance now asserts destination count equals captured successful IDs; `count()` returns `usize` (`table_manager_crud.rs:698-701`). Spawned writer has a 30-second bound and is joined through `&mut JoinHandle`, with abort/reap on start/rename failure. No commands or tests run.

## Final report (revision 2)

1. **DONE — Phase 0 Q11–Q13.** Q11/H analysis and correction are recorded at lines 215–223 and 380–386; Q12 inventory/conclusion at lines 225–234 and 388–400; Q13 at lines 236–240. H did not fire; conclusions are source-based, not runtime results.
2. **PARTIAL — S1 plus acceptance-2 tests written.** Sidecar token union in `crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs` (helper near lines 796–831); `sidecar_index_only_commit_rejected_while_frozen`, `sidecar_counter_only_commit_rejected_while_frozen`, and `sidecar_index_only_commit_applies_when_not_frozen` in `crates/shamir-engine/src/table/tests/index_lifecycle/registry/migration_freeze_tests.rs`. Not executed or CI-validated.
3. **PARTIAL — S3 and acceptance-3 tests written.** `MigrationFreezeGuard::retire`/armed Drop in `crates/shamir-engine/src/table/support/guards/migration_freeze_guard.rs`; tests `retire_keeps_freeze_bit_after_drop` and `dropped_freeze_guard_clears_bit` in `migration_freeze_tests.rs`. Not run.
4. **PARTIAL — S2/S4/S5 implementation written, acceptance-4 test written.** Detach lifecycle/check inventory in table manager files as described in Design above; `RenameFreeze` in `crates/shamir-engine/src/repo/instance/repo_instance/catalog/rename_freeze.rs`; transitions and counter flush in `catalog.rs`; stale clone test `successful_rename_rejects_stale_clone_crud_and_ddl` in `crates/shamir-engine/src/repo/tests/rename/freeze_tests.rs`. No tests or compilation run; the unauthorized unmounted `write_domain_core.rs` remains present and may trigger source-integrity checks if staged.
5. **PARTIAL — acceptance-5 abort test written.** `failed_rename_reattaches_and_unfreezes_source` in `freeze_tests.rs` asserts the injected `DbError::Storage` variant/message and source registration/flag/writes. Not run.
6. **PARTIAL — acceptance-6 concurrent writer test written.** `writer_is_drained_before_successful_rename` in `freeze_tests.rs` uses bounded writer/rename waits, oneshot synchronization, abort/reap paths, destination rows/count and exact migration error assertions. Not run.
7. **PARTIAL — acceptance-7 visibility test written.** `visibility_wait_times_out_before_persist_and_restores_source` in `crates/shamir-engine/src/repo/tests/rename/visibility_wait_tests.rs` holds an allocated guard, checks timeout and uncalled persist, uses bounded receipt because Strict acknowledgement waits for settlement, then checks writable CRUD/DDL. This substitutes a bounded receipt for the brief's strict insert while preserving the unsettled guard; no runtime result is claimed.
8. **PARTIAL — acceptance-8 cancellation tests written.** `cancelled_persist_reattaches_and_unfreezes_source` and `cancelled_visibility_wait_reattaches_and_unfreezes_source` in `crates/shamir-engine/src/repo/tests/rename/cancel_tests.rs`. Not run.
9. **PARTIAL — acceptance-9 counter test written.** `rename_flushes_live_record_counter` in `freeze_tests.rs`; counter increments are cache-only by source (`record_counter.rs:149–185`), so this is expected to distinguish lazy persistence on master, not a runtime-proven result.
10. **PARTIAL — acceptance-10 contracts preserved, not executed.** Existing rename, migration-freeze and hand-staged index tests were not weakened by these edits; test suite/CI has not run.
11. **PARTIAL — acceptance-11 gates.** Parent observed `cargo fmt -p shamir-engine`, `cargo fmt -p shamir-engine -- --check`, and `python -I scripts/ci/check_layout.py` pass at 00:33–00:34 CET; this child did not run them. The checker does not scan the unauthorized untracked file. Parent reports file/folder counts within limits (including `index_admission/` at four entries). Compilation, clippy, tests, and CI were not run. The audit contains duplicated revision-2 progress/design sections from a prior append-edit error; they remain under the no-delete instruction. Full import-target/source-integrity review and CI remain outstanding.

PARTIAL: S1–S5 and regression tests are written in shamir-engine. Formatting and layout checks passed; compilation, clippy, tests and CI were not run. The accidental unmounted write_domain_core.rs and duplicated audit sections remain under the no-delete rule. In-doubt reconciliation and M1–M7 remain deferred to 03d. All child work has completed; nothing was committed or pushed.
