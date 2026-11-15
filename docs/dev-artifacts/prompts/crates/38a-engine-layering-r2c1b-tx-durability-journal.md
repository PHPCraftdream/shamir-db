# Sub-brief 38a of R2c1b — tx / durability / journal groups onto `RepoCore` (ENGINE-LAYERING R2c1b, worker 1)

Read the root CLAUDE.md (binding) and docs/dev-artifacts/prompts/crates/38-engine-layering-r2c1b.md (the parent brief). You implement worker-1's share of that brief: move the tx, durability and journal method bodies of the `RepoInstance` shell onto inherent impls of `RepoCore` (and one method onto `TxHorizon`) in `crates/engine/shamir-engine/src/tx/commit/core/`, leaving one-line delegating methods on the shell. Behaviour must not change. You own EXACTLY these files (other workers own the catalog files and transactions.rs — never touch them):

NEW (you create):
- crates/engine/shamir-engine/src/tx/commit/core/tx_access.rs
- crates/engine/shamir-engine/src/tx/commit/core/durability_access.rs
- crates/engine/shamir-engine/src/tx/commit/core/journal_access.rs
EDIT:
- crates/engine/shamir-engine/src/tx/commit/core/mod.rs (register the three new modules; keep the existing pub(crate) use re-exports)
- crates/engine/shamir-engine/src/tx/commit/core/tx_horizon.rs (add one method, see below)
- crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle.rs
- crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/journal.rs
- crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/durability_repo_impl.rs
- crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/clean_horizon_repo_impl.rs
- crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/recovery/cold_reconcile_repo_impl.rs

## Layer rules (rust-guard layers ratchet fails on any new backward pair)

`repo < table < validator < tx < query < instance < db_instance`; a module may refer only to its own and lower layers. `RepoCore` lives in `tx`: its methods may use repo, table, validator and tx items ONLY. Every body you move was pre-verified to name only such items plus external crates (shamir_tx, shamir_wal, shamir_storage, shamir_types, shamir_query_types, tokio, std, log). Do not introduce any `crate::query` or `crate::repo::instance` reference in the new files.

## Method-for-method instructions

In the tables below, "core visibility" is the visibility of the NEW method on `RepoCore`/`TxHorizon`. Shell methods keep their exact current name, signature, visibility and doc contract; where the current doc comment explains WHY (deadlock fixes, cancel-safety), move that doc verbatim onto the core method (replacing any `[`x`](Self::x)` intra-doc link that pointed at a shell item with plain backticks or a full path that still resolves) and leave the shell method a short one-line doc. Shell delegate body: `self.core.<method>(<args>).await` (drop `.await` for sync fns).

### impl RepoCore in tx_access.rs
- `name(&self) -> &str` — NEW pub(crate) one-liner returning `&self.name`.
- `tx_gate(&self) -> DbResult<Arc<shamir_tx::RepoTxGate>>` — pub(crate); body moves verbatim from `transactions.rs` (`RepoInstance::tx_gate`), with `self.core.horizon.tx_gate_cell()` → `self.horizon.tx_gate_cell()`, `self.core.repo` → `self.repo`; the inner `self.max_inflight_commit_version()`, `self.wal_group()` calls stay (they are core methods too now). Its `crate::repo::durability_policy::checkpoint_allocation_floor(self).await?` call becomes `self.checkpoint_allocation_floor().await?` (the core method in journal_access.rs, see below). Move the full doc comment (CRIT-B explanation) with it.
- `tx_metrics(&self) -> &Arc<shamir_tx::TxMetrics>` — pub(crate) one-liner delegating to `self.horizon.tx_metrics()`.
- `commit_tail(&self) -> &Arc<crate::tx::CommitTailBarrier>` — pub(crate) one-liner delegating to `self.horizon.commit_tail()`.
- `commit_visibility_wait(&self) -> shamir_query_types::batch::VisibilityWait` — pub(crate) one-liner delegating to `self.horizon.commit_visibility_wait()`.
- `initialized_tx_gate(&self) -> Option<&shamir_tx::RepoTxGate>` — FIRST add it to `TxHorizon` in tx_horizon.rs as pub(crate) (body: `self.tx_gate_cell().get().map(|gate| gate.as_ref())` — moved verbatim from `transactions.rs`), then the RepoCore one-liner delegates to `self.horizon.initialized_tx_gate()`.
- `tx_info_store(&self) -> DbResult<Arc<dyn Store>>` — pub(crate); body `self.repo.store_get("__tx__").await` (moved from lifecycle.rs). Keep the doc.
- `max_inflight_commit_version(&self) -> DbResult<u64>` — pub(crate); body moves verbatim from transactions.rs (`self.repo_wal()` stays). The shell copy of this method is DELETED (it was pub(crate), its only caller moved with tx_gate) — say so in your report.
- `marker_write_mutex(&self) -> &Arc<tokio::sync::Mutex<()>>` — pub(crate) one-liner returning `&self.durability_handle.marker_write_mutex` (moved from lifecycle.rs).
- `begin_repo_atomic_batch(&self, permit: &RepoWritePermit) -> DbResult<Box<dyn AtomicBatch>>` — pub(crate); body moves verbatim from transactions/follower_durability.rs (`self.core.durability_handle` → `self.durability_handle`, `self.core.repo` → `self.repo`). Keep the doc.
- `storage_supports_atomic_batch(&self) -> bool` — pub(crate) one-liner `self.repo.supports_atomic_batch()` (moved from follower_durability.rs).
- `per_table_mvcc(&self) -> &Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>>` — pub(crate) one-liner returning `&self.catalog.per_table_mvcc` (moved from lifecycle.rs). Keep the doc.

