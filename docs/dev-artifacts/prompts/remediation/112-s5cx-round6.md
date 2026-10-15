# Brief — S5c-X PR6b-2a Round 6: close the reviewed boundary escapes

Fix every P1 of the Round 5 review (verdict FAIL) on branch `feat/s5cx-repo-write-domain`. Read `CLAUDE.md` first and `docs/dev-artifacts/prompts/remediation/111-s5cx-round5.md` (Round 5 ledger).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO builds/tests/clippy/rustc of any kind (not even `cargo check`). The ONLY cargo command allowed is `cargo fmt -p <crate>`, run by the orchestrator as the very last step — do NOT run it.
- Read the REAL definition (View/Grep) of every type, function and trait you call or change; never recall signatures.
- Existing tests keep their meaning: read every existing test touching your area BEFORE finishing. Adapt fixture accessors; never delete or weaken assertions.
- Imports at the top; one primary export per file; tests in `tests/` dirs wired through manifests; `mod.rs` = re-exports only; engine module wiring uses `#[path]` (check neighbours).
- Layout ratchet: at most 7 direct entries per folder, at most 1000 lines per source file. `crates/shamir-engine/src/table/write_receipt/admission/` is AT the 7-entry cap — NO new files there; put new view types inside the existing `read_only_views.rs` (169 lines, room). `crates/shamir-server/src/db_handler/cursor_handlers.rs` is 2158 vs origin/master 2153 — your changes there must end NET ZERO vs 2153 (count lines; if a change grows it, compensate by tightening comments in the same file).
- No new dependencies, no version bumps, no unrelated refactors or comment churn.

## F1 — Interner: genuinely read-only public surface (P1)

Today `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs` publicly exposes `interner_get() -> DbResult<&Interner>`, `interner_persist()`, `interner_cell() -> Arc<OnceCell<Interner>>`. `Interner` is interior-mutable (`touch_ind(&self)` allocates+inserts, shamir-types/src/core/interner/interner.rs:138) and `InternerManager::persist` writes+flushes chunks (table/interner_manager.rs:306-371). This is a writable escape.

REQUIRED:
1. STEP ZERO: grep every NON-test cross-crate caller of `interner_get` / `interner_persist` / `interner_cell` under `crates/shamir-db/src`, `crates/shamir-server/src` (production code only; `*_for_test` fixture callers are out of scope). List them first; every one must be rewired.
2. Remove `interner_get`, `interner_persist`, `interner_cell` from the public read view. NO public method may return `&Interner`, `&InternerManager`, `Arc<OnceCell<Interner>>`, or any type from which mutation is reachable.
3. Add engine-side read helpers on `TableInfoStoreReadView` covering exactly the real production uses found in step 1. Expected shapes (verify real signatures first):
   - `filter_context(&self, refs) -> DbResult<FilterContext>` — build `shamir-engine`'s own `query::filter::FilterContext` engine-side so the caller never holds `&Interner`. Its returned borrow may be tied to the manager's lifetime (`impl<'a> TableInfoStoreReadView<'a>` returning `FilterContext<'a>`); verify this composes with every caller.
   - `to_inner(&self, &QueryValue) -> DbResult<InnerValue>` wrapping `query_value_to_inner` (check its real return/error type and wrap faithfully).
   - narrow lookups actually used (`get_ind`-style id lookups, `get_str`), named for their use.
   - `changelog.rs` (`crates/shamir-db/src/shamir_db/shamir_db/changelog.rs:72,97`) uses `interner_cell` — inspect what it really does with the cell; if it only decodes (get_str-style lookups), give it a read helper; if it genuinely needs shared loader state, move that logic into an engine-owned `TableManager`/`InternerManager` helper so no cell crosses the boundary.
4. `interner_persist` is a WRITE: replace with `TableManager::persist_interner_admitted(&self, permit: &RepoWritePermit)` in `admitted_ops.rs` (debug_check_permit + `self.interner.persist()`), and rewire its production callers (`system_store.rs`, `admin_replication.rs`, `replication_seed/catalogue.rs`) to acquire repo admission via the repo's `write_domain().admit()` (these are rare DDL/catalogue paths; follow the admit pattern already used in `admin_table_index.rs`).
5. Update the doc comments in `read_only_views.rs` so the "nothing here can mutate" claim is TRUE.

## F2 — Validator registry: read-only view (P1)

`read_only_views.rs:115` `validator_registry()` returns `Option<&Arc<ValidatorRegistry>>`; the registry has public `&self` mutators (`register`/`replace`/`rename`/`remove`, validator/registry.rs:99-225).

