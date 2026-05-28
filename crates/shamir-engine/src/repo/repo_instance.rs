use super::super::table::{TableConfig, TableManager};
use super::repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{Repo, Store};
use shamir_types::types::common::{new_dash_map_wc, TDashMap};
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::table::table_manager::table_token_for;

/// Manages a single repository and its tables
pub struct RepoInstance {
    name: String,
    repo: BoxRepo,
    configs: Arc<TDashMap<String, TableConfig>>,
    tables: Arc<TDashMap<String, OnceCell<TableManager>>>,
    /// Lazy-initialized RepoTxGate. Created on first call to `tx_gate()`.
    tx_gate: Arc<OnceCell<Arc<shamir_tx::RepoTxGate>>>,
    /// Lazy-initialized RepoWalManager. Created on first call to `repo_wal()`.
    repo_wal: Arc<OnceCell<Arc<shamir_tx::RepoWalManager>>>,
    /// Per-table MvccStore map for SSI version provider. Populated
    /// on demand when `create_table_context` instantiates a
    /// TableManager — both share the same data_store reference.
    /// Key = `table_token_for(name)` (deterministic).
    per_table_mvcc: Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>>>,
}

impl Clone for RepoInstance {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            repo: self.repo.clone(),
            configs: Arc::clone(&self.configs),
            tables: Arc::clone(&self.tables),
            tx_gate: Arc::clone(&self.tx_gate),
            repo_wal: Arc::clone(&self.repo_wal),
            per_table_mvcc: Arc::clone(&self.per_table_mvcc),
        }
    }
}

impl RepoInstance {
    pub fn new(name: String, repo: BoxRepo, configs: Vec<TableConfig>) -> Self {
        Self::from_box_repo(name, repo, configs)
    }

    fn from_box_repo(name: String, repo: BoxRepo, configs: Vec<TableConfig>) -> Self {
        let configs_map: TDashMap<String, TableConfig> = new_dash_map_wc(configs.len().max(16));
        for cfg in configs {
            configs_map.insert(cfg.name.clone(), cfg);
        }

        let tables: TDashMap<String, OnceCell<TableManager>> = new_dash_map_wc(100);

        Self {
            name,
            repo,
            configs: Arc::new(configs_map),
            tables: Arc::new(tables),
            tx_gate: Arc::new(OnceCell::new()),
            repo_wal: Arc::new(OnceCell::new()),
            per_table_mvcc: Arc::new(scc::HashMap::new()),
        }
    }

    /// Creates a RepoInstance asynchronously from a factory.
    /// This is the preferred method as it properly handles blocking I/O.
    pub async fn from_factory(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
    ) -> DbResult<Self> {
        let repo = factory.create().await?;
        Ok(Self::from_box_repo(name, repo, configs))
    }

    /// Repository name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        let cell = self
            .tables
            .entry(table_name.to_string())
            .or_insert_with(OnceCell::new);