### impl RepoCore in durability_access.rs
- `write_domain(&self) -> &Arc<RepoWriteDomain>` — pub(crate) one-liner (moved from lifecycle.rs).
- `repo_wal(&self) -> DbResult<Arc<shamir_tx::RepoWalManager>>` — pub(crate); body moves verbatim from transactions.rs (all `self.core.durability_handle.*` → `self.durability_handle.*`, `self.core.repo` → `self.repo`, `self.core.wal_segment_bytes` → `self.wal_segment_bytes`, `self.name` stays — RepoCore HAS a `name: String` field, so `self.name` keeps working; the log line reading `self.name` is unchanged). Keep the full doc.
- `wal_group(&self) -> DbResult<Arc<shamir_wal::WalGroupCommit>>` — pub(crate); body moved verbatim. Keep the doc.
- `settle_unwritten_version(&self, version: u64)` — pub(crate); body moved verbatim (calls `self.wal_group()`).
- `initialized_mem_wal(&self) -> bool` — pub(crate) one-liner `self.durability_handle.initialized_mem_wal()`.
- `durability_policy(&self) -> arc_swap::Guard<Arc<DurabilityPolicy>>` — pub(crate) one-liner `self.durability_handle.durability_policy()`. Keep the doc.
- `wal_retention_gate(&self) -> &Arc<RetentionGate>` — pub(crate) one-liner `self.durability_handle.wal_retention_gate()`.
- `wal_retention_refuse_writes(&self) -> bool` — pub(crate) one-liner.
- `reserve_recovery_floor(&self, info: &Arc<dyn Store>, minimum: u64) -> DbResult<u64>` — pub(crate) one-liner into `self.durability_handle.reserve_recovery_floor(...)`.
- `has_persistent_wal(&self) -> bool` — pub(crate) one-liner. Keep the doc.
- `flush_all_history(&self) -> DbResult<()>` — pub(crate); body moved verbatim from lifecycle.rs (`self.core.catalog` → `self.catalog`, `self.core.repo` → `self.repo`, `self.name` unchanged). Keep the full doc.
- `note_unclean_open_bump(&self, permit: &RepoWritePermit) -> DbResult<()>` — pub(crate); body moved verbatim from durability_repo_impl.rs; its `self.bump_history_epoch_admitted(permit)` call now hits the core method below.
- `bump_history_epoch_admitted(&self, permit: &RepoWritePermit) -> DbResult<RepoIdentity>` — pub(crate); body moved verbatim from catalog.rs (`self.core.durability_handle` → `self.durability_handle`, `self.tx_info_store()` stays; `RepoIdentity` = `crate::meta::repo_identity::RepoIdentity`). Keep the doc. (Worker 2 rewrites the shell twins in catalog.rs to delegate here — you only create the core method.)

