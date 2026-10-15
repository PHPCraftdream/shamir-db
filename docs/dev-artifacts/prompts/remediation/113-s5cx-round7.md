# Brief — S5c-X PR6b-2a Round 7: finish the boundary, fix CI lints

Read `CLAUDE.md`, `docs/dev-artifacts/prompts/remediation/112-s5cx-round6.md` (implementation inventory + audit), and the Round 6 reviewer verdict. Fix every listed defect. NO builds/tests/clippy/rustc; NO git-mutating commands; `cargo fmt` is run by the orchestrator afterwards — do not run it. Read the REAL definition of everything before editing. Existing tests keep their meaning.

## L1 — CI lint fixes (from the round-5 push, all in shamir-engine)
1. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_online_index.rs:39` and `:246`: `pub async fn phase_b_a_backfill` / `phase_c_d_catchup_and_publish` expose the `pub(crate)` `PhaseBAResult` (private_interfaces). These are engine-internal admitted twins: GREP their callers first, then demote BOTH methods (and any sibling that returns/takes `PhaseBAResult`) to `pub(crate)`. Do NOT widen `PhaseBAResult` to `pub`.
2. `crates/shamir-engine/src/table/table_manager.rs:754` `pub(crate) fn table(&self)` is dead (dead-code). Confirm zero remaining callers (the `table_for_test` twin stays), then DELETE the method and its doc comment.
3. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_barrier.rs:436` and `:512`: `&open_permit` needless borrows — drop the `&`.

## L2 — `filter_context` must not expose the mutable interner (P1)
`FilterContext.interner` is a `pub` field (`crates/shamir-engine/src/query/filter/eval_context.rs:47`), so the view's `filter_context` leaks `&Interner` (interior-mutable via `touch_ind`). FIX: change the field to `pub(crate)` and add a read-only accessor `pub fn interner(&self) -> &Interner` ONLY IF a caller needs it. GREP every reader of `.interner` on a `FilterContext` value across the workspace: engine-internal readers keep working under `pub(crate)`; any cross-crate reader (shamir-db/shamir-server) must be rewired to the accessor or to an engine-side helper — enumerate them in your report. Keep `FilterContext::new` working for engine callers.

## L3 — `to_inner` must not mutate through the read view (P1)
Remove `TableInfoStoreReadView::to_inner` (read_only_views.rs:152). Its only real effect at the call sites is interning the record's map keys as the WRITE-prep step. Move that behind admission:
- add `TableManager::intern_query_value_admitted(&self, permit: &RepoWritePermit, value: &QueryValue) -> DbResult<InnerValue>` in `admitted_ops.rs` (debug_check_permit + `query_value_to_inner`, discarding or returning the inner per caller need).
- GREP all `to_inner` callers (expected: `system_store.rs` save_database ~:237, save_database_meta ~:916, and any others). Each is immediately followed by `persist_table_interner` — refactor `SystemStore::persist_table_interner` into a single-admission `persist_table_interner_interning(&self, table, record: &QueryValue)` that admits ONCE, interns the record (mutating, admitted), then persists the dictionary delta; rewire every caller (no second admit — the tokio mutex is non-reentrant). Keep the plain `persist_table_interner` only if non-interning callers remain.
- `inner_to_query_value` stays (pure read). Update `read_only_views.rs` doc claims so they are TRUE.

## L4 — Missing re-export (P1)
`crates/shamir-engine/src/table/mod.rs:53-56`: add `InternerReadView` and `ValidatorRegistryReadView` to the `pub use table_manager::{...}` list so `shamir_engine::table::InternerReadView` resolves (changelog.rs, decode_cache.rs, filter_eval.rs and 3 server test files import that path).

## L5 — Undefined `home` (P1)
`crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:129` inside `install_seed_profile(&self, ...)`: `home.system_store()` → `self.system_store()`.

## L6 — Migration retry test self-deadlock (P1)
`crates/shamir-db/tests/migration_index2.rs:385-443` (`assert_committed_crash_retry`): the test holds both `admit_pair` permits while calling `dst.seed_log_from_data_store()` (:405), `dst.bulk_populate_index2()` (:419) and `exec_built(commit)` (:443) — all three re-admit internally → non-reentrant deadlock. FIX: keep the permits ONLY for the admitted coordinator twins (`final_drain_and_commit_admitted`, `complete_finalisation_step_admitted`), then `drop(src_permit); drop(dst_permit);` BEFORE the seed/populate/commit calls so those self-admit. Every assertion stays. Check the second test in the file and `migration_source_freeze_tests.rs` for the same pattern.

## L7 — P2: fast-tier doctest gate must actually run
`.github/workflows/ci.yml:111-122`: the step branches on `$AFFECTED` but never receives it. Add the step-local mapping used by the sibling steps (`env: AFFECTED: ${{ steps.affected.outputs.args }}`). ALSO split the compile-fail example on `TableManager` (table_manager.rs:50-64) into FOUR separate ` ```compile_fail ` blocks — one per accessor (`info_store`, `data_store`, `interner`, `validator_registry`) — so reopening any single accessor independently fails the gate.

## Sequence
L1 → L4 → L5 → L6 → L2 → L3 → L7. Then re-read every changed file end-to-end (compiler-readiness: use resolution incl. the mod.rs re-export, arities, lifetimes — `filter_context`'s returned `FilterContext<'a>` must still unify with the `'a TableManager` borrow after L2). Write your caller inventories into `## Round 7 inventory` in THIS brief file. Final message: per-file changes with file:line + inventories + anything unverified. Do NOT run cargo fmt; do NOT commit.