        // §B13: existence-check happens INSIDE the init closure, so it
        // is serialized with the actual context construction. Doing the
        // check up-front would race with concurrent `remove_table`
        // between our `configs.contains_key` and the `tables.entry`
        // install (two independent DashMaps). On a removed table the
        // init returns Err and `OnceCell::get_or_try_init` leaves the
        // cell empty so subsequent calls retry.
        cell.get_or_try_init(|| async move {
            if !self.configs.contains_key(table_name) {
                return Err(DbError::NotFound(format!(
                    "Table '{}' is not configured in this repository",
                    table_name
                )));
            }
            self.create_table_context(table_name).await
        })
        .await
        .cloned()
    }

    async fn create_table_context(&self, table_name: &str) -> DbResult<TableManager> {
        let data_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__data__{}", table_name))
            .await?;
        let info_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__info__{}", table_name))
            .await?;
        let history_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__history__{}", table_name))
            .await?;

        let gate = self.tx_gate().await?;
        let mvcc = Arc::new(shamir_tx::MvccStore::new(
            Arc::clone(&data_store),
            history_store,
            gate,
        ));

        let token = table_token_for(table_name);
        let _ = self.per_table_mvcc.insert(token, Arc::clone(&mvcc));

        let tbl = TableManager::create(table_name.to_string(), data_store, info_store).await?;
        Ok(tbl.with_mvcc_store(mvcc))
    }

    pub fn list_table_names(&self) -> Vec<String> {
        self.configs.iter().map(|e| e.key().clone()).collect()
    }

    pub fn has_table(&self, table_name: &str) -> bool {
        self.configs.contains_key(table_name)
    }

    pub fn table_count(&self) -> usize {
        self.configs.len()
    }

    /// Register a new table in the repository.
    /// The table is lazily created on first access via get_table().
    pub fn add_table(&self, config: TableConfig) {
        self.configs.insert(config.name.clone(), config);
    }

    /// Remove a table from the repository.
    /// Returns true if the table existed and was removed.
    pub fn remove_table(&self, table_name: &str) -> bool {
        let removed = self.configs.remove(table_name).is_some();
        if removed {
            self.tables.remove(table_name);
        }
        removed
    }

    /// Returns the per-repo transaction gate, lazily initialising it on
    /// first call.
    ///
    /// The gate is seeded from durable recovery markers stored under the
    /// repo's `"__tx__"` info store:
    /// - `last_committed_version` from `MetaKey::LastCommittedVersion`
    /// - `next_tx_id` from `MetaKey::NextTxId`
    ///
    /// Recovery marker reads are best-effort — if absent we start from
    /// defaults (`RepoTxGate::fresh()`).
    pub async fn tx_gate(&self) -> DbResult<Arc<shamir_tx::RepoTxGate>> {
        self.tx_gate
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;

                let last_committed = crate::meta::recovery_marker::load_last_committed(&info_store)
                    .await
                    .unwrap_or(None)
                    .unwrap_or(0);
                let next_tx_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(1);

                let gate = shamir_tx::RepoTxGate::new(last_committed, next_tx_id);
                Ok::<Arc<shamir_tx::RepoTxGate>, DbError>(Arc::new(gate))
            })
            .await
            .cloned()
    }

    /// Returns the per-repo WAL manager, lazily initialising it on first
    /// call. Shares the `"__tx__"` info store with [`tx_gate`].
    pub async fn repo_wal(&self) -> DbResult<Arc<shamir_tx::RepoWalManager>> {
        self.repo_wal
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;
                let initial_txn_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(1);
                let mgr = shamir_tx::RepoWalManager::new(info_store, initial_txn_id);
                Ok::<Arc<shamir_tx::RepoWalManager>, DbError>(Arc::new(mgr))
            })
            .await
            .cloned()
    }

    /// Open a fresh transaction on this repo.
    ///
    /// Returns a `(TxContext, SnapshotGuard)` pair. The guard's lifetime
    /// must extend at least until commit (or drop = rollback) — drop
    /// removes the snapshot from the active set so GC can reclaim
    /// versions older than `min_alive`.
    ///
    /// `repo_id` in the TxContext is populated via [`repo_token`].
    pub async fn begin_tx(
        &self,
        isolation: shamir_tx::IsolationLevel,
    ) -> DbResult<(shamir_tx::TxContext, shamir_tx::SnapshotGuard)> {
        let gate = self.tx_gate().await?;
        let guard = gate.open_snapshot().await;
        let snapshot_version = guard.version();
        let tx_id = gate.fresh_tx_id();
        let mut tx =
            shamir_tx::TxContext::new(tx_id, repo_token(&self.name), snapshot_version, isolation);

        if isolation == shamir_tx::IsolationLevel::Serializable {
            let provider = std::sync::Arc::new(crate::repo::RepoVersionProvider {
                per_table_mvcc: Arc::clone(&self.per_table_mvcc),
            });
            tx.set_version_provider(provider);
        }

        Ok((tx, guard))
    }

    /// Commit a transaction via the 7-phase commit pipeline.
    ///
    /// Wrapper around [`crate::tx::commit_tx`]. The free function is
    /// the canonical implementation; this method exposes it on the
    /// natural semantic owner.
    pub async fn commit_tx(
        &self,
        tx: shamir_tx::TxContext,
    ) -> Result<crate::tx::TxOutcome, crate::tx::CommitError> {
        crate::tx::commit_tx(tx, self).await
    }

    // ============================================================================
    // Index Management API (proxy to TableManager)
    // ============================================================================

    /// Create a regular index on a table.
    pub async fn create_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_index(index_name, paths).await
    }

    /// Create a unique index on a table.
    pub async fn create_unique_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_unique_index(index_name, paths).await
    }

    /// Drop a regular index from a table.
    pub async fn drop_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_index(index_name).await
    }

    /// Drop a unique index from a table.
    pub async fn drop_unique_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_unique_index(index_name).await
    }

    /// Check if a regular index exists on a table.
    pub async fn index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.index_exists(index_name).await)
    }

    /// Check if a unique index exists on a table.
    pub async fn unique_index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.unique_index_exists(index_name).await)
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<shamir_types::types::record_id::RecordId>> {
        let table = self.get_table(table_name).await?;
        table.lookup_by_index(index_name, values).await
    }

    /// Look up the table whose token matches `token`. Used by V2 WAL
    /// recovery to resolve ops by `table_id_interned`.
    ///
    /// O(N tables) scan — acceptable for recovery hot path which
    /// touches at most one entry per inflight tx.
    pub async fn table_by_token(&self, token: u64) -> DbResult<Option<TableManager>> {
        let names: Vec<String> = self.configs.iter().map(|e| e.key().clone()).collect();
        for name in names {
            if table_token_for(&name) == token {
                let tbl = self.get_table(&name).await?;
                return Ok(Some(tbl));
            }
        }
        Ok(None)
    }

    /// Run V2 WAL recovery: replay any inflight tx entries and remove
    /// their markers. Idempotent — safe to call on every open.
    /// Returns the count of recovered entries.
    pub async fn recover_v2_inflight(&self) -> DbResult<usize> {
        crate::tx::recovery::recover_inflight_v2(self).await
    }
}

/// Deterministic u64 token for a repository name.
///
/// Stage 4: `DefaultHasher(name)` placeholder.
/// Stage 5: real repo-level interner ID.
pub fn repo_token(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}