### impl RepoCore in journal_access.rs
- `checkpoint_allocation_floor(&self) -> DbResult<u64>` — NEW pub(crate) method; body = the body of the free fn `checkpoint_allocation_floor` in repo_instance.rs: `let store = self.repo.store_get("__changelog__").await?; crate::repo::durability_policy::checkpoint_allocation_floor_for(store).await`. Carry over the free fn's doc comment. (Worker 2 turns the free fn in repo_instance.rs into a one-line delegate to this — you only create the core method.)
- `changefeed(&self) -> DbResult<ChangefeedHandle>` — pub(crate); body moves verbatim from lifecycle/journal.rs MINUS the first statement `#[cfg(test)] test_seam::record(self, "before_feed_init");` (that line STAYS in the shell wrapper, which keeps its order relative to everything else). Replace `self.core.durability_handle.*` with `self.durability_handle.*`, `self.core.repo` → `self.repo`, `self.core.journal_publish_hook` → `self.journal_publish_hook`. Keep the full doc.
- `prepare_journal_event(&self, event: Option<shamir_tx::ChangelogEvent>) -> Option<(Arc<shamir_tx::ChangelogEvent>, Option<Arc<shamir_tx::RepoChangefeed>>)>` — pub(crate); body moved verbatim (calls `self.changefeed()`; the log line uses `self.name()` — add the core `name()` call or `&self.name`, whichever reads cleaner, behaviour identical).
- `mark_changefeed_gap(&self, version: u64) -> DbResult<()>` — pub(crate); body moved verbatim.
- `record_pending_changefeed_gap(&self, version: u64)` — pub(crate) one-liner into `self.durability_handle.record_pending_changefeed_gap(version)`.
- `broadcast_changefeed_event(&self, event: Option<Arc<shamir_tx::ChangelogEvent>>)` — pub(crate) one-liner into `self.durability_handle.broadcast_changefeed_event(event)`.
- `follower_journal_store(&self) -> DbResult<Arc<dyn Store>>` — pub(crate) one-liner `self.repo.store_get("__changelog__").await`.
- `changelog_store_for_cold_reconcile(&self) -> DbResult<Arc<dyn Store>>` — pub(crate) one-liner `self.repo.store_get("__changelog__").await`.
- `certify_legacy_cold_start(&self) -> DbResult<()>` — pub(crate); body moved verbatim from lifecycle/recovery/cold_reconcile_repo_impl.rs (its calls `self.tx_gate()`, `self.flush_all_history()`, `self.changefeed()`, `self.wal_group()` are all core methods now). Keep the doc.
- `#[cfg(test)] fire_post_publish_pre_broadcast_test_hook(&self)` — pub(crate); body moved verbatim from journal.rs (`self.core.journal_publish_hook` → `self.journal_publish_hook`).
- `#[cfg(test)] clean_horizon_test_state(&self) -> &std::sync::OnceLock<Arc<std::sync::Mutex<crate::tx::clean_horizon::checkpoint::test_seam::State>>>` — pub(crate) one-liner returning `&self.clean_horizon_test_state` (moved from clean_horizon_repo_impl.rs). Note: importing the `test_seam` module path in the header under `#[cfg(test)]` is required; `test_seam` is a `pub(crate) mod` inside `tx/recovery/clean_horizon/checkpoint.rs`, reachable as `crate::tx::recovery::clean_horizon::checkpoint::test_seam` — check the exact path journal.rs/clean_horizon_repo_impl.rs use today (`crate::tx::clean_horizon::checkpoint::test_seam` via the module re-exports) and reuse exactly that path.

### tx_horizon.rs
Add ONLY `initialized_tx_gate` (pub(crate)) as described above.