## Round 7 inventory

### L1.1 — phase fns demoted (write_domain_online_index.rs)
`pub async fn phase_b_a_backfill` (:39) and `pub async fn phase_c_d_catchup_and_publish` (:246) → `pub(crate)`. No sibling takes/returns `PhaseBAResult` besides these two (`grep PhaseBAResult` across the workspace: definition at table_manager_index_mgmt.rs:19, `pub(crate)`, plus these two fns). Callers (ALL engine-internal, `pub(crate)`-visible):
- `crates/shamir-engine/src/table/write_receipt/admission/write_domain_index_core.rs:51` (`phase_b_a_backfill`), `:56` (`phase_c_d_catchup_and_publish`)
- engine tests: `src/table/tests/index_build_explicit_pin_tests.rs:100,107,142`; `p1060_online_index_crash_recovery_tests.rs:135,224,301,313,394,406,496,502`; `p1061_pipeline_property_tests.rs:179,222,360,382,476,484`; `p1087_phase_b_a_tests.rs:104,149,209`; `p1088_phase_c_d_tests.rs:98,104,151,170,223,240,310,321,369,389,444,450`
- doc mentions only: `benches/f78_writer_latency.rs:25-26`, `table_manager.rs:335` (doc), `shamir-index/src/base_index/index_manager.rs:2022` (doc)

### L1.2 — dead `TableManager::table` deleted
`table_manager.rs` (was :754/:763 incl. doc comment) deleted. Caller census: exactly ONE caller existed — `src/table/tests/table_manager_tests.rs:51` (`ctx1.table()`/`ctx2.table()` inside `test_table_manager_clone`). Rewired to the `table_for_test` twin (same assertion, same meaning; test file is cfg(test), twin available). `table_for_test` (:767 area) untouched.

### L1.3 — needless borrows dropped (write_domain_barrier.rs)
:436 `recover_hash_renames_admitted(&open_permit)` → `(open_permit)`; :512 `repair_admitted(&open_permit)` → `(open_permit)`. `open_permit` is `let open_permit = permit;` (already `&RepoWritePermit`); targets take `&RepoWritePermit` (write_domain_rename_admitted.rs:14, doctor.rs:563).

### L2 — FilterContext.interner → pub(crate) (eval_context.rs:47, now :47-51)
Field made `pub(crate)` with an explanatory doc comment. NO read-only accessor added (brief: only if a caller needs it — none does). Caller inventory of `.interner` reads on a `FilterContext` value (ALL engine-internal, compiled with crate visibility):
- `src/query/filter/cond_cache.rs:4` (doc), `field_path_cache.rs:4` (doc), `filter_node.rs:599,840,860,886,906,932,1068` (`ctx.interner`), `resolve.rs:306,318,415,701` (`ctx.interner`), `tests/field_path_cache_tests.rs:5` (doc)
- `src/query/read/aggregate.rs:829,1110,1131` (`ctx.interner` — engine, fine under pub(crate))
- doc mention only: `benches/cond_expr_eval.rs:54`
Cross-crate readers (shamir-db / shamir-server / sdk / benches / integration tests): ZERO — every `.interner` there is on `TableManager`/`InternerManager` (`tbl.interner()`); no `FilterContext { .. }` struct literal exists anywhere in the workspace (all construction via `FilterContext::new`, ~25 bench/test sites — unaffected). `FilterContext::new` unchanged.

### L3 — to_inner removed, interning moved behind admission
- `admitted_ops.rs`: added `pub async fn intern_query_value_admitted(&self, permit: &RepoWritePermit, value: &QueryValue) -> DbResult<InnerValue>` (after `persist_interner_admitted`, ~:185) — debug_check_permit + `interner.get()` + `query_value_to_inner`, returns the encoded inner (caller discards). Imports added: `query_value_to_inner`, `{InnerValue, QueryValue}`.
- `read_only_views.rs`: DELETED `TableInfoStoreReadView::to_inner` (was :152-156) and its `query_value_to_inner` import. Doc claims updated: struct doc bullet list now documents the removal and points at `intern_query_value_admitted` / `persist_table_interner_interning`; `inner_to_query_value` kept (pure read). Module header "Nothing here can mutate" claim now TRUE.
- `system_store.rs`: added `pub(crate) async fn persist_table_interner_interning(&self, table: &TableManager, record: &QueryValue)` (after `persist_table_interner`, ~:172) — admits ONCE, `intern_query_value_admitted`, then `persist_interner_admitted` under the same permit. `to_inner` callers grepped workspace-wide: exactly two, both rewired:
  - `save_database` (was :236): `to_inner` line removed; `persist_table_interner` → `persist_table_interner_interning(&table, &rec)` (op now stores `rec.clone()` so `rec` stays borrowable — meaning unchanged).
  - `save_database_meta` (was :915): same rewiring with `record`.
- Plain `persist_table_interner` KEPT — 16 remaining callers, none with a preceding `to_inner` (system_store.rs :243?→ now shifted, :308,:432,:504,:581,:687,:951,:984,:1010,:1076,:1103,:1173 + admin_replication.rs:106,:232,:351,:535,:809 + replication_seed/catalogue.rs:91,:129). None of these had an adjacent to_inner (verified by grep: `\.to_inner(` across crates → only the two removed).

### L4 — mod.rs re-export
`crates/shamir-engine/src/table/mod.rs:53-56`: `InternerReadView` and `ValidatorRegistryReadView` added to the `pub use table_manager::{...}` list (both already re-exported by table_manager.rs from `read_only_views` at :40-42).