REQUIRED: replace it with `validator_registry_read_view()` returning `Option<ValidatorRegistryReadView<'_>>`, a struct in `read_only_views.rs` that wraps the `Arc` PRIVATELY and exposes only the read methods the real cross-crate callers use (FIRST grep production callers: `crates/shamir-server/src/db_handler/cursor_handlers.rs` and any others). No `&ValidatorRegistry` / `Arc<ValidatorRegistry>` may be reachable from it. Update the compile-fail doc example (see P2) to also demonstrate `.validator_registry()` (the old name) is unavailable downstream.

## F3 — MigrationCoordinator: capability requires the permit (P1)

`admitted_ops.rs:180-239` factories return an owned `Arc<MigrationCoordinator>` whose public methods (`run_snapshot`, `drain_until_caught_up`, `mark_cutover_ready`, `persist_state`, `rollback`, `final_drain_and_commit`, `complete_finalisation_step`, `take_migration_freeze`) mutate raw src/dst stores with no admission after return. Also `shadow_log()` exposes a log whose public `append` writes metadata.

REQUIRED:
1. Add `src_domain`/`dst_domain: Option<Arc<RepoWriteDomain>>` to `MigrationCoordinator` (crates/shamir-engine/src/migration/coordinator.rs), set via a builder (`with_domains(...)`) from BOTH admitted factories in `admitted_ops.rs` (they have `self.write_domain` and `dst.write_domain`).
2. For EVERY state-changing coordinator method the shamir-db facade calls (grep ALL call sites in `crates/shamir-db/src` first): add an admitted twin `*_admitted(&self, src_permit: &RepoWritePermit, dst_permit: &RepoWritePermit)` that debug-checks BOTH permits against the stored domains, then demote the plain mutating method to `pub(crate)`. Pure reads (`phase`, `state`, `migration_id`, `targets_table`, `finalisation_progress`, read-only verify counts, `lock_finalisation`) stay `pub`.
3. `MigrationCoordinator::shadow_log()` → `pub(crate)`; `MigrationShadowLog::append` / `append_batch` / `purge` → `pub(crate)` IF no non-engine callers exist (grep first).
4. Keep `MigrationCoordinator::new` usable for the cross-crate tests that construct it directly with raw stores (e.g. `crates/shamir-db/tests/migration_index2.rs`) — but their state-changing calls must go through the `*_admitted` twins; adapt the tests to acquire permits via `repo.write_domain().admit()` (they have repo handles). Assertions unchanged.
5. Update the facade (`crates/shamir-db/src/shamir_db/execute/admin_migration.rs`: Start/Commit/Rollback/Status handlers) to call the admitted twins, holding the permits for the duration of each mutation as it does today.

## F4 — REGRESSION: restore master freeze ordering (P1)

Round 5 moved the source migration freeze INTO `migration_coordinator_admitted` (admitted_ops.rs:192-195), so the interner dictionary copy now happens BEFORE the freeze. Master froze the source right after table resolution, BEFORE `replicate_interner_from`. Compare `git show origin/master:crates/shamir-db/src/shamir_db/execute/admin_migration.rs` and restore that ordering: in `handle_start_migration`, immediately after `get_table_admitted(src...)`, call `src_table.begin_migration_freeze_admitted(&src_permit)`; then replicate interner + descriptors; then pass the already-held `MigrationFreezeGuard` INTO `migration_coordinator_admitted` (change the factory to accept the guard as a parameter instead of acquiring it — no window where the guard is unowned). Recovery path stays freeze-free (unchanged semantics, see its doc comment).

## F5 — Compile-readiness defects (P1)

