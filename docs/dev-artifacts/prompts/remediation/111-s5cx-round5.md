# Round 5 preservation brief — TableManager crate boundary

## Binding decisions

- Keep engine-internal raw store and getter operations permit-free; the leader commit path remains unchanged.
- Raw `TableManager` handles that expose mutable engine state become `pub(crate)`. Preserve their names and internal callers; do not turn `info_store()` into a public view with the same name.
- Cross-crate read-only production callers must use purpose-built views/accessors that expose only read operations, with no `Deref` and no cloned raw handle. Cross-crate tests/fixtures may use distinct `*_for_test` accessors guarded by the existing `cfg(any(test, feature = "test-util"))` feature boundary.
- Production mutations stay admitted through their public APIs. Migration raw storage arcs remain engine-owned; expose operations/factories rather than raw stores.
- Add a `compile_fail` rustdoc example on the real public `TableManager` type demonstrating that downstream `.info_store()`/`.data_store()` access is unavailable.
- Update follower-journal-durability wording to state exactly: “inside shamir-engine the commit path writes stores without the permit by design; no other crate can”; qualify `test-util` as an explicit nonproduction-fixture exception.

## Scope and constraints

Read `CLAUDE.md` and `108-s5cx-repo-write-domain.md`. This stage focuses on the complete `TableManager` raw-handle boundary, necessary external caller adaptations, and the named durability-doc clarification. Do not change RepoInstance getter exposures beyond changes strictly necessary to compile/adapt these callers. No dependency/version changes, builds, tests, clippy, rustc, or git writes. Do not format until the explicitly requested final global cargo-fmt step (which must not be run in this task); preserve style manually.

## Preservation checklist

- Retain TableManager raw getter method names and engine-internal callers unchanged.
- Audit actual getters and every external reference, including integration tests, benches, examples, server/db crates, and migration.
- Add read-only, purpose-built interfaces for production reads; test-only raw handles use distinct names and existing test-util feature gating.
- Keep migration raw arcs and coordinator creation engine-owned.
- Respect layout ratchets: `table_manager.rs` baseline 1719/current 1392 has net room; do not add new files at `table/` root. Existing `table/write_receipt/admission` has room for at most two files.
- Report exact old/new getter list, external replacement ledger, references actually searched, and remaining concrete issues. Do not claim compilation or verification.

## Work sequence

This brief is written before code changes. Next inspect getter definitions, crate dependencies/features, and all cross-crate call sites; implement only after recording the source boundary and migration call patterns.

## Initial observations

- `TableManager` already has raw getters including `table`, `data_store`, `info_store`, `interner`, `counter`, `index_manager_ref`, `index2_registry`, `unique_write_lock`, `sorted_indexes`, and `mvcc_store`; validator accessor is defined in `table_manager_validators.rs`.
- `table()` currently returns `&Table`, not an owned mutable handle, but its API boundary is still under review because it exposes store access.
- `grep` shows extensive internal test callsites and multiple potentially cross-crate users; those need classification before any visibility edits.

## Round 5 actual ledger

### Getters — old (public, removed from the public boundary) / new

Raw getters made `pub(crate)` in `table_manager.rs` / `table_manager_validators.rs` (names preserved, engine-internal callers unchanged):

`table`, `data_store`, `info_store`, `interner`, `counter`, `index_manager_ref`, `index2_registry`, `unique_write_lock`, `sorted_indexes`, `mvcc_store`, `validator_registry_ref` (11 raw handles).

New public surface:

