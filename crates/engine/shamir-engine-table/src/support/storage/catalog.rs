//! Plain, Arc-shared catalogue of one repository's tables. Cloned shallowly
//! with the repo handle; `RepoInstance` (instance layer) constructs it and
//! keeps delegating wrappers.

use crate::interner_manager::InternerManager;
use crate::{table_store_names, TableConfig, TableManager, TableRegistration};
use shamir_storage::error::DbResult;
use shamir_types::types::common::{TDashMap, THasher};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::OnceCell;

#[derive(Clone)]
#[doc(hidden)]
pub struct TableCatalog {
    configs: Arc<TDashMap<String, TableConfig>>,
    tables: Arc<TDashMap<String, Arc<OnceCell<TableManager>>>>,
    /// Reverse index `table_token_for(name) → name`, maintained at table
    /// *registration* time (`from_box_repo` + `add_table`), independent of
    /// whether the table has been instantiated yet. Lets
    /// `RepoInstance::table_by_token` resolve a token in O(1)
    /// instead of scanning every config and re-hashing its name under the
    /// per-repo `commit_lock` (III.1). The token is a pure function of the
    /// name, so this is just a pre-computed inverse of that function.
    token_names: Arc<scc::HashMap<u64, String, THasher>>,
    /// Per-table MvccStore map for SSI version provider. Populated
    /// on demand when `create_table_context` instantiates a
    /// TableManager — both share the same data_store reference.
    /// Key = `table_token_for(name)` (deterministic).
    per_table_mvcc: Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>>,
    /// Lazy-initialized per-repo string interner. Backed by the dedicated
    /// `"__interner__"` store. Shared via [`Arc::clone`] so every
    /// [`TableManager`](crate::TableManager) in this repo shares the
    /// same live [`Interner`](shamir_types::core::interner::Interner) and
    /// id-namespace.
    repo_interner: Arc<OnceCell<InternerManager>>,
}

impl TableCatalog {
    pub fn new(
        configs: TDashMap<String, TableConfig>,
        tables: TDashMap<String, Arc<OnceCell<TableManager>>>,
        token_names: scc::HashMap<u64, String, THasher>,
        per_table_mvcc: scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>,
        repo_interner: OnceCell<InternerManager>,
    ) -> Self {
        Self {
            configs: Arc::new(configs),
            tables: Arc::new(tables),
            token_names: Arc::new(token_names),
            per_table_mvcc: Arc::new(per_table_mvcc),
            repo_interner: Arc::new(repo_interner),
        }
    }

    pub fn register_token_in(token_names: &scc::HashMap<u64, String, THasher>, name: &str) {
        let token = crate::table_manager::table_token_for(name);
        if let Err((_, attempted)) = token_names.insert_sync(token, name.to_string()) {
            let existing = token_names.read_sync(&token, |_, name| name.clone());
            if existing.as_deref() != Some(attempted.as_str()) {
                log::warn!(
                    "repo_instance: table token collision on {} — keeping '{}', \
                     refusing '{}'; the latter is unresolvable by token",
                    token,
                    existing.as_deref().unwrap_or("<unknown>"),
                    attempted
                );
            }
        }
    }

    pub fn register_table_with(&self, config: TableConfig, invalidate: impl FnOnce()) {
        let _entry = self.tables.entry(config.name.clone());
        Self::register_token_in(&self.token_names, &config.name);
        self.configs.insert(config.name.clone(), config);
        invalidate();
    }

    pub fn remove_table_with(&self, name: &str, invalidate: impl FnOnce()) -> bool {
        let entry = self.tables.entry(name.to_string());
        let removed = self.configs.remove(name).is_some();
        if removed {
            invalidate();
            let token = crate::table_manager::table_token_for(name);
            let _ = self
                .token_names
                .remove_if_sync(&token, |existing| existing.as_str() == name);
            if let dashmap::mapref::entry::Entry::Occupied(entry) = &entry {
                if let Some(mvcc) = entry.get().get().and_then(TableManager::mvcc_store) {
                    self.remove_mvcc_if_same(token, &mvcc);
                }
            }
            if let dashmap::mapref::entry::Entry::Occupied(entry) = entry {
                entry.remove();
            }
        }
        removed
    }

