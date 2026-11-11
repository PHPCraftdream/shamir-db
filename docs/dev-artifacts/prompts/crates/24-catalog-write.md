# Brief: system catalogue writes — one write semantics for the function row, durable ACL/DDL acknowledgements (CATALOG-WRITE)

## Why

The sprint review found two related defects in `crates/engine/shamir-db/src/shamir_db/system_store/`.

**A. Two write semantics for one function row.** `SystemStore::save_function` (`identities.rs`) replaces the
whole row: delete + insert in one `run_implicit_batch_tx`, then `flush_system_repo_durable`. That replaced an
earlier merge-SET, which let fields of the old row (e.g. `call_signature`) survive a replacement.
`SystemStore::save_function_meta_record` (`validators.rs`) still writes with a merge-SET (`SetOp` through
`set_via_implicit_tx`), with a plain data-store `flush()` and without `catalogue_write_fault_gate`. Its only
production caller is `update_function_resource_meta` (`shamir_db/shamir_db/access/meta.rs`, under the per-name
function DDL lock), which writes the fully loaded row plus `ResourceMeta`, so it is correct today, but two write
semantics for one row is the trap that already produced the `call_signature` bug. Several test fixtures write raw
rows through it and must call `remove_function` first because of the merge.

**B. Acknowledged catalogue writes that may not be durable.** "Durable DDL" is implemented once, as
`SystemStore::flush_system_repo_durable` (`store.rs`): `sync_wal` of the SYSTEM repo, then the table's data-store
`flush()`. The system repo opens with the same `DurabilityPolicy` as user repositories; the default commit tier is
`WalCommitTier::Buffered` (`crates/engine/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy.rs`).
Many acknowledged catalogue writes only run the implicit tx plus a plain data-store `flush()`, without `sync_wal`:

* `catalogue/ops.rs`: `save_repository`, `remove_repository`, the write near line 248, `remove_table`,
  `save_database_meta` (database chmod/chown/chgrp), `save_repository_meta`, the write near line 687;
* `identities.rs`: `save_group` (group membership), `remove_group`;
* `validators.rs`: `save_validator`, `remove_validator`, `save_function_meta_record`, `save_function_folder`,
  `remove_function_folder`, `save_function_folder_meta`.

If a plain `flush()` does not make the implicit-tx write durable, a revocation (chmod tightening, removal from a
group) is acknowledged to the client but can be lost on power failure, which silently restores a right.

Read first: `system_store/store.rs` (`flush_system_repo_durable`, `set_via_implicit_tx`,
`catalogue_write_fault_gate`, `durability_tail_fault_gate`), `system_store/identities.rs`,
`system_store/validators.rs`, `system_store/catalogue/ops.rs`, `shamir_db/shamir_db/access/meta.rs`,
`shamir_db/shamir_db/tests/catalogue/catalogue_lifecycle_tests.rs` (the `rv_db_0486e3f9_*` tests and
`FlushFaultStore`), the implicit-tx path of `RepoInstance::run_implicit_batch_tx` and how its WAL records reach the
data store, and `crates/storage/shamir-storage/src/stores/storage_fjall.rs` (`flush` = `persist_sync_all`).

## What

1. **Establish B from the code first.** Determine whether, after `run_implicit_batch_tx` returns, the committed
   write is already in the table's data store (so `flush()` = fsync covers it) or only in the WAL (applied later by
   a drainer), under the `Buffered` tier. State the answer with file/line evidence in your final report.
   * If a plain `flush()` is NOT sufficient: route every write listed in B through `flush_system_repo_durable`
     (keep `persist_table_interner` where it is today, before the durable tail, as `save_function` does). Add
     `catalogue_write_fault_gate()` only where the sibling write of the same table already has it; say which.
   * If it IS sufficient: change nothing in B; record the evidence in the doc comment of
     `flush_system_repo_durable` (why some tails call `sync_wal` and others do not).
2. **A: one semantics.** `save_function_meta_record` replaces the whole row exactly like `save_function`
   (delete + insert in one `run_implicit_batch_tx`, interner persist, durable tail), or the caller moves to
   `save_function` with the record's own meta; pick the smaller diff and say why. Remove the now-unneeded
   `remove_function` calls in the fixtures that worked around the merge (list them).
3. No behaviour change on any DDL path besides durability; no public API change; no new dependencies.

## Tests (run in CI, not here)

* Next to the `rv_db_0486e3f9_*` tests (respect the folder limit; add a sibling file under
  `shamir_db/tests/catalogue/` if `catalogue_lifecycle_tests.rs` would pass 1000 lines): an armed `sync_wal` fault
  (`RepoInstance::arm_sync_wal_fault`) surfaces as an error from (a) database chmod, (b) a group membership change,
  (c) a function meta update — only if item 1 concluded the tails must change.
* A function meta update after a raw row with an extra field leaves no field that is absent from the new record
  (proves replacement, not merge).
* Never delete or weaken existing tests; list every changed one with the reason.

## Observable acceptance

fmt/check/clippy clean for `shamir-db`; `git add -A` then `python -I scripts/ci/check_layout.py` passes (<=7
entries per folder, <=1000 lines per file; rerun on the final tree); rust-guard rules (no logic in `mod.rs`, no
inline test modules, no local `use`, no new `#[allow]`, no visibility widening — list any).

## Constraints

Compilation, builds and tests run in CI, not here. Allowed locally only: `cargo fmt -p <crate>`,
`cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --` with
`-j 2`. No `cargo build/test/run/bench/doc`, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: the answer to item 1 with evidence, what changed, tests added/changed and
why, anything unverified. Do not end on a tool call.