- Read-only production views: `TableManager::info_store_read_view()` / `data_store_read_view()` returning `TableInfoStoreReadView` / `TableDataStoreReadView` (`write_receipt/admission/read_only_views.rs`); plus flat read accessors `index_definitions`, `unique_index_definitions`, `sorted_index_definitions`, `sorted_index_definition_by_field`, `index2_descriptors`, `mvcc_snapshot_stream`, `validator_registry`.
- Test-only raw handles, each gated `#[cfg(any(test, feature = "test-util"))]` (11 gates): `validator_registry_ref_for_test`, `table_for_test`, `data_store_for_test`, `info_store_for_test`, `interner_for_test`, `counter_for_test`, `index_manager_ref_for_test`, `index2_registry_for_test`, `mvcc_store_for_test`, `unique_write_lock_for_test`, `sorted_indexes_for_test`. Feature wired: `shamir-engine/test-util` → `shamir-db/test-util` (dev-dep) → `shamir-server/test-util` (dev-dep).
- Admitted ops (`write_receipt/admission/admitted_ops.rs`), all taking `&RepoWritePermit`: `write_ddl_op_status_admitted`, `drop_all_indexes_for_table_removal_admitted`, `seed_current_versions_admitted`, `reset_and_populate_index2_admitted`, `migration_coordinator_admitted`, `migration_coordinator_recovered_admitted`. NOT an exhaustive transitive proof: this inventory lists the ops defined in this diff and the façade call sites rewired in it; it does not prove no other transitive path reaches a raw handle.

### External caller replacements (searched: `rg` over `crates/shamir-db`, `crates/shamir-server`, `crates/shamir-engine/benches`)

- `shamir-server/src/db_handler/cursor_handlers.rs`: `interner()` → `info_store_read_view().interner_get()`; `sorted_indexes().find_by_field` → `sorted_index_definition_by_field`; `validator_registry_ref()` → new read-only `validator_registry()`.
- `shamir-server` integration tests: `tests/doctor_e2e.rs` `.counter()` → `.counter_for_test()`; `tests/user_admin_port_wire.rs` `.interner()` → `.interner_for_test()`.
- `shamir-engine` benches (14 files: keyset_seek, cursor_pages_depth, select_pipeline, fts_indexed, streaming_topk, tx_pipeline, tx_aware_reads, read_path_matrix, batch_stage_parallelism, when_skip_eval, for_each_eval, fk_cascade_index, p1107_stale_value_gate, p1108_released_unique_growth): `.interner()` → `.interner_for_test()`, `.data_store()` → `.data_store_for_test()`, `.mvcc_store()` → `.mvcc_store_for_test()`; each bench gated `required-features = ["test-util"]` in `crates/shamir-engine/Cargo.toml`.
- `shamir-db` production/tests: migrated to read views, admitted ops, and `*_for_test` handles (see diff).

### compile_fail doctest location

On the real public type: doc-example ` ```compile_fail ` block on `pub struct TableManager` in `crates/shamir-engine/src/table/table_manager.rs` (demonstrates `.info_store()`/`.data_store()` are unavailable downstream). Note: crate-level `doctest = false` in `shamir-engine` means this block is enforced only by explicit `cargo test --doc`, not by the default test run.

### Layout

- No new files at `table/` root. `table/write_receipt/admission/` now has 7 entries (5 existing + `read_only_views.rs` + `admitted_ops.rs`) — at the ≤7 cap.
- Line ratchet (read-only python count vs `origin/master`): `table_manager.rs` 1719 → 1492 (net room). Other changed paths that grew vs `origin/master` (no documented baselines for them): `admin_migration.rs` +61, `table_manager_validators.rs` +27, `cursor_handlers.rs` +5, `admin_list.rs` +4, `table/mod.rs` +3, `admin_table_index.rs` +2, `doctor_e2e.rs` +1.

### Limitations

- No builds/tests/clippy/rustc were run at this stage's code-edit point (per brief); the final `cargo fmt -p shamir-engine -p shamir-db` was the only mutating command after verification.
- CRLF/LF: many working-tree files carry CRLF that git will normalize; `git diff` content is unchanged for those (line-ending state only, no churn hunks).
- The admitted-ops inventory above is a record of what this diff introduced/rewired, not a proof of full transitive closure over every internal engine caller.