    pub fn token_name_for(&self, token: u64) -> Option<String> {
        self.token_names.read_sync(&token, |_, name| name.clone())
    }

    pub fn with_current_cell<R>(
        &self,
        name: &str,
        cell: &Arc<OnceCell<TableManager>>,
        action: impl FnOnce(bool) -> R,
    ) -> R {
        let configured = self.configs.contains_key(name);
        let current = self.tables.get(name);
        let is_current = configured
            && current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), cell));
        let result = action(is_current);
        drop(current);
        result
    }

    pub fn table_config_matches_store_id(
        &self,
        name: &str,
        store_id: &Option<shamir_types::types::record_id::RecordId>,
    ) -> bool {
        self.configs
            .get(name)
            .is_some_and(|current| current.value().store_id == *store_id)
    }

    pub fn insert_table_cell(&self, name: &str) -> Arc<OnceCell<TableManager>> {
        Arc::clone(
            self.tables
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .value(),
        )
    }

    fn remove_mvcc_if_same(&self, token: u64, mvcc: &Arc<shamir_tx::MvccStore>) -> bool {
        self.per_table_mvcc
            .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, mvcc))
            .is_some()
    }

    pub fn remove_mvcc_for_table_if_same(
        &self,
        name: &str,
        mvcc: &Arc<shamir_tx::MvccStore>,
    ) -> bool {
        self.remove_mvcc_if_same(crate::table_manager::table_token_for(name), mvcc)
    }

    fn mvcc_store(&self, token: u64) -> Option<Arc<shamir_tx::MvccStore>> {
        self.per_table_mvcc
            .read_sync(&token, |_, mvcc| Arc::clone(mvcc))
    }

    pub fn mvcc_for_table(&self, name: &str) -> Option<Arc<shamir_tx::MvccStore>> {
        self.mvcc_store(crate::table_manager::table_token_for(name))
    }

    fn insert_mvcc(
        &self,
        token: u64,
        mvcc: Arc<shamir_tx::MvccStore>,
    ) -> Result<(), (u64, Arc<shamir_tx::MvccStore>)> {
        self.per_table_mvcc.insert_sync(token, mvcc)
    }

    pub fn attach_mvcc(
        &self,
        name: &str,
        mvcc: Arc<shamir_tx::MvccStore>,
    ) -> Result<(), (u64, Arc<shamir_tx::MvccStore>)> {
        self.insert_mvcc(crate::table_manager::table_token_for(name), mvcc)
    }
    pub fn mvcc_snapshot(&self) -> Vec<Arc<shamir_tx::MvccStore>> {
        let mut mvccs = Vec::new();
        self.per_table_mvcc.iter_sync(|_, mvcc| {
            mvccs.push(Arc::clone(mvcc));
            true
        });
        mvccs
    }

    pub fn mvcc_map_for_commit_pipeline(
        &self,
    ) -> &Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>> {
        &self.per_table_mvcc
    }

    pub async fn repo_interner_or_try_init<F, Fut>(&self, init: F) -> DbResult<InternerManager>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = DbResult<InternerManager>>,
    {
        self.repo_interner.get_or_try_init(init).await.cloned()
    }

    pub fn set_repo_interner_if_uninitialized(&self, manager: InternerManager) -> bool {
        self.repo_interner.set(manager).is_ok()
    }

    /// Return whether a table is configured.
    pub fn has_table(&self, name: &str) -> bool {
        self.configs.contains_key(name)
    }

    /// List configured table names.
    pub fn list_table_names(&self) -> Vec<String> {
        self.configs
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    pub fn table_config(&self, name: &str) -> Option<TableConfig> {
        self.configs.get(name).map(|entry| entry.value().clone())
    }

    /// Live registration identity of a configured table: the catalogue cell
    /// the registration reserved for `name`. Never instantiates the table. An
    /// unconfigured name has no registration (`None`), so a caller fails closed
    /// instead of inventing one; for a configured name the cell is reserved on
    /// demand (the same get-or-insert `insert_table_cell` performs), which is
    /// what makes an identity exist for every registration path, including
    /// boot-loaded legacy tables.
    pub fn table_registration(&self, name: &str) -> Option<TableRegistration> {
        if !self.configs.contains_key(name) {
            return None;
        }
        Some(TableRegistration::of(self.insert_table_cell(name)))
    }

    pub fn config_pair_matches(
        &self,
        from: &str,
        to: &str,
        expected_to: &TableConfig,
    ) -> (Option<TableConfig>, Option<TableConfig>, bool) {
        let source = self.table_config(from);
        let destination = self.table_config(to);
        let destination_matches = destination.as_ref().is_none_or(|destination| {
            destination.store_id == expected_to.store_id
                && destination.enable_indexes == expected_to.enable_indexes
        });
        (source, destination, destination_matches)
    }

    /// Return mapped data, info and history store names for a table.
    pub fn mapped_table_store_names(&self, name: &str) -> Option<(String, String, String)> {
        let config = self.configs.get(name).map(|entry| entry.value().clone())?;
        let names = table_store_names(name, config.store_id.as_ref());
        Some((names.data, names.info, names.history))
    }

    /// Count configured tables.
    pub fn table_count(&self) -> usize {
        self.configs.len()
    }

    /// Test-only: direct cell-map size (cold-resolver retention probe).
    #[cfg(feature = "test-util")]
    pub fn table_cell_count(&self) -> usize {
        self.tables.len()
    }

    /// Non-instantiating peek: like
    /// `RepoInstance::table_by_token`,
    /// but returns `None` (rather than lazily creating and returning) for a
    /// table that is registered (`add_table` was called) but has never been
    /// touched via
    /// `RepoInstance::get_table` — the
    /// `OnceCell` is still dormant.
    ///
    /// #538 Part A: `pre_commit_prelock`'s Phase 2.5 uses this instead of
    /// `table_by_token` to check `needs_write_barrier()` for every table in
    /// `tx.write_set`, specifically so that check-only lookup does NOT force
    /// lazy table instantiation as a side effect. This matters because
    /// instantiating a `TableManager` registers it in `per_table_mvcc`,
    /// which flips `apply_data_batch`'s routing for that table's data writes
    /// from the direct-store fallback to MVCC-routed — a behavior change
    /// nothing in `pre_commit_prelock` should trigger merely by CHECKING a
    /// barrier flag. Correctness is preserved: `needs_write_barrier()` can
    /// only ever be `true` on an instance that was actually created — no
    /// code path can flip `index2_create_barrier` or register a base_index
    /// unique index without first holding a live `TableManager` (`get_table`
    /// must have run at least once for a `create_index_v2` /
    /// `create_unique_index` to even be callable) — so a dormant `OnceCell`
    /// can only ever mean "this table has no in-flight barrier", making
    /// `None` here exactly equivalent to "table needs no barrier" for this
    /// check's purposes.
    ///
    /// Resolve a token only when its manager is already initialized.
    ///
    /// This is a non-instantiating peek: a dormant manager stays dormant, so keep
    /// the lookup non-instantiating if it is used by background/drain paths.
    pub async fn table_by_token_if_live(&self, token: u64) -> Option<TableManager> {
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async`. The
        // `token_names` map is also touched SYNCHRONOUSLY by `register_token`'s
        // `insert_sync` + `read_sync` (DDL — table create/rename) and by
        // drop-table's `remove_if_sync` (DDL). `read_async`'s wait is
        // lock-HANDOFF: saa grants the shared bucket lock to the suspended
        // reader TASK, which then holds it while unpolled in tokio's run queue;
        // a DDL exclusive writer (create/rename/drop) racing sustained
        // commit-pipeline / V2-WAL-recovery `read_async` traffic can park every
        // worker behind that unpolled reader → whole-runtime deadlock.
        // `read_sync`'s bucket lock is held only by a RUNNING thread for a few
        // instructions (a `String` clone), bounding every wait.
        //
        // NON-INSTANTIATING PROPERTY PRESERVED: this change is purely the
        // lock-acquisition mechanism. The lookup still reads the dormant
        // `tables` OnceCell map DIRECTLY (`self.tables.get(&name)`) and does
        // NOT call the lazily-instantiating `get_table` — so the documented
        // reason this fn exists (Phase 2.5's barrier check must NOT force table
        // instantiation as a side effect) is unaffected.
        let name = self.token_names.read_sync(&token, |_, name| name.clone())?;
        let cell = self
            .tables
            .get(&name)
            .map(|entry| Arc::clone(entry.value()))?;
        cell.get().cloned()
    }

    /// Returns an already initialized table without admitting or triggering
    /// initialization. Callers must not retain a map guard across awaits.
    pub fn initialized_table(&self, table_name: &str) -> Option<TableManager> {
        let cell = self.tables.get(table_name)?;
        cell.get().cloned()
    }

    /// #984 — count degraded (non-`Ready`) indexes across all ALREADY-OPEN
    /// tables in this repo. Pure in-memory — zero store reads. Does NOT
    /// force-open closed tables (that would turn a cheap `/metrics` gauge
    /// into an expensive scan and change server behaviour as a side-effect
    /// of scraping). See [`TableManager::degraded_index_count`] for the
    /// per-table cost model.
    pub async fn degraded_index_count(&self) -> u64 {
        // Collect clones first to avoid holding DashMap read guards across
        // the `.await` points inside the loop.
        let tables: Vec<TableManager> = self
            .tables
            .iter()
            .filter_map(|entry| entry.value().get().cloned())
            .collect();
        let mut count = 0;
        for table in tables {
            count += table.degraded_index_count().await;
        }
        count
    }

    /// Test-only fault-injection seam: force-overwrite the cached
    /// `TableManager` for `table_name` with a caller-supplied instance
    /// (e.g. one built via [`TableManager::create`] on a fault-injecting
    /// `Store` double), so subsequent
    /// `RepoInstance::get_table` calls
    /// on THIS repo instance (and any clone sharing the same `tables` map)
    /// observe it. Used by `shamir-db`'s resource_meta fail-closed
    /// regression test (audit #540) to make a real catalogue read return
    /// `Err` without a large `BoxRepo`/`BoxRepoFactory` fault-injection
    /// rewrite.
    ///
    /// Gated by the `test-util` feature (NOT plain `#[cfg(test)]`) because
    /// the caller is a downstream crate's test (`shamir-db`) — a
    /// `#[cfg(test)]` item is private to the crate being compiled in test
    /// mode and would not exist when `shamir-engine` is pulled in as a
    /// normal (non-test) dependency. Never compiled into a production
    /// build: no default feature enables `test-util`.
    ///
    /// **Caveat for reuse**: this only overwrites the `tables` cache, NOT
    /// `configs`. Safe when `table_name` is already registered in `configs`
    /// (true for every existing caller — e.g. `SystemStore::init` registers
    /// `"databases"` before this is ever called). Calling this with a
    /// table name that is NOT already in `configs` would desync
    /// `has_table`/`list_table_names`/`token_names` from `tables` — if a
    /// future caller needs that, register the table via the normal config
    /// path first.
    ///
    /// Install a prebuilt table manager for test fixtures.
    #[cfg(feature = "test-util")]
    pub fn install_table_for_test(&self, name: &str, table: TableManager) {
        self.tables
            .insert(name.into(), Arc::new(OnceCell::new_with(Some(table))));
    }
}