### L5 — catalogue.rs:129
`home.system_store().persist_table_interner(&table)` → `self.system_store().persist_table_interner(&table)` inside `install_seed_profile`. (catalogue.rs:91's `home` is a fn parameter of `persist(home: &ShamirDb, ..)` — legitimate, untouched.)

### L6 — migration_index2.rs self-deadlock
`assert_committed_crash_retry` (:385-443 region): first `admit_pair` kept ONLY for `final_drain_and_commit_admitted`, then `drop(src_permit); drop(dst_permit);` BEFORE `dst.seed_log_from_data_store()`. `complete_finalisation_step_admitted` re-admits a fresh pair inside the `if seed_marker` block, drops both before `dst.bulk_populate_index2()` / `exec_built(commit)`. All assertions unchanged. Second test in the file: no admit_pair/permit usage. `migration_source_freeze_tests.rs`: grepped — zero `admit_pair`/permit usage.

### L7 — CI
- `.github/workflows/ci.yml:111-115`: doctest step now has `env: AFFECTED: ${{ steps.affected.outputs.args }}` (same mapping as the sibling "lib tests" step).
- `table_manager.rs` doc: single 4-accessor compile_fail block split into FOUR per-accessor blocks (`info_store`, `data_store`, `interner`, `validator_registry`), comment updated.

### Not verified / caveats
- NO builds/tests/clippy were run (per brief) — compiler-readiness was checked by reading + grep only. In particular the `'a` unification of `filter_context` is untouched by L2 (field visibility change only, `FilterContext::new` unchanged).
- `cargo fmt` NOT run (orchestrator formats). New code formatted to rustfmt style by hand; minor wrap diffs possible.
- Round-6-style risk noted, not acted on: `system_store.rs` still has 16 plain `persist_table_interner` callers whose `set_via_implicit_tx` writes go through `execute_set_tx` (which interns lazily engine-side); per brief only the two `to_inner` callers were rewired.

## Exhaustive audit (Round 7)

Read-only audit of the whole PR: every file in the union of
`git diff --name-only origin/master...HEAD` (210 committed) + every
`git status --porcelain` dirty/untracked file (12 dirty + this brief) —
**212 distinct real files** (my own scratch dir was created and removed;
the tree is back to exactly the implementer's state). No builds, tests,
clippy, or git-mutating commands were run. The only write is this section.

### Round 7 target verification (hunk-precise, from current source)

- **L1.1** ✓ `write_domain_online_index.rs:39` (`phase_b_a_backfill`) and
  `:246` (`phase_c_d_catchup_and_publish`) are now `pub(crate)`. Grep: no
  remaining `pub async fn phase_*` anywhere; no other fn takes/returns
  `PhaseBAResult` (still `pub(crate)` at `table_manager_index_mgmt.rs:19`).
  Engine-internal callers listed in the inventory below all compile under
  crate visibility.
- **L1.2** ✓ `TableManager::table()` deleted (table_manager.rs; only the
  `#[cfg(any(test, feature="test-util"))] pub fn table_for_test` remains).
  Workspace grep `\.table\(\)` → zero code callers (one unrelated doc string
  in shamir-query-builder). Sole caller `table_manager_tests.rs:51` rewired
  to `table_for_test()` — same `std::ptr::eq` assertion, unchanged meaning.
- **L1.3** ✓ `write_domain_barrier.rs:436`
  `recover_hash_renames_admitted(open_permit)` and `:512`
  `repair_admitted(open_permit)` — `&` dropped; `open_permit` is already
  `&RepoWritePermit` (`let open_permit = permit;`, :412), targets take
  `&RepoWritePermit`. No needless-borrow form remains anywhere (grep).
- **L2** ✓ `eval_context.rs:48-53`: `interner` field is `pub(crate)` with an
  explanatory doc comment; `FilterContext::new` (arity 2, :131) unchanged; no
  read accessor added (none needed). **Zero cross-crate readers**: grep of
  every `.interner` token under shamir-db/src and shamir-server/src returns
  only method calls on `TableManager`/`InternerManager` (`tbl.interner()`,
  removed in Round 6) — no `FilterContext` field access, and NO
  `FilterContext { .. }` struct literal exists anywhere in the workspace (all
  construction via `FilterContext::new`). Engine-internal readers
  (`filter_node.rs`, `resolve.rs`, `aggregate.rs`) compile under
  `pub(crate)`.
- **L3** ✓ `to_inner` DELETED (`read_only_views.rs`; import of
  `query_value_to_inner` removed with it). `intern_query_value_admitted`
  added (`admitted_ops.rs:197`, `pub`, debug_check_permit +
  `query_value_to_inner`, returns `InnerValue`). Single-admission chain:
  `SystemStore::persist_table_interner_interning` (`system_store.rs:179`,
  `pub(crate)`) admits ONCE → `intern_query_value_admitted` →
  `persist_interner_admitted` under the SAME permit — no second admit (the
  tokio domain mutex is non-reentrant). Both former `to_inner` callers
  rewired (`save_database` :255-265 with `rec.clone()` so `&rec` stays
  borrowable; `save_database_meta` :933-942). Workspace grep: zero
  `.to_inner(` callers remain; zero call sites on the old
  `to_inner`+`persist_table_interner` pair. Plain `persist_table_interner`
  KEPT (remaining callers had no adjacent interning — spot-checked
  `catalogue.rs:129`, `admin_replication.rs` sites). Doc claims in
  `read_only_views.rs` (module header + struct doc) are now TRUE: every
  method reads or idempotently flushes; nothing mutates.
- **L4** ✓ `table/mod.rs:53-56`: `InternerReadView` + `ValidatorRegistryReadView`
  added to `pub use table_manager::{...}`; `table_manager.rs:40-42` re-exports
  both from the `#[path]` child module. Every import path resolves:
  `crate::engine::table::InternerReadView` (changelog.rs:6),
  `shamir_db::engine::table::InternerReadView` (decode_cache.rs:7,
  filter_eval.rs:3, bridge.rs:10, and the three server test files
  filter_eval_tests.rs:5 / filter_lens_parity_tests.rs:14 /
  target_match_tests.rs:5).
- **L5** ✓ `catalogue.rs:129` reads `self.system_store()` inside
  `install_seed_profile` (the undefined `home` is gone; `catalogue.rs:91`'s
  `home` is the legitimate fn parameter of `persist(home: &ShamirDb, ..)` —
  untouched, correct).
- **L6** ✓ `migration_index2.rs::assert_committed_crash_retry`:
  first `admit_pair` (:393) held ONLY for `final_drain_and_commit_admitted`
  (:400), then `drop(src_permit); drop(dst_permit);` (:405-406) BEFORE
  `seed_log_from_data_store` (:411) / `flush_history` (:413). The
  `seed_marker` branch re-admits a FRESH pair (:413-417) for
  `complete_finalisation_step_admitted` (:420) and drops both (:423-424)
  BEFORE `bulk_populate_index2` (:435) and the `commit_migration` batch
  (:456). No permit is live across any self-admitting call → no
  non-reentrant deadlock. All assertions preserved verbatim (the dirty diff
  for this file is +12 permit-only lines). Second test in the file: zero
  permit usage. `migration_source_freeze_tests.rs`: zero `admit_pair`/permit
  usage (grep) — read-only APIs only.
- **L7** ✓ `ci.yml:113-114`: the doctest step now has
  `env: AFFECTED: ${{ steps.affected.outputs.args }}` (identical to the
  sibling lib-tests step :99-100); the run block consumes `$AFFECTED`
  (workspace branch + `*shamir-engine*` case). `table_manager.rs` doc: the
  single 4-accessor compile_fail block is split into FOUR separate
  ` ```compile_fail ` blocks, each naming exactly ONE accessor
  (`info_store`, `data_store`, `interner`, `validator_registry`) — reopening
  any single one independently fails the gate.

### Admit-site table (production, cross-crate entry → chain)

| Entry | Chain | Permit |
|---|---|---|
| `admin_migration` START | private dst factory → `admit_pair` → `get_table_admitted` ×2 → `begin_migration_freeze_admitted` (BEFORE interner copy — F4 ordering) → `replicate_*_admitted` → `migration_coordinator_admitted(freeze,…)` → admitted twins → `publish_repo_admitted` → drop permits | ✓ both, whole span |
| `admin_migration` COMMIT | state read → `admit_pair` → lock + state re-check → admitted twins + `seed_current_versions_admitted`/`reset_and_populate_index2_admitted` | ✓ |
| `admin_migration` ROLLBACK / RECOVERY | `admit_pair` (dst optional) / `migration_coordinator_recovered_admitted` (freeze-free, domains captured) | ✓ |
| `SystemStore::persist_table_interner_interning` (save_database, save_database_meta) | admits SYSTEM repo once → `intern_query_value_admitted` → `persist_interner_admitted` (Round 7) | ✓ single admit |
| `SystemStore::persist_table_interner` (16 remaining callers) | admits once → `persist_interner_admitted` | ✓ |
| `catalogue.rs:129` | `self.system_store().persist_table_interner` (Round 7 fix) | ✓ |
| migration_index2 tests | permit lifetime now ends before every self-admitting call (L6) | ✓ |
| coordinator admitted twins | `debug_check_admission` against domains captured in BOTH factories | ✓ |
| engine-internal leaders (RecordCounter::increment, commit path, drainer) | deliberately permit-free (documented design) | by design |

No permit-free mutation is reachable from outside shamir-engine (grep
verified: zero cross-crate raw-getter calls; membership
add/remove/get/rename all gated; `write_ddl_op_status_admitted`,
`drop_all_indexes_for_table_removal_admitted` permit-threaded).

### Round-6 open items — re-checked, ALL STILL OPEN (P2 debt, not fixed, per brief)

1. `TableManager::set_validator_registry(&mut self, Arc<ValidatorRegistry>)`
   still `pub` (`table_manager_validators.rs:107`); cross-crate callers
   remain (`table_resolver.rs:23`, `table_management.rs:376,395`) plus many
   test/bench callers.
2. `DbInstance::add_repo_with_durability` (:58), `remove_repo` (:161),
   `rename_repo` (:176, sync) still pub and permit-free;
   `publish_repo_admitted` (:79) remains the only gated insert.
3. `RepoInstance::repo_interner` still `pub` (`repo_instance.rs:810`) and
   `InternerManager::interner_cell` still `pub`
   (`interner_manager.rs:137`) — the raw-cell escape one hop off the audited
   path.

### Findings (this round)

- **F7-1 (P3, cosmetic)** — `write_domain_index_core.rs:132-133`:
  `self.write_domain.debug_check_permit(permit);` appears TWICE back-to-back
  in `create_unique_index_admitted`. Harmless (no-op debug check, no lint
  fires on duplicated statements), but should be deduped.
- **F7-2 (P3, observation)** — `write_domain_barrier.rs:405-411`: two
  successive "S5c-X: open-path recovery…" comment blocks, the first
  superseded by the second (stale-comment churn from the moved body).
  Harmless; tidy-up candidate.
- **F7-3 (P3, carried)** — all identity checks remain `debug_assert`-only
  (`RepoWriteDomain::debug_check_permit`,
  `MigrationCoordinator::debug_check_admission`): in `--release` a
  wrong-domain permit is silently accepted. Consistent with the established
  engine pattern; unchanged this round.
- **F7-4 (P2, carried, still open)** — the three Round-6 open items above.

### Compile-readiness (by reading; no rustc)

- All new/changed `use`s resolve: `admitted_ops.rs` imports
  `query_value_to_inner` + `{InnerValue, QueryValue}` (both used);
  `read_only_views.rs` dropped the now-unused `query_value_to_inner` import
  while keeping `InnerValue` (still used by `inner_to_query_value` and the
  matches paths); `system_store.rs` already imports `QueryValue`;
  `write_domain_online_index.rs` imports `PhaseBAResult` from its sibling
  module path. `RepoWritePermit` imported in every module that takes it.
- Arities/receivers: `intern_query_value_admitted(&self, permit, value)`;
  `persist_table_interner_interning(&self, &TableManager, &QueryValue)`;
  `phase_b_a_backfill(&self, name, index_def, batch_size, permit)` — call
  sites (`write_domain_index_core.rs:51,56`) match; `admit_pair`
  restores caller argument order.
- No struct-literal breaks: `WriteBarrierGuard::set` gained a 4th param
  `permit: Option<RepoWritePermit>` — both construction sites updated
  (`_impl` passes `None`; `retaining_permit` for the self-admitting path);
  `FilterContext` literal form unused anywhere.
- `Send` across `.await`: `persist_table_interner_interning` holds only the
  owned `RepoWritePermit` (Send: `OwnedMutexGuard<()>` is Send) across
  awaits; `save_database`'s `rec` is owned. No non-Send guard is held
  across `.await` in any changed fn.
- Clippy `-D warnings` traps: needless_borrows fixed (L1.3);
  `pub(crate) phase_*` no longer expose `pub(crate) PhaseBAResult`
  (private_interfaces closed); `intern_query_value_admitted` is `pub` but
  returns `pub` types only; `persist_table_interner_interning` is
  `pub(crate)` in shamir-db taking `&TableManager` (pub type) — no
  private_interfaces. No unused imports remain (grep-verified per file).
  Full-engine `pub fn`-returning-`pub(crate)`-type recheck beyond the two
  L1 sites: none found by reading every new signature in the seven
  admission modules, `table_manager.rs`, `coordinator.rs`, `shadow_log.rs`,
  `db_instance.rs`, `repo_membership_impl.rs`, `write_domain_membership.rs`.

### Per-file inventory (all 212 files, individually)

Legend: **[7]** = Round-7 dirty diff read in full this round;
**[F]** = full diff read this round; **[S]** = symmetric test/bench/doc diff
— covered by (i) the whole-diff removed-line assertion scan (every `-`
`assert*` line across the entire committed diff was extracted and matched to
its added counterpart — 22 removed vs 135 added; each removal is an accessor
substitution or a documented strengthening, e.g. `wal.recover().len()==1` →
`== inflight_before` + `assert_eq!(inflight_before.len(), 1)`; the
`interner_cell` `debug_assert` fallbacks were removed together with the cell
in the read-view rewrite, replaced by the always-populated
`InternerReadView`), (ii) the Round-6 audit's full reads, and (iii)
representative full reads; **[OK]** = no finding; **[F7-n]** = finding.

1. `.github/workflows/ci.yml` — [7][F] L7 env mapping + doctest gate. OK
2. `crates/shamir-db/Cargo.toml` — [S] test-util feature chain. OK
3. `…/execute/admin_describe.rs` — [S] read-view rewiring. OK
4. `…/execute/admin_list.rs` — [S] read-view rewiring. OK
5. `…/execute/admin_migration.rs` — [F] admit_pair/freeze-before-interner-copy/admitted twins/publish_repo_admitted; F5.1 `?`→BatchError verified (`admit_pair` recovery uses `.map_err(|e| err(e.to_string()))?`). OK
6. `…/execute/admin_replication.rs` — [S] persist/read-view rewiring (5 persist sites admitted). OK
7. `…/execute/admin_schema.rs` — [S] single `schema_repo_permit` per DDL op. OK
8. `…/execute/admin_table_index.rs` — [S] admitted status writes; permit dropped before re-admitting sweep. OK
9. `…/execute/helpers.rs` — [S] helper rewiring. OK
10. `…/execute/replication_seed/catalogue.rs` — [7] L5 `self.system_store()`. OK
11. `…/execute/repo_admin/admin_schema_admission.rs` — [S] helpers moved verbatim. OK
12. `…/shamir_db/changelog.rs` — [S] `InternerReadView` decode path. OK
13. `…/shamir_db/core.rs` — [S] `_checked` create/drop wiring. OK
14. `…/shamir_db/schema_management.rs` — [S] read-view rewiring. OK
15. `…/shamir_db/seed_catalogue.rs` — [S] read-view rewiring. OK
16. `…/shamir_db/seed/subscriptions.rs` — [S] read-view rewiring. OK
17. `…/shamir_db/table_management.rs` — [S] `get_table_admitted`/`compile_table_schema` facade; `set_validator_registry` wiring (open P2 item #1). OK
18. `…/shamir_db/tests/keyset_safe_write_barrier_tests.rs` — [S] fixture accessors. OK
19. `…/shamir_db/tests/schema_rollback_tests.rs` — [S] +admission test (additive). OK
20. `…/shamir_db/system_store.rs` — [7][F] L3 single-admission chain; imports OK. OK
21. `…/shamir_db/tests/clean_horizon_tests.rs` — [S] fixture accessors. OK
22. `…/shamir_db/tests/p1065_ddl_status_contract_tests.rs` — [S] read-view op-status. OK
23. `…/shamir_db/tests/p1067_ddl_family_classification_tests.rs` — [S] read-view op-status. OK
24. `…/shamir_db/tests/r0c_cross_family_drop_rename_tests.rs` — [S] read-view/permit. OK
25. `…/shamir_db/tests/replication_ddl_tests.rs` — [S] permit threading. OK
26. `…/shamir_db/tests/shutdown_quiesce_tests.rs` — [S] fixture accessors. OK
27. `crates/shamir-db/tests/ddl_wire_e2e/admission_barrier_tests.rs` — [S] permit/e2e. OK
28. `…/ddl_wire_e2e/drop_index_index2_support.rs` — [S] accessor. OK
29. `…/ddl_wire_e2e/drop_index_unified_resolution.rs` — [S] accessor. OK
30. `crates/shamir-db/tests/migration_index2.rs` — [7] L6 deadlock fix; assertions intact; `migration_source_freeze_tests` clean. OK
31. `crates/shamir-db/tests/migration_source_freeze_tests.rs` — [S] zero permit usage (grep). OK
32. `crates/shamir-db/tests/query_index2.rs` — [S] permit threading. OK
33. `crates/shamir-db/tests/retention_ddl.rs` — [S] +58 additive pending-until-admit test. OK
34. `crates/shamir-db/tests/validators_lifecycle.rs` — [S] accessor. OK
35-48. engine benches (14): `batch_stage_parallelism`, `cursor_pages_depth`, `fk_cascade_index`, `for_each_eval`, `fts_indexed`, `keyset_seek`, `p1107_stale_value_gate`, `p1108_released_unique_growth`, `read_path_matrix`, `select_pipeline`, `streaming_topk`, `tx_aware_reads`, `tx_pipeline`, `when_skip_eval` — [S] `required-features=["test-util"]` + fixture accessors (`interner_for_test`, `FilterContext::new` reads, `set_validator_registry` fixtures); no assertion content. OK (each enumerated individually above — no wildcard)
49. `crates/shamir-engine/Cargo.toml` — [S] doctest=true + test-util feature. OK
50. `…/db_instance/db_instance.rs` — [F] checked twins + `publish_repo_admitted`; **add/remove/rename still open (P2 item #2)**. OK-with-carried-P2
51. `…/db_instance/tests/mod.rs` — [S] mod wiring. OK
52. `…/db_instance/tests/write_domain_membership_tests.rs` — [S] new additive tests. OK
53. `…/meta/tests/lifecycle/lifecycle_record_tests.rs` — [S] accessor. OK
54. `…/migration/coordinator.rs` — [F] F3 demotions + admitted twins + `with_domains`. OK
55. `…/migration/shadow_log.rs` — [F] `append`/`append_batch`/`purge` `pub(crate)`; zero non-engine callers (grep). OK
56. `…/repo/changelog_store/repo_membership_impl.rs` — [F] `get_table` self-admits; `add/remove_table` `pub(crate)`+permit; `*_for_test` crate-gated. OK
57. `…/repo/changelog_store.rs` — [S] module wiring. OK
58. `…/repo/changelog_store/tests/mod.rs` — [S] mod wiring. OK
59. `…/repo/changelog_store/tests/write_domain_tests.rs` — [S] new additive tests. OK
60. `…/repo/changelog_store/write_domain_membership.rs` — [F] admitted membership twins. OK
61. `…/repo/changelog_store/write_domain.rs` — [F] domain/permit/reservation/admit_pair (ascending token order, same-instance rejected); unforgeable permit compile-fail fence. OK
62. `…/repo/durability_policy/durability_policy/repo_impl.rs` — [S] factory-open admits once. OK
63. `…/repo/mod.rs` — [S] re-exports. OK
64. `…/repo/repo_instance.rs` — [S] `rename_table_stores(_admitted)`; **`repo_interner` still pub (P2 item #3)**. OK-with-carried-P2
65. `…/repo/seed_install/writer.rs` — [S] seed install admits once. OK
66. `…/repo/tests/per_table_mvcc_token_names_deadlock_tests.rs` — [S] additive. OK
67. `…/repo/tests/repo_instance_tests.rs` — [S] fixture accessors. OK
68. `…/repo/tests/repo_tx_accessors_tests.rs` — [S] fixture accessors. OK
69. `…/repo/tests/table_init_retry_tests.rs` — [S] contended-admission Notify adaptation; assertions strengthened. OK
70. `…/table/doctor.rs` — [S] `repair` self-admits; `repair_admitted` single-admission (verified at `write_domain_barrier.rs:512`). OK
71. `…/table/mod.rs` — [7] L4 re-export. OK
72. `…/table/record_counter.rs` — [S] counter admits (`new_with_domain`). OK
73. `…/table/table_manager_buffer.rs` — [S] buffer-config self-admits. OK
74. `…/table/table_manager_index_mgmt.rs` — [F] body split out; `PhaseBAResult` definition remains here (`pub(crate)`). OK
75. `…/table/table_manager_replication.rs` — [S] replicate/bulk-populate self-admit + twins. OK
76. `…/table/table_manager.rs` — [7][F] raw getters demoted, `*_for_test` gated, guard carries permit, compile-fail ×4 (L7), dead `table()` removed (L1.2). OK
77. `…/table/table_manager_sorted_index.rs` — [S] sorted create/drop self-admits. OK
78. `…/table/table_manager_validators.rs` — [S] bindings self-admit; **`set_validator_registry` still pub (P2 item #1)**. OK-with-carried-P2
79-115. engine table tests (37 files): `batch_hoist_invariants_tests`, `doctor_tests`, `f50_index_lifecycle_spike_tests`, `f50_step2_index_lifecycle_tests`, `f57_index_create_barrier_tests`, `f69_write_barrier_single_atomic_tests`, `f70_lock_order_inversion_tests`, `f72_planner_invisibility_tests`, `f76_drop_visibility_tests`, `f95_ddl_admission_tests` (see note), `filtered_ann_tests`, `index2_create_barrier_tests`, `index2_empty_result_tests`, `index_build_explicit_pin_tests`, `index_rename_concurrency_tests`, `migration_freeze_tests`, `non_tx_ack_wait_tests`, `p02_base_index_rederive_tests`, `p1008_instance_provenance_tests`, `p1011_reader_drain_tests`, `p1037_sorted_reader_drain_tests`, `p1038_index2_reader_drain_tests`, `p1060_online_index_crash_recovery_tests`, `p1061_pipeline_property_tests`, `p1063_multi_fts_index_stats_tests`, `p1087_phase_b_a_tests`, `p1088_phase_c_d_tests`, `p1098_gen_read_order_tests`, `p997_hash_rename_durability_tests` (uses `recover_hash_renames_admitted(&permit)` — crate-internal, fine), `r0a_registry_watermark_admission_tests`, `read_one_tx_tests`, `schema_activation_barrier_tests`, `snapshot_vector_tests`, `stream_tx_tests`, `tx_read_shortcut_tests`, `tx_scan_overlay_tests`, `version_cas_tests` — [S] fixture/permit/phase-arity call-site updates; removed-line scan clean (no deleted assertions in any of them). OK (each enumerated individually above — no wildcard)
116. `…/table/tests/write_receipt/crud_tests.rs` — [S] fixture accessors. OK
117. `…/table/tests/write_receipt/mod.rs` — [S] mod wiring. OK
118. `…/table/tests/write_receipt/write_domain_admission_matrix_tests.rs` — [S] new additive (443 lines). OK
119. `…/table/tests/write_receipt/write_domain_barrier_tests.rs` — [S] new additive. OK
120. `…/table/tests/write_receipt/write_domain_counter_tests.rs` — [S] new additive. OK
121. `…/table/tests/write_receipt/write_domain_shared.rs` — [S] new fixture helper. OK
122. `…/write_receipt/admission/admitted_ops.rs` — [7][F] read in full; L3 admitted interning; factories; `debug_check_permit` on every fn. OK
123. `…/write_receipt/admission/read_only_views.rs` — [7][F] read in full; `to_inner` gone; doc claims TRUE. OK
124. `…/write_receipt/admission/write_domain_barrier.rs` — [7][F] read in full; L1.3 borrows; moved create body line-matches origin; stale duplicated comment (F7-2). F7-2
125. `…/write_receipt/admission/write_domain_index_admitted.rs` — [S] create/drop sorted + index2 status plumbing (Round-6 read; signature recheck this round). OK
126. `…/write_receipt/admission/write_domain_index_core.rs` — [F] read in full vs origin; admitted twins of create/unique/drop ×3; F7-1 duplicated debug_check (cosmetic). F7-1
127. `…/write_receipt/admission/write_domain_online_index.rs` — [7][F] read in full; bodies line-match origin text (`begin_write_barrier`→`_admitted(…, permit)`, debug_check added, `pub(crate)` L1.1). OK
128. `…/write_receipt/admission/write_domain_rename_admitted.rs` — [F] read in full vs origin; recovery + rename with admitted swaps; `resolve_index_paths` helper verbatim. OK
129. `…/tx/drainer.rs` — [S] drainer permit-free leader path (by design). OK
130. `…/tx/orphan_repair/tests/repair_tests.rs` — [S] fixture accessors. OK
131. `…/tx/recovery.rs` — [S] replay/recovery admits; `recover_inflight_v2(repo, permit)`. OK
132-174. engine tx tests (43 files): `a8_interner_delta_tests`, `acceptance_tests`, `backpressure_gc_tests`, `base_index_tx_tests` (two ```text conversions — hygiene only), `commit_async_visibility_tests`, `commit_phase5_defer_tests`, `commit_phase5_tests`, `commit_tests`, `cutover_tests`, `drainer_tests`, `f73_rederive_fail_closed_tests`, `f74_bump_apply_ordering_tests`, `gc_tests`, `index_rollback_tests`, `interner_checkpoint_tests`, `journal_publish_order_tests`, `non_tx_mvcc_routing_tests`, `oracle_stress_tests`, `p1096_tx_aware_unique_check_tests`, `p1097_remove_posting_owner_tests`, `p1100_stale_snapshot_delete_posting_tests`, `p1101_released_skip_durable_check_tests`, `pessimistic_lock_tests`, `pessimistic_lost_update_tests`, `post_wal_orphan_replay_tests`, `read_your_own_writes_tests`, `recovery_tests` (len()==1 → `== inflight_before`, strengthened), `rederive_stale_value_dedup_cache_tests`, `repo_diagnostics_tests`, `snapshot_stress_tests`, `snapshot_visibility_park_tests`, `ssi_phantom_tests/{gc,integration,mvcc1_fix,perf233,phase_2bis,predicate_capture,update_phantom}_tests`, `ssi_stress_tests`, `ssi_unique_serialization_tests`, `truncation_tests`, `tx_commit_writer_drain_tests`, `tx_vector_delete_tests` — [S] fixture/permit call-site updates; removed-line scan clean. OK (each enumerated individually above — no wildcard)
175. `…/validator/persistence.rs` — [S] `save_validators_metadata` `pub(crate)`. OK
176. `crates/shamir-server/Cargo.toml` — [S] test-util dev-dep. OK
177. `…/db_handler/cursor_handlers.rs` — [S] engine-side `build_filter_context`, `validator_registry_read_view`, probe via read view. OK
178. `…/doctor.rs` — [S] `create_table_checked`, read-view name resolution. OK
179. `…/replication/seed/admission.rs` — [S] `filter_context` engine-side. OK
180. `…/replication_seed/export/exporter.rs` — [S] read-view introspection + `mvcc_snapshot_stream`. OK
181. `…/replication/seed/fence.rs` — [S] `filter_context` engine-side. OK
182-186. replication_seed tests (5): `install/subscription/tests/disk_tests`, `install/tests/disk_tests`, `tests/artifact_tests`, `tests/bounds_tests`, `tests/disk_export_tests` — [S] accessor/`*_for_test`. OK
187. `…/replication/seed/tests/loop_tests.rs` — [S] accessor. OK
188. `…/replication/supervisor.rs` — [S] read-view rewiring. OK
189-191. replication tests (3): `follower_loop_tests`, `read_after_replication_tests`, `supervisor_tests` — [S] accessor. OK
192. `…/server/server_launcher.rs` — [S] `create_table_checked`. OK
193. `…/subscriptions/bridge.rs` — [S] `Arc<InternerReadView>` end-to-end. OK
194. `…/subscriptions/decode_cache.rs` — [S] cache type → `InternerReadView` (L4 import path resolves). OK
195. `…/subscriptions/filter_eval.rs` — [S] eval via read view (L4 import path resolves). OK
196. `…/subscriptions/target_match.rs` — [S] read-view rewiring. OK
197-199. subscription tests (3): `filter_eval_tests`, `filter_lens_parity_tests`, `target_match_tests` — [S] `from_interner_for_test` (L4 import paths resolve). OK
200. `…/tests/shutdown_truth_tests.rs` — [S] accessor. OK
201-204. server integration tests (4): `tests/db_handler.rs`, `tests/doctor_e2e.rs`, `tests/hmac_gate.rs`, `tests/user_admin_port_wire.rs` — [S] accessor. OK
205-209. remediation ledgers 108–112 — [S] docs. OK
210. `docs/dev-artifacts/prompts/remediation/113-s5cx-round7.md` — [7] this brief + implementer inventory. OK
211. `docs/guide-docs/follower-journal-durability.md` — [S] docs. OK
212. Round-7 dirty-only files already listed above with [7]: `eval_context.rs` (L2), `table/tests/table_manager_tests.rs` (L1.2 rewiring, assertion unchanged).

(Files dirty in BOTH the committed diff and the working tree — `ci.yml`,
`catalogue.rs`, `system_store.rs`, `migration_index2.rs`, `table/mod.rs`,
`table_manager.rs`, `admitted_ops.rs`, `read_only_views.rs`,
`write_domain_barrier.rs`, `write_domain_online_index.rs` — carry both
tags; the [7] diff was read on top of the committed state, so both layers
were inspected.)

### Coverage statement

**Inspected in full this round (hunk-by-hunk):** the entire Round-7 dirty
diff (12 files, every hunk); the full current text of all seven admission
modules (`admitted_ops`, `read_only_views`, `write_domain_barrier`,
`write_domain_index_admitted` recheck, `write_domain_index_core`,
`write_domain_online_index`, `write_domain_rename_admitted`) with the three
moved bodies compared against their pre-move text in
`git show origin/master:crates/shamir-engine/src/table/table_manager_index_mgmt.rs`;
full committed diffs of `table_manager.rs`, `admin_migration.rs`,
`coordinator.rs`, `shadow_log.rs`, `db_instance.rs`, `tx/recovery.rs`;
`write_domain.rs`, `repo_membership_impl.rs` /
`write_domain_membership.rs` current text; and — across ALL 212 files —
the complete removed-line scan of the committed diff (every removed
`assert*`/`debug_assert*` line workspace-wide extracted and matched; 22
removed vs 135 added; no assertion deleted or weakened — the only removed
regression content is the F-3/P2-documented `f95` concurrency rewrite and
the accessor substitutions).

**Not inspected hunk-by-hunk this round (stated precisely, instead of
claiming coverage):** the ~140 purely symmetric test/bench/doc diffs
(items tagged [S] above) were verified by the exhaustive removed-line scan
+ Round-6 full reads + targeted greps (cross-crate raw getters, `.to_inner`,
`.table()`, `.interner` readers, `pub async fn phase_*`, permit usage) —
NOT by re-reading every added line of each hunk this round; their ADDED
lines outside assertions/fixture-accessors were not individually re-read.
Everything else — every production source file, every new admission module,
all Round-7 targets, CI, both manifests — was inspected in full. No builds,
tests, clippy, doctests, or git mutations were run; compile-readiness was
verified by reading only.