1. `crates/shamir-db/src/shamir_db/execute/admin_migration.rs` returns `Result<QueryResult, BatchError>`. `BatchError` (re-exported from shamir-query-types) has NO `From<DbError>`. Fix every `?` that propagates a freshly-mapped `DbError` in such fns: the `admit_pair` call in `handle_start_migration` (~:137-140) and in the recovery path (~:321-326) must use `.map_err(|e| err(e.to_string()))?` (or `err_code`). Then re-audit the WHOLE file (and `admin_table_index.rs`'s new admit sites) hunk by hunk: every `?`'s error type must convert to the fn's error type. The `run` closure typed `Ok::<_, DbError>` may keep DbError internally.
2. The `interner_get` lifetime defect (temporary view borrow escaping into `FilterContext` in `crates/shamir-server/src/db_handler/cursor_handlers.rs:374-383`) disappears with F1's `filter_context` design — verify the resulting signatures compose (manager lifetime, not view-temporary lifetime).
3. Read every new/changed file as the compiler would: `use` visibility, arity, receivers, struct-literal fields, `Send` across `.await`, clippy `-D warnings` traps (`needless_borrow`, `redundant_clone`, `unused_imports`, `absolute_paths` ≤5 segments, `type_complexity`, `too_many_arguments`).

## P2 — Enforced compile-fail gate

The `compile_fail` doctest on `pub struct TableManager` is dead weight: `doctest = false` in shamir-engine's `[lib]` (Cargo.toml:313) AND nextest never runs doctests. Fix BOTH halves:
1. Remove `doctest = false` from `crates/shamir-engine/Cargo.toml` `[lib]` (keep the explanatory comment, updated: doctests are enabled on THIS crate solely so the boundary compile-fail example is enforced; they still don't run under nextest).
2. Because nextest cannot run doctests, add an explicit step to `.github/workflows/ci.yml` FAST tier job (after the lib-test step): `cargo test --doc -p shamir-engine` (with the same toolchain/flags style as neighbouring steps). Also add it to the full-tier test job if that job runs tests per-OS (only one OS is enough; pick linux).
3. CRITICAL: with doctests enabled, EVERY rust code fence in shamir-engine doc comments will compile/run in CI. Grep the whole crate for doc fences (```` ``` ```` in `///` comments), audit each: convert illustrative ones to ` ```text ` ONLY if they genuinely cannot compile as-is; do not weaken real examples. Report every fence you touched.
4. Extend the `compile_fail` example to demonstrate the CLOSED surface: `.info_store()`, `.data_store()`, `.interner()`, `.validator_registry()` (old names) all unavailable downstream.

## Sequence

1. STEP ZERO greps + the F1/F2/F3 caller inventories (write them into your final message AND into `docs/dev-artifacts/prompts/remediation/112-s5cx-round6.md` under "## Implementation inventory").
2. F4 (small, isolated) → F5.1 → F1 → F2 → F3 → P2.
3. Re-read every changed file end-to-end (compile-readiness pass).
4. Final message: per-file change list with file:line, the caller inventories, the doc-fence audit result, and anything you could not verify. Do NOT run cargo fmt (orchestrator does it), do NOT commit.

## Implementation inventory (Round 6)

### F1 interner escape — callers (step zero)
Production (non-test) callers of `interner_get` / `interner_persist` / `interner_cell`, all rewired:
- `interner_get` (FilterContext reads): `shamir-db/src/shamir_db/system_store.rs` (14 sites),
  `shamir-db/src/shamir_db/execute/admin_replication.rs` (608, 642, 686, 767),
  `shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:24`,
  `shamir-db/src/shamir_db/shamir_db/seed/subscriptions.rs:60`,
  `shamir-db/src/shamir_db/execute/admin_replication.rs:424` (alter lookup),
  `shamir-server/src/db_handler/cursor_handlers.rs:378 (build_filter_context), :1320 (probe)`,
  `shamir-server/src/replication/seed/fence.rs:125`,
  `shamir-server/src/replication/seed/admission.rs:27`,
  `shamir-server/src/doctor.rs:441` (get_str),
  `shamir-server/src/replication/supervisor.rs:608`.
- `interner_get` (codec reads): `system_store.rs` (save_database / save_database_meta),
  `shamir-db/src/shamir_db/shamir_db/changelog.rs:33` (decode).
- `interner_cell`: `changelog.rs:72,97` (decode_record_value_inner / get_table_interner_cell
  → renamed `get_table_interner_read_view`) — consumers are the shamir-server subscriptions
  decode cache + filter eval, rewired onto `InternerReadView`.
- `interner_persist` (WRITE): `system_store.rs` (12 sites, via new
  `SystemStore::persist_table_interner` → `TableManager::persist_interner_admitted`),
  `admin_replication.rs` (5 sites), `replication_seed/catalogue.rs` (2 sites).

New read-view surface (`read_only_views.rs`): `filter_context`, `interner_read_view`,
`interned_field_id`, `intern_field_path`, `interner_str`, `to_inner`, `inner_to_query_value`,
`InternerReadView` {generation, get_str, get_ind, inner_to_query_value, compile_filter,
matches_bytes, matches_inner, record_bytes_to_query_value}, test-only
`InternerReadView::from_interner_for_test`. `persist_interner_admitted` added in
`admitted_ops.rs`.

### F2 validator registry — callers (step zero)
Only `shamir-server/src/db_handler/cursor_handlers.rs:532` (`order_by_column_is_schema_typed_scalar`,
uses `get_by_id` + `RecordValidator::as_schema_rules`). Replaced with
`TableManager::validator_registry_read_view()` → `ValidatorRegistryReadView::get_by_id`.
Comment at cursor_handlers (old :1229) updated.

### F3 coordinator — facade call sites (step zero)
`admin_migration.rs`: run_snapshot :196, drain_until_caught_up :197/:392,
mark_cutover_ready :203/:408, persist_state :204/:397, final_drain_and_commit :439,
complete_finalisation_step :495/:519, take_migration_freeze :381/:633, rollback :634 —
all now call the `*_admitted` twins. Demoted to `pub(crate)`: the eight plain mutators +
`drain_shadow_log` + `shadow_log()`; `MigrationShadowLog::append/append_batch/purge` →
`pub(crate)` (verified: no non-engine callers). `MigrationCoordinator::new` stays public;
`with_domains(...)` captured in BOTH admitted factories. Pure reads stay `pub`: phase,
state, migration_id, targets_table, finalisation_progress, shadow_lag, lock_finalisation,
verify_record_count, verify_committed_record_count, load_state.
Test adaptations: `shamir-db/tests/migration_index2.rs` (final_drain_and_commit_admitted +
complete_finalisation_step_admitted via `RepoWriteDomain::admit_pair` permits);
`migration_source_freeze_tests.rs` untouched (only read-only APIs).
Engine-internal `coordinator_tests.rs` / `shadow_log_tests.rs` / `cutover_guard_tests.rs`
compile unchanged (crate-internal visibility).

### Doc-fence audit (shamir-engine, doctests now ON)
- ```text fences (batch/mod.rs ×4, cond_gap_tests, p1096 ×6, query_value_conv_tests): not
  compiled by rustdoc — untouched.
- ```sh (tx/tests/oracle_stress_tests.rs:12) and ```ignore (op_watchdog.rs:188,
  validator/schema/rule_builder.rs:4): skipped by rustdoc — untouched.
- compile_fail kept: repo/changelog_store/write_domain.rs:57 (private-field forgery — still
  fails as required); table/table_manager.rs:51 EXTENDED with `.interner()` and
  `.validator_registry()` (old names) — now demonstrates the full closed surface.
- CHANGED → ```text: tx/tests/base_index_tx_tests.rs:661 and :726 (untagged pseudo-SQL
  protocol walkthroughs — cannot compile as Rust). They live in a #[cfg(test)] module, so
  doctests never collect them either way; conversion is hygiene.

## Exhaustive audit (Round 6)

Read-only audit of the WHOLE PR (merge-base `d615d12` → HEAD **plus** the
dirty working tree; all diffs below are `git diff origin/master`, i.e. both
combined). 226 files changed (7,853+/6,490−). No builds/tests run; no git
mutations. The only write was this section.

### Per-file classification (all 226 files)

Legend: (a) raw-handle visibility/getter change, (b) read-view/admitted-op
rewiring, (c) test fixture accessor substitution, (d) doc/comment,
(e) config/feature, (f) EOL-only, (g) other.

| File(s) | Class | Notes |
|---|---|---|
| `shamir-engine/Cargo.toml` | (e) | doctest=true + `required-features = ["test-util"]` on 14 benches; `test-util = []` feature. OK |
| `shamir-db/Cargo.toml`, `shamir-server/Cargo.toml` | (e) | `test-util` feature chain; shamir-server adds dev-dep `shamir-db { features = ["test-util"] }` (line 157) — shamir-db itself has the mirror dev-dep (Cargo.toml:99), so inline `#[cfg(test)]` tests compile. OK |
| `.github/workflows/ci.yml` | (e) | `cargo test --doc -p shamir-engine --locked` added to FAST tier (affected-gated) and FULL tier (ubuntu only). Matches P2. OK |
| `shamir-engine/src/table/table_manager.rs` | (a)+(b)+(d) | raw getters demoted `pub(crate)`, `*_for_test` gated `cfg(any(test, feature="test-util"))`; read views exported; `begin_write_barrier` now returns `DbResult` and self-admits; `begin_migration_freeze_admitted` added; `WriteBarrierGuard` carries optional permit (F4 no-unowned-window). Compile-fail fence at :58 covers `.info_store()/.data_store()/.interner()/.validator_registry()`. OK |
| `admission/admitted_ops.rs` (new) | (b) | all `*_admitted` fns debug-check permit; F4 guard-as-parameter; `persist_interner_admitted`. OK |
| `admission/read_only_views.rs` (new) | (b) | no `&Interner`/cell/registry escapes; `InternerReadView`/`ValidatorRegistryReadView` wrap handles PRIVATELY; `from_interner_for_test` gated. See P3-1 note on `to_inner`. OK |
| `admission/write_domain_barrier.rs`, `write_domain_index_admitted.rs`, `write_domain_index_core.rs`, `write_domain_online_index.rs`, `write_domain_rename_admitted.rs` (new) | (b) | admitted twins of index DDL/online-build/rename; every pub fn takes `&RepoWritePermit` + debug-check (verified signatures). Old plain `create_index`/`drop_*` remain PUB but now self-admit internally (index_mgmt.rs:34-315 verified). OK |
| `migration/coordinator.rs` | (a)+(b) | 8 mutators + `shadow_log`/`drain_shadow_log` demoted `pub(crate)`; admitted twins check BOTH domains; `with_domains` captured in both factories. `debug_check_admission` skips dst check when domain/permit `None`. OK (see P3-2 on debug-only checking) |
| `migration/shadow_log.rs` | (a) | `append`/`append_batch`/`purge` → `pub(crate)`; verified no non-engine callers (grep). OK |
| `repo/changelog_store/write_domain.rs` (new) | (b) | domain/permit/reservation/admit_pair (ascending-token order, same-instance rejected); compile-fail forgery fence at :57. OK |
| `repo/changelog_store/write_domain_membership.rs` (new), `changelog_store/repo_membership_impl.rs` (new), `repo_instance.rs`, `repo/mod.rs`, `changelog_store.rs` | (a)+(b) | `add_table`/`remove_table` → `pub(crate)` + permit; `get_table` self-admits; `get_table_admitted`; write_domain constructed before recovery; `worker_lifecycle` field removed (F-6b scaffolding retirement — PR-planned, not Round 6). `rename_table_stores`/`repo_identity`/`bump_history_epoch`/`apply_restore_operation` self-admit; `_admitted` twins `pub(crate)`. OK |
| `db_instance/db_instance.rs` | (a)+(b) | `publish_repo_admitted` (vacant-only insert), `create_table_checked(_with_config)`, `drop_table_checked` all admit. **`add_repo_with_durability` / `remove_repo` / `rename_repo` remain pub, permit-free** — see F-1 (P2) |
| `tx/recovery.rs` | (b) | `replay_v2_op`/`replay_v2_entry` pub self-admit; `recover_inflight_v2(repo, permit)` checks permit; all internal paths threaded. OK |
| `repo/durability_policy/.../repo_impl.rs` | (b) | factory-open admits once across identity+recovery. OK |
| `repo/seed_install/writer.rs` | (b) | seed install admits once; `add_table_admitted`/`get_table_admitted`/`repo_identity_admitted`. OK |
| `table/record_counter.rs` | (b) | `set`/`set_to`/`persist` self-admit; admitted twins; `Persistable::persist` short-circuits on clean (`is_dirty`, 2 atomic loads); `new` gets a private domain. `increment`/`get` stay admission-free by design (leader write path). OK |
| `table/doctor.rs` | (b) | `repair` self-admits; `repair_admitted` uses `set_to_admitted` (no re-admit deadlock). OK |
| `table/table_manager_buffer.rs` | (b) | `set_buffer_config` self-admits; `alter_buffer_config` single admission. OK |
| `table/table_manager_index_mgmt.rs` | (b)+(d) | body moved to `write_domain_index_core.rs`; plain pub wrappers self-admit. OK |
| `table/table_manager_replication.rs` | (b) | `replicate_interner_from`/`replicate_index2_descriptors_from`/`seed_log_from_data_store`/`bulk_populate_index2` self-admit + admitted twins. Minor doc/comment tightening + `src_interner` hoisted out of loop (behaviour-identical). OK |
| `table/table_manager_sorted_index.rs` | (b) | create/drop sorted self-admit + admitted twins. OK |
| `table/table_manager_validators.rs` | (a)+(b) | `validator_registry_ref` → `pub(crate)`; add/remove binding self-admit + twins. **`set_validator_registry(Arc<ValidatorRegistry>)` stays pub** — see F-2 (P2) |
| `validator/persistence.rs` | (a) | `save_validators_metadata` → `pub(crate)`. OK |
| `table/mod.rs`, `repo/mod.rs` | (a) | re-exports (read views; `RepoWrite*` types). OK |
| `shamir-db/src/shamir_db/execute/admin_migration.rs` | (b) | Start: private dst `RepoInstance::from_factory` → `admit_pair` → `get_table_admitted` → **freeze before interner copy (F4, master ordering)** → `replicate_*_admitted` → `migration_coordinator_admitted(…, freeze, …)` → admitted mutators → `publish_repo_admitted`; failure drops unpublished dst (replaces master's remove-repo rollback — equivalent-or-better). Commit: state snapshot → admit_pair → finalisation lock → **state-unchanged re-check** (new TOCTOU guard) → admitted twins. Rollback: same order, dst permit optional. Recovery: `admit_pair` + `migration_coordinator_recovered_admitted`, freeze-free (unchanged semantics). **F5.1 verified: every bare `?` in the file resolves to `BatchError`** (`ok_or_else(err/err_code)`); both Start's run closure (`Ok::<_, DbError>` + `DbError::Internal` mappings) and the recovery-path `admit_pair` use `.map_err(\|e\| err(e.to_string()))?`. OK |
| `admin_table_index.rs` | (b) | DROP-TABLE sweep → `drop_all_indexes_for_table_removal_admitted` (permit dropped before the non-reentrant re-admitting DDL); InProgress status writes admitted; op-status reads via read view. OK |
| `admin_schema.rs` + `repo_admin/admin_schema_admission.rs` (new) | (b) | `schema_repo_permit` acquired once per DDL op; barrier/validate/stamp/compile all permit-threaded; helpers moved to the new module verbatim. OK |
| `admin_describe.rs`, `admin_list.rs`, `helpers.rs`, `core.rs`, `changelog.rs`, `seed/subscriptions.rs`, `seed_catalogue.rs`, `table_management.rs`, `system_store.rs`, `replication_seed/catalogue.rs`, `admin_replication.rs` | (b) | read-view rewiring (`filter_context`, `interner_read_view`, `interned_field_id`, `to_inner`, `mvcc_snapshot_stream`, `read_op_status`, `data_store_read_view().flush()`); interner persist → `SystemStore::persist_table_interner` (admits SYSTEM repo — all system tables live there, identity check valid); `create_table`/`drop_table` → `_checked`; `compile_table_schema` self-admits + `pub(crate)` admitted twin; `get_table_admitted` facade twin. OK |
| `shamir-engine/src/repo/changelog_store/tests/*`, `db_instance/tests/*`, `meta/tests/*`, `repo/tests/*`, `table/tests/**` (49 files), `tx/orphan_repair/tests/*`, `tx/tests/**` (47 files), `benches/*` (14) | (c) (+ new tests) | all are fixture accessor substitutions (`*_for_test`, `add_table_for_test`, permit threading into `phase_b_a_backfill`/`phase_c_d_catchup_and_publish`, `begin_write_barrier(...).await.unwrap()`). Line-level dedup scan of every removed line confirms NO assertion deletions outside the two explicitly rewritten tests below. New tests: `write_domain_membership_tests.rs` (190), `write_domain_admission_matrix_tests.rs` (443), `write_domain_barrier_tests.rs` (41), `write_domain_counter_tests.rs` (123), `write_domain_shared.rs` (47), `schema_rollback_tests.rs` admission test (+53), `retention_ddl.rs` (+58) — all additive, assertions preserved/strengthened (e.g. recovery_tests `len()==1` → `== inflight_before`) |
| `table/tests/f95_ddl_admission_tests.rs` | (c)+(g) | **REWRITTEN test — see F-3 (P2)** |
| `repo/tests/table_init_retry_tests.rs` | (c)+(g) | retired-initializer test adapted to contended-admission Notify; assertions preserved and strengthened (poll-pending check). OK |
| `shamir-server/src/db_handler/cursor_handlers.rs` | (b) | `build_filter_context` engine-side; `probe_index_seek_eligible` async via read view; `validator_registry_read_view`. **2154 lines vs required net-zero 2153 — see F-5 (P3)** |
| `shamir-server/src/doctor.rs`, `server_launcher.rs` | (b) | `create_table_checked`. OK |
| `shamir-server/src/replication/seed/admission.rs`, `seed/fence.rs`, `supervisor.rs` | (b) | `filter_context` engine-side. OK |
| `shamir-server/src/subscriptions/{bridge,decode_cache,filter_eval,target_match}.rs` | (b) | `Arc<OnceCell<Interner>>` → `Arc<InternerReadView>` end-to-end (cache type, eval, delivery). Behaviour-identical delegation. OK |
| `shamir-server/src/replication_seed/export/exporter.rs` | (b) | read-view introspection + `mvcc_snapshot_stream`. OK |
| `shamir-server` tests (17 files incl. `seed_recovery_e2e/scenarios/*`) | (c) | accessor substitution / `*_for_test`; seed e2e scenarios only un-inline `Box::pin(…_body())` wrappers — zero assertion changes. OK |
| `docs/dev-artifacts/prompts/remediation/{108,109,110,111}-*.md`, `112-s5cx-round6.md` (untracked) | (d) | ledgers. OK |
| `docs/guide-docs/follower-journal-durability.md` | (d) | PR6b-2b lifecycle section replaced by the PR6b-2a write-domain section (matches implementation). OK |
| **EOL-only / whitespace-only** | (f) | none observed — every changed file carries substantive hunks (several files show CRLF→LF normalization warnings on rewrite, but each also has content changes; no file is EOL-only) |

### Admit-site transitive closure (production, outside shamir-engine → chain up)

| Entry point (caller crate) | Chain | Permit held for whole mutation? |
|---|---|---|
| `ShamirDb::get_table_admitted` (admin_schema, admin_migration, doctor via facade) | handler → `table_management.rs:379` → `RepoInstance::get_table_admitted` → table construction | ✓ debug-checked at every layer |
| `ShamirDb::compile_table_schema_admitted` (admin_schema) | handler holds `schema_repo_permit` → binding add `add_validator_binding_admitted` → `bindings_lock` | ✓ (permit → bindings_lock order) |
| `SystemStore::persist_table_interner` (admin_replication ×4, system_store saves, catalogue.rs persist) | handler → admits SYSTEM repo domain → `persist_interner_admitted` → `interner.persist()` | ✓ |
| `admin_table_index` InProgress/DROP sweep | handler admits repo → `write_ddl_op_status_admitted` / `drop_all_indexes_for_table_removal_admitted` | ✓ (permit dropped before re-admitting DDL — correct, non-reentrant) |
| `ShamirAdminExecutor::handle_start_migration` | private `RepoInstance::from_factory` (recovery inside factory admits under `from_factory_with_durability`) → `admit_pair` → freeze → interner copy → descriptor copy → coordinator factory → run_snapshot/drain/cutover/persist → `publish_repo_admitted(dst, dst_permit)` → drop permits | ✓ both permits span the entire mutation; freeze owned continuously |
| `handle_commit_migration` / `handle_rollback_migration` | state read → `admit_pair` (dst optional in rollback) → finalisation lock → state-unchanged re-check → admitted coordinator mutators + admitted barriers + `seed_current_versions_admitted`/`reset_and_populate_index2_admitted` | ✓ |
| `handle_drop_repo` / `handle_drop_db` cascade / `db_management.rs` remove paths | handler → `ShamirDb::remove_repo` → `DbInstance::remove_repo` → `repos.remove` | ✗ NO PERMIT — F-1 below |
| `db_management.rs:616 rename_repo` | handler → `DbInstance::rename_repo` (remove+reinsert, re-keys name) | ✗ NO PERMIT — F-1 |
| `core.rs:302` / `db_management.rs:419 add_repo_with_durability` | handler → `DbInstance::add_repo_with_durability` → unconditional `repos.insert` (clobbers an existing name) | ✗ NO PERMIT — F-1 |
| `server_launcher.rs:413` / `doctor.rs:255 create_table_checked` | handler → admits repo → `add_table_admitted` | ✓ |
| `shamir-server` subscriptions/changefeed decode (`get_table_interner_read_view`) | read-only (`get_table` read init path self-admits when construction needed) | n/a (no mutation) |
| Engine-internal leaders: `RecordCounter::increment`, leader commit writes, drainer apply | deliberately permit-free (design: leader write path gains nothing) | by design (documented) |

Counter persist: `set`/`set_to`/`persist`/`Persistable::flush_metadata` all admit (clean counters short-circuit). Schema/retention (`apply_table_retention` admits, new test enforces pending-until-admit), validator persistence (`save_validators_metadata` pub(crate); bindings admitted), migration (F3/F4 above), recovery/replay (recovery admits; replay threaded), seed install (writer.rs admits once), doctor (`repair` self-admits; server doctor `create_table_checked`) — all closed.

### Findings

**F-1 (P2) — registry add/remove/rename mutations remain permit-free and pub.**
`crates/shamir-engine/src/db_instance/db_instance.rs:58` (`add_repo_with_durability` — unconditional `repos.insert`, clobbers an existing repo name, no admission), `:161` (`remove_repo`), `:176` (`rename_repo`). Production cross-crate callers mutate the registry with no permit: `crates/shamir-db/src/shamir_db/shamir_db/core.rs:302`, `shamir_db/db_management.rs:419,440,476,521,616`, `shamir_db/execute/admin_db_repo.rs:142,325,411`. The PR added the admitted twin (`publish_repo_admitted`, used only by the migration path) without demoting or gating the plain paths. Pre-existing on master, but exactly the class scope item 3 asks to report. Suggest: admit (or at least `admit_for_close`-style) the target repo in remove/rename, and make `add_repo_with_durability` reject occupied names like `publish_repo_admitted` does.

**F-2 (P2) — `TableManager::set_validator_registry(&mut self, Arc<ValidatorRegistry>)` is pub** (`table_manager_validators.rs:107`). `get_table` returns an owned `TableManager`, so any cross-crate caller can swap the registry (mutation) without a permit. Used by the facade (`table_management.rs:378`) as the wiring mechanism, so it cannot simply be demoted — needs an admitted twin + `pub(crate)` demotion or a facade-only constructor seam. Pre-existing mechanism; report per scope item 4.

**F-3 (P2) — test rewritten to weaker form:** `crates/shamir-engine/src/table/tests/f95_ddl_admission_tests.rs:469` `f95_admission_mutex_is_per_table_not_global`. Master asserted a live concurrency property: while X's barrier critical section is parked mid-CS, Y's DDL completes within 200 ms (per-table `ddl_admission` independence). The rewrite asserts only that both barriers can be held simultaneously under one repo permit — no concurrent non-blocking claim remains. This mirrors a real semantic change (barriers now hold the REPO domain, so cross-table DDL on one repo does serialize), but the old regression is deleted, not adapted. If the repo-wide serialization is intended, the replacement should say so in its doc comment; consider keeping a cross-REPO independence test.

**F-4 (P2) — `RepoInstance::repo_interner(&self) -> DbResult<InternerManager>` is pub** (`repo_instance.rs:799`), and `InternerManager::interner_cell() -> Arc<OnceCell<Interner>>` is pub (`interner_manager.rs:137`). Cross-crate: `db.get_repo(r)?.repo_interner().await?` yields the manager clone; `.get().await?` hands out `&Interner` (interior-mutable via `touch_ind`) and `.interner_cell()` the raw cell — the exact escape F1 was meant to close, one hop off the audited path. Engine-internal callers only today (grep: `create_table_context`, `flush_buffers`, tests), but the surface is open. Suggest `pub(crate)` (or a read view) for both.

**F-5 (P3) — cursor_handlers.rs line ratchet violated:** 2154 lines vs the brief's mandated net-zero vs 2153 (one line over). Brief-compliance, not correctness.

**F-6 (P3) — identity checks are `debug_assert` only.** Every `debug_check_permit` / `debug_check_admission` compiles to a no-op in release (`RepoWriteDomain::debug_check_permit`, `MigrationCoordinator::debug_check_admission`). The docs say "identity-checked"; in `--release` a wrong-domain permit is silently accepted. Consistent with the established engine pattern, but the audit's "identity-checked" bar is met only in debug builds — worth an explicit note in the write-domain docs.

**F-7 (P3) — `InternerReadView::to_inner`/`intern_field_path` semantics:** `to_inner` may mint new interner ids (documented on the method, correct — it is the write-prep step); `intern_field_path`/`filter_context`/`compile_filter` are pure `get_ind` reads (verified against `query::filter` usage). No action; recorded as reviewed-and-correct.

**F-8 (P3, observation) — `handle_start_migration` failure semantics changed:** master rolled back by `remove_repo(dst)`; now the dst is never published (private instance dropped). Strictly better (no remove-without-permit), but the state-unchanged/dst-collision behaviour differs: an existing dst name is now rejected up-front (`has_repo` check) instead of replaced. Intentional per the diff comments; noted for the record.

**Round-6 defect checks — all PASS:**
- F4 freeze ordering: freeze acquired immediately after `get_table_admitted(src)` and BEFORE `replicate_interner_from_admitted`; guard passed into `migration_coordinator_admitted` as a parameter; no unowned window; recovery path freeze-free with domains captured (admitted_ops.rs:186-247).
- F5.1 BatchError `?` conversions: whole-file audit clean (both Start closure and recovery `admit_pair`).
- Doc fences: full crate scan — only `text`/`sh`/`ignore`/`compile_fail` fences exist (list verified at read_only_views audit); both compile-fail fences reference names that genuinely do not resolve downstream; the two `base_index_tx_tests.rs` conversions match the ledger.
- P2 wiring: `doctest` no longer false in engine `[lib]`; CI runs `cargo test --doc -p shamir-engine --locked` in FAST (affected-gated) and FULL (linux) tiers.

### What was and was NOT inspected

Inspected: full `git diff origin/master` hunks for ALL 226 changed files — every production file line-by-line (shamir-db execute/shamir_db, engine repo/table/migration/tx-recovery/validator/db_instance, server handlers/subscriptions/replication/launcher/doctor, CI, manifests, docs); every asymmetric (insert≠delete) test diff in full; a line-level deduplicated scan of every REMOVED line across all symmetric test/bench diffs (only accessor substitutions and the two documented rewrites appeared); the full text of all six new production source files and spot-reads of the five new admission modules' public surfaces; greps proving no residual raw-getter use in shamir-db/shamir-server production code (`interner()`, `info_store()`, `data_store()`, `index2_registry()`, `index_manager_ref()`, `sorted_indexes()`, `mvcc_store()`, `unique_write_lock()`, `validator_registry_ref`, `interner_get/persist/cell` — only comments and test-gated `*_for_test` calls remain).

NOT inspected (and why it does not change the verdict): the ~40 purely symmetric (n/n ≤ 3) bench/test diffs were verified by removed-line dedup + representative full reads rather than hunk-by-hunk for each; bodies of moved code inside `write_domain_index_core.rs`/`write_domain_online_index.rs`/`write_domain_rename_admitted.rs` were compared at the signature/admission level, not line-diffed against their pre-move text in `table_manager_index_mgmt.rs` (a mechanical move; fmt/clippy gate covers drift). No builds, tests, or doctests were executed (per rules) — compile-fail fences and `?` conversions were verified by reading, not by rustc.