### Shell edits (same-name, same-signature, same-visibility one-line delegates)
- lifecycle.rs: `marker_write_mutex`, `write_domain`, `per_table_mvcc`, `tx_metrics`, `tx_info_store`, `repo_interner`?? — NO: repo_interner is worker 2's? NO — repo_interner's shell method lives in lifecycle.rs (it does: `lifecycle.rs` lines 281-294) — so YOU move it: core method `repo_interner(&self) -> DbResult<InternerManager>` goes into tx_access.rs? It is catalog-owned state (`self.catalog.repo_interner` cell + `self.repo.store_get("__interner__")` + `InternerManager::new_labeled(store, self.name().to_string())`). Put it in durability_access.rs? Put it in tx_access.rs — decision: put `repo_interner` in tx_access.rs (it is called 8x by tx; the cell lives on the catalog but the method is repo-wide). Body verbatim with `self.core.catalog.repo_interner` → `self.catalog.repo_interner`, `self.core.repo` → `self.repo`, `self.name()` → `self.name()`. Then: shell `repo_interner` in lifecycle.rs becomes `self.core.repo_interner().await`. Same for `flush_all_history` → `self.core.flush_all_history().await`; `settle_unwritten_version` (in durability_repo_impl.rs) → `self.core.settle_unwritten_version(version).await`; `initialized_mem_wal` → `self.core.initialized_mem_wal()`; `durability_policy` → `self.core.durability_policy()`; `wal_group` → `self.core.wal_group().await`; `wal_retention_gate` / `wal_retention_refuse_writes` / `reserve_recovery_floor` (clean_horizon_repo_impl.rs) → `self.core....`; `has_persistent_wal` / `changelog_store_for_cold_reconcile` (cold_reconcile_repo_impl.rs) → `self.core....`; `certify_legacy_cold_start` → `self.core.certify_legacy_cold_start().await`; `note_unclean_open_bump` (durability_repo_impl.rs) → `self.core.note_unclean_open_bump(permit).await`; `marker_write_mutex` (lifecycle.rs) → `self.core.marker_write_mutex()`; `write_domain` (lifecycle.rs) → `self.core.write_domain()`; `per_table_mvcc` (lifecycle.rs) → `self.core.per_table_mvcc()`; `tx_metrics` (lifecycle.rs) → `self.core.tx_metrics()`; `tx_info_store` (lifecycle.rs) → `self.core.tx_info_store().await`.
- journal.rs: `changefeed` becomes the wrapper shown in the parent brief style: keep `#[cfg(test)] test_seam::record(self, "before_feed_init");` as the first statement, then `self.core.changefeed().await`; keep its doc but shorten the parts that described the init closure (the init doc moved to core). `prepare_journal_event`, `mark_changefeed_gap`, `record_pending_changefeed_gap`, `broadcast_changefeed_event`, `fire_post_publish_pre_broadcast_test_hook` become one-line delegates. `install_journal_for_test`, `journal_feed_for_test`, `stop_journal_writer_for_test`, `read_changelog_*`, `subscribe_changelog`, `clean_horizon`, `certified_gap_interval`, `await_journal_progress` stay UNCHANGED (they call shell methods that still exist).
- durability_repo_impl.rs: the five delegates listed above; everything else (from_factory*, durability_metrics_snapshot, durability_change_requires_reopen, validate_durability_change, set_durability, read_changelog_replication_page, ensure_replication_armed, finish_clean_shutdown) stays UNCHANGED.
- clean_horizon_repo_impl.rs: `wal_retention_gate`, `wal_retention_refuse_writes`, `reserve_recovery_floor` become delegates; `clean_horizon_test_state` becomes `self.core.clean_horizon_test_state()`; the fjall test fns stay.
- cold_reconcile_repo_impl.rs: `changelog_store_for_cold_reconcile`, `has_persistent_wal` become delegates; `certify_legacy_cold_start` becomes a delegate; `certify_cold_start` STAYS UNCHANGED (it delegates to `crate::tx::cold_reconcile::reconcile_cold_start(self, ...)` which still takes `&RepoInstance` — converting it is the sweep stage's job).
- lifecycle.rs: do NOT touch `drainer` (the `OnceLock::get_or_init` closure, ~lines 105-138), `clone_for_background`, `quiesce_commits`, `diagnostics`, `sync_wal`, `synced_flush`, `run_gc`, `spawn_gc_task`, `from_factory*`, `name`, `with_name`, `recover_v2_*`, `fk_reverse_cache`, `arm_sync_wal_fault`, `commit_tail` (already a horizon delegate — leave as-is).

## Hard rules
- No function-local `use` anywhere; hoist every import to the file header of the file you touch. After each edit re-read the whole file and prune imports that became unused and add ones the moved bodies need — unused imports fail clippy `-D warnings`.
- No new `#[allow]`. No visibility widening beyond `pub(crate)` on the new core methods (that widening is declared by the orchestrator). Do not change any signature on the shell.
- `mod.rs` files: only `mod` declarations + `pub(crate) use` re-exports.
- Comments terse; do not touch comments unrelated to the moved bodies; keep the big explanatory docs WITH the bodies (fix `[`x`](Self::x)` links that no longer resolve — use plain backticks or full paths).
- `#[cfg(test)]` / `#[cfg(any(test, feature = "test-util"))]` code is compiled by CI with all features: re-read every cfg-gated block you touch for path/privacy/import correctness.
- Files must stay under 1000 lines; tx/commit/core/ ends at exactly 7 entries.
- NEVER run cargo/clippy/fmt (the orchestrator gates after all workers land). Read-only git (status/diff/log) allowed. NEVER mutate git state.

## Report (final message)
List every method you moved (from-file → core file + visibility), every shell delegate you wrote, every import you added/pruned, the TxHorizon addition, and anything you decided beyond this brief. State explicitly that `drainer`'s get_or_init closure in lifecycle.rs is untouched.
