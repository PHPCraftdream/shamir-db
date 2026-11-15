# Sub-brief 38b of R2c1b — catalog group onto `RepoCore` (ENGINE-LAYERING R2c1b, worker 2)

Read the root CLAUDE.md (binding) and docs/dev-artifacts/prompts/crates/38-engine-layering-r2c1b.md (the parent brief). You implement worker-2's share: move the catalog-resolution method bodies of the `RepoInstance` shell onto an inherent `impl RepoCore` in `crates/engine/shamir-engine/src/tx/commit/core/catalog_access.rs` (NEW), leaving delegates on the shell. Behaviour must not change. You own EXACTLY these files:

NEW:
- crates/engine/shamir-engine/src/tx/commit/core/catalog_access.rs
EDIT:
- crates/engine/shamir-engine/src/repo/instance/repo_instance/catalog.rs
- crates/engine/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs
- crates/engine/shamir-engine/src/repo/instance/repo_instance.rs

Do NOT touch tx/commit/core/mod.rs (worker 1 registers all new core modules), tx_horizon.rs, lifecycle*.rs, transactions.rs, or query/**.

## Layer rules

`RepoCore` is a tx-layer type: catalog_access.rs may use repo, table, validator, tx items and external crates only. The bodies below were pre-verified: they name `crate::repo` items (RepoWritePermit, version_provider), `crate::table` items (TableManager, TableConfig, table_token_for, interner via `crate::table::interner_manager::InternerManager`), `shamir_tx`, `shamir_storage`, `tokio`, `std`. NO `crate::query` / `crate::repo::instance` reference may appear in the new file.

## Core methods (impl RepoCore in catalog_access.rs)

Move these bodies verbatim from the shell (rewrite `self.core.catalog.*` → `self.catalog.*`, `self.core.repo` → `self.repo`, `self.core.durability_handle.*` → `self.durability_handle.*`; calls to `self.get_table_unfenced/get_table_admitted_unfenced/get_table_ctx/repo_interner/tx_gate/...` keep working — they become core methods too). Carry the full explanatory doc comments onto the core methods (fix `[`x`](Self::x)` links to shell items: plain backticks or paths that still resolve); shell methods get short docs.

- `table_by_token(&self, token: u64) -> DbResult<Option<TableManager>>` — pub(crate); body from catalog.rs (the DEADLOCK FIX read_sync doc moves with it). Its `self.get_table_unfenced(&name)` call stays (core method).
- `table_by_token_admitted(&self, token: u64, permit: &RepoWritePermit) -> DbResult<Option<TableManager>>` — pub(crate); body from repo_membership_impl.rs.
- `table_by_token_if_live(&self, token: u64) -> Option<TableManager>` — pub(crate) one-liner `self.catalog.table_by_token_if_live(token).await`.
- `get_table_unfenced(&self, table_name: &str) -> DbResult<TableManager>` — pub(crate); body from repo_membership_impl.rs.
- `get_table_admitted_unfenced(&self, table_name: &str, permit: &RepoWritePermit) -> DbResult<TableManager>` — pub(crate); body from repo_membership_impl.rs.
- `get_table_ctx(&self, table_name: &str, permit: &RepoWritePermit) -> DbResult<TableManager>` — pub(crate) (the shell's get_table/get_table_admitted call it); body from repo_membership_impl.rs.
- `create_table_context(&self, table_name: &str, cell: &Arc<tokio::sync::OnceCell<TableManager>>, permit: &RepoWritePermit) -> DbResult<TableManager>` — PRIVATE (its only caller `get_table_ctx` is in the same file); body from repo_membership_impl.rs. Its `self.tx_gate()` / `self.repo_interner()` calls hit the core methods worker 1 creates (tx_access.rs / tx_access.rs `repo_interner`) — keep the calls as `self.tx_gate()` and `self.repo_interner()`.
- `has_table(&self, name: &str) -> bool` — pub(crate) one-liner `self.catalog.has_table(name)`.
- `list_table_names(&self) -> Vec<String>` — pub(crate) one-liner.
- `mapped_table_store_names(&self, name: &str) -> Option<(String, String, String)>` — pub(crate) one-liner.
- `repo_interner(&self) -> DbResult<InternerManager>` — pub(crate); body moved by worker 1 (it goes in tx_access.rs) — do NOT create it; just rely on it.

## Shell edits

- catalog.rs: `table_by_token` becomes a one-line delegate `self.core.table_by_token(token).await` (keep the short doc; the long doc moved to core). `table_by_token_if_live`, `has_table`, `list_table_names`, `mapped_table_store_names` already delegate to `self.core.catalog.*` — LEAVE THEM UNCHANGED. `get_table` keeps the fence checks and fast path but its final line becomes `Box::pin(self.core.get_table_ctx(table_name, &permit)).await` (keep both `check_table_rename_fence` calls, the `initialized_table` fast path and the `admit()` exactly where they are). `get_table_admitted` keeps `debug_check_permit` + fence check, final line `Box::pin(self.core.get_table_ctx(table_name, permit)).await`. `rename_table_stores_admitted`'s single call `self.get_table_admitted_unfenced(from, permit)` becomes `self.core.get_table_admitted_unfenced(from, permit)` (one-line body change). `bump_history_epoch` (pub twin) keeps admit + becomes `self.core.bump_history_epoch_admitted(&permit).await` (the admitted twin moved to core — worker 1 owns its body). `rename_table_transition`, `recover_table_rename_with`, `prepare_table_rename_admitted`, `copy_table_stores_for_rename`, `commit_table_rename_admitted`, `rename_table_stores*`, `repo_identity*`, `apply_restore_operation*` stay UNCHANGED (they call shell methods that still exist).
- repo_membership_impl.rs: `table_by_token_admitted`, `get_table_unfenced`, `get_table_admitted_unfenced`, `get_table_ctx`, `create_table_context` MOVE OUT (delegates / removed as follows): `table_by_token_admitted` shell method becomes `self.core.table_by_token_admitted(token, permit).await`; `get_table_unfenced` shell method becomes `self.core.get_table_unfenced(table_name).await`; `get_table_admitted_unfenced` shell method becomes `self.core.get_table_admitted_unfenced(table_name, permit).await`; `get_table_ctx` and `create_table_context` are REMOVED from the shell entirely (their shell callers now use the core methods; they were private). Keep `add_table_for_test`, `remove_table_for_test`, `add_table`, `add_table_inner`, `remove_table`, `remove_table_inner` UNCHANGED (they stay on the shell this stage). Keep the module doc and adjust it if it names moved items with stale links.
- repo_instance.rs: the free fn `checkpoint_allocation_floor(repo: &RepoInstance)` becomes a one-line delegate: `repo.core.checkpoint_allocation_floor().await` (the core method is worker 1's, in journal_access.rs). Keep its doc comment and visibility (`pub(crate)`).

## Hard rules
Same as the parent brief: no function-local `use` (hoist to file headers, prune/add imports after each edit), no new `#[allow]`, no signature changes on the shell, terse comments, keep big docs with the bodies (fix stale `Self::` links), re-read cfg-gated code carefully, never run cargo (orchestrator gates), read-only git only, never mutate git state. Files under 1000 lines.

## Report (final message)
List the core methods created (with visibility), every shell body you replaced/removed, every import added/pruned per file, and anything you decided beyond this brief.
