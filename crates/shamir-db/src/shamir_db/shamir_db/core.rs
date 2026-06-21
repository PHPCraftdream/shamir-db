use base64::Engine;

use crate::access::Actor;
use crate::engine::db_instance::db_instance::DbInstance;
use crate::{DbError, DbResult};
use dashmap::DashMap;
use shamir_collections::THasher;
use shamir_engine::function::{
    EnvPolicy, FnCtx, FunctionMeta, FunctionRegistry, GlobalVars, NetGateway, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_engine::validator::ValidatorRegistry;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::engine::migration::MigrationCoordinator;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;

use super::super::system_store::{SystemStore, SystemStoreConfig};
use super::SYSTEM_DB_NAME;

/// Input source for [`ShamirDb::create_function_with_opts`].
pub enum FunctionSource<'a> {
    /// Pre-compiled WASM binary bytes.
    Wasm(&'a [u8]),
    /// Rust source code to compile.
    Source(&'a str),
}

/// Top-level manager for multiple database instances.
///
/// Hierarchy:
/// ```text
/// ShamirDb
///   ├── SystemStore (persistent metadata: databases, repos, settings, users, roles)
///   │
///   ├── production (DbInstance)
///   │   └── main (RepoInstance)
///   │       └── users (TableManager)
///   │
///   └── analytics (DbInstance)
///       └── archive (RepoInstance)
///           └── logs (TableManager)
/// ```
#[derive(Clone)]
pub struct ShamirDb {
    pub(super) dbs: Arc<DashMap<String, DbInstance, THasher>>,
    pub(super) system_store: SystemStore,
    /// Serialises admin RMW ops (GrantRole/RevokeRole) per user_name
    /// to close the §B9 read-modify-write race when two concurrent
    /// admin commands target the same user.  Entries leak by design
    /// (each unique user occupies a slot forever), but admin ops are
    /// rare so the memory cost is negligible.
    pub(super) admin_user_locks: Arc<DashMap<String, Arc<Mutex<()>>, THasher>>,
    pub(super) active_migrations: Arc<DashMap<String, Arc<MigrationCoordinator>, THasher>>,
    /// Live function registry (builtins + WASM functions loaded on open).
    pub(super) functions: Arc<FunctionRegistry>,
    /// Shared Wasmtime engine used for all WASM function invocations.
    pub(super) wasm_engine: Arc<WasmEngine>,
    /// Database-global variables shared across all function invocations.
    pub(super) globals: Arc<GlobalVars>,
    /// Network egress allowlist (host patterns). Default empty = deny all.
    pub(super) net_allowlist: Arc<Vec<String>>,
    /// Per-function metadata (visibility, security, secret_grants).
    /// Populated on create/load, updated on rename, removed on drop.
    /// Wrapped in `Arc` so all clones share one map — function metadata
    /// is process-global, exactly like `functions`/`globals`.
    pub(super) function_meta: Arc<DashMap<String, FunctionMeta, THasher>>,
    /// Serialises group-id allocation so concurrent create_group calls
    /// can't read-modify-write the same `next_group_id`. Group creation is
    /// rare, so holding this across the (bounded) await sequence is fine.
    pub(super) group_id_lock: Arc<Mutex<()>>,
    /// Live validator registry (compiled WASM validators loaded on open).
    pub(super) validators: Arc<ValidatorRegistry>,
    /// Base directory for durable repos, derived from the system store
    /// config. `Some(p)` when the system store is redb-backed (production),
    /// `None` for in-memory (tests). Wire-created repos default to a
    /// durable redb engine under this root; in-memory homes fall back to
    /// in-memory repos — coherent with the home's durability class.
    pub(super) data_root: Option<std::path::PathBuf>,
}

impl ShamirDb {
    /// Initialize ShamirDb with a system store.
    ///
    /// # Arguments
    /// * `config` — system store config (InMemory for tests, Redb(path) for production)
    pub async fn init(config: SystemStoreConfig) -> DbResult<Self> {
        Self::init_with_env_policy(config, EnvPolicy::default()).await
    }

    /// Initialize ShamirDb with a system store and an explicit env-seeding policy.
    ///
    /// After constructing the global-vars store, eligible OS environment variables
    /// are seeded into the `env.*` namespace according to `policy`.
    pub async fn init_with_env_policy(
        config: SystemStoreConfig,
        policy: EnvPolicy,
    ) -> DbResult<Self> {
        // Derive data_root BEFORE `config` is moved into SystemStore::init.
        let data_root: Option<std::path::PathBuf> = match &config {
            SystemStoreConfig::InMemory => None,
            SystemStoreConfig::Fjall(p) => p.parent().map(|d| d.to_path_buf()),
        };

        let system_store = SystemStore::init(config).await?;

        let dbs = Arc::new(DashMap::with_hasher(THasher::default()));
        let admin_user_locks = Arc::new(DashMap::with_hasher(THasher::default()));
        let active_migrations = Arc::new(DashMap::with_hasher(THasher::default()));
        let wasm_engine =
            Arc::new(WasmEngine::new().map_err(|e| DbError::Function(e.to_string()))?);
        let functions = Arc::new(FunctionRegistry::with_builtins());
        let globals = Arc::new(GlobalVars::new());
        globals.seed_env(&policy);

        let validators = Arc::new(ValidatorRegistry::new());

        let shamir = Self {
            dbs,
            system_store,
            admin_user_locks,
            active_migrations,
            functions,
            wasm_engine,
            globals,
            net_allowlist: Arc::new(Vec::new()),
            function_meta: Arc::new(DashMap::with_hasher(THasher::default())),
            group_id_lock: Arc::new(Mutex::new(())),
            validators,
            data_root,
        };

        // Load existing databases from system store
        let db_records = shamir.system_store.load_databases().await?;
        for record in &db_records {
            if let Some(name) = record["name"].as_str() {
                if name != SYSTEM_DB_NAME {
                    shamir.dbs.insert(name.to_string(), DbInstance::new());
                }
            }
        }

        // Load existing repositories and register them
        let repo_records = shamir.system_store.load_repositories().await?;
        // Load the per-repo table catalogue once (I.2). Each repo's tables
        // must be re-created BEFORE recovery so V2 crash-recovery's
        // `table_by_token` resolves and Put/Delete/Index ops actually replay
        // for disk-backed repos.
        let table_records = shamir.system_store.load_tables().await?;
        for record in &repo_records {
            let db_name = record["db_name"].as_str().unwrap_or_default();
            let repo_name = record["repo_name"].as_str().unwrap_or_default();
            let engine = record["engine"].as_str().unwrap_or("in_memory");
            let path = record["path"].as_str();

            // Clone the `DbInstance` out of the registry (cheap Arc) so we
            // do NOT hold the DashMap shard guard across the `add_repo` /
            // recovery awaits below.
            if let Some(db) = shamir.get_db(db_name) {
                let factory = Self::factory_from_meta(engine, path);
                if let Some(factory) = factory {
                    // I.2: re-attach the repo WITH its persisted table
                    // catalogue, so the tables exist (and the token→name
                    // reverse index is populated) BEFORE recovery runs.
                    // Previously this passed an empty table list, so a
                    // disk-backed repo's tables didn't exist on restart and
                    // V2 recovery's `table_by_token` resolved nothing —
                    // Put/Delete/Index replay was silently skipped.
                    let mut config = RepoConfig::new(repo_name, factory);
                    for trec in &table_records {
                        if trec["db_name"].as_str() == Some(db_name)
                            && trec["repo_name"].as_str() == Some(repo_name)
                        {
                            if let Some(table_name) = trec["table_name"].as_str() {
                                let mut tcfg = TableConfig::new(table_name);
                                if trec["enable_indexes"].as_bool().unwrap_or(false) {
                                    tcfg = tcfg.with_indexes();
                                }
                                config = config.add_table(tcfg);
                            }
                        }
                    }
                    if let Err(e) = db.add_repo(config).await {
                        log::warn!(
                            "shamir_db::init: failed to attach repo '{}/{}' ({}): {}",
                            db_name,
                            repo_name,
                            engine,
                            e
                        );
                        continue;
                    }

                    // CRIT-A: run V2 WAL crash recovery on the OPEN path,
                    // BEFORE the server accepts requests. A crash between
                    // commit Phase 4 (`wal.begin`) and Phase 7
                    // (`wal.commit`) leaves a durable inflight `WalEntryV2`;
                    // without this replay the committed tx data is silently
                    // lost on restart. A recovery failure is propagated
                    // (not swallowed) — a repo that cannot recover must not
                    // serve.
                    if let Some(repo) = db.get_repo(repo_name) {
                        let recovered = repo.recover_v2_inflight().await?;
                        if recovered > 0 {
                            log::info!(
                                "recovered {} inflight transactions for repo '{}/{}'",
                                recovered,
                                db_name,
                                repo_name
                            );
                        }
                    }
                }
            }
        }

        // Load persisted WASM functions from the function catalogue and
        // register them. This proves a runner-without-cargo can still run
        // functions — no toolchain needed, just `from_binary`.
        let fn_records = shamir.system_store.load_functions().await?;
        for rec in &fn_records {
            let name = match rec["name"].as_str() {
                Some(n) => n.to_string(),
                None => continue,
            };
            let wasm_b64 = match rec["wasm_b64"].as_str() {
                Some(b) => b,
                None => {
                    log::warn!(
                        "shamir_db::init: skipping function '{}' — no wasm_b64 field",
                        name
                    );
                    continue;
                }
            };
            let wasm_bytes = match base64::engine::general_purpose::STANDARD.decode(wasm_b64) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!(
                        "shamir_db::init: skipping function '{}' — base64 decode error: {}",
                        name,
                        e
                    );
                    continue;
                }
            };
            match WasmFunction::from_binary(
                shamir.wasm_engine.clone(),
                &wasm_bytes,
                WasmLimits::default(),
            ) {
                Ok(wf) => {
                    if shamir.functions.register(&name, Arc::new(wf)).is_err() {
                        log::warn!(
                            "shamir_db::init: function '{}' already registered (builtin?), skipping catalogue load",
                            name
                        );
                    }
                    // Populate in-memory metadata from the persisted record.
                    let meta = FunctionMeta::from_record(rec);
                    let _ = shamir.function_meta.insert(name.clone(), meta);
                }
                Err(e) => {
                    log::warn!(
                        "shamir_db::init: failed to compile WASM for function '{}': {}",
                        name,
                        e
                    );
                }
            }
        }

        // Load persisted WASM validators from the validator catalogue and
        // register them (S1). Mirrors the function loading block above.
        let val_records = shamir.system_store.load_validators().await?;
        for rec in &val_records {
            let name = match rec["name"].as_str() {
                Some(n) => n.to_string(),
                None => continue,
            };
            let wasm_b64 = match rec["wasm_b64"].as_str() {
                Some(b) => b,
                None => {
                    log::warn!(
                        "shamir_db::init: skipping validator '{}' — no wasm_b64 field",
                        name
                    );
                    continue;
                }
            };
            let wasm_bytes = match base64::engine::general_purpose::STANDARD.decode(wasm_b64) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!(
                        "shamir_db::init: skipping validator '{}' — base64 decode error: {}",
                        name,
                        e
                    );
                    continue;
                }
            };
            // Reconstruct the RecordId from the persisted `_id` field.
            let id = match rec.get("_id").and_then(|v| v.as_str()) {
                Some(id_str) => match id_str.parse::<RecordId>() {
                    Ok(rid) => rid,
                    Err(e) => {
                        log::warn!(
                            "shamir_db::init: skipping validator '{}' — bad _id: {}",
                            name,
                            e
                        );
                        continue;
                    }
                },
                None => {
                    log::warn!(
                        "shamir_db::init: skipping validator '{}' — no _id field",
                        name
                    );
                    continue;
                }
            };
            match WasmFunction::from_binary(
                shamir.wasm_engine.clone(),
                &wasm_bytes,
                WasmLimits::default(),
            ) {
                Ok(wf) => {
                    if shamir.validators.register(id, &name, Arc::new(wf)).is_err() {
                        log::warn!(
                            "shamir_db::init: validator '{}' already registered, skipping",
                            name
                        );
                        continue;
                    }
                    // Restore bound_in from the persisted record.
                    if let Some(bound) = rec.get("bound_in").and_then(|v| v.as_array()) {
                        for entry in bound {
                            if let Some(table_ref) = entry.as_str() {
                                shamir.validators.add_binding(&id, table_ref);
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "shamir_db::init: failed to compile WASM for validator '{}': {}",
                        name,
                        e
                    );
                }
            }
        }

        Ok(shamir)
    }

    /// Initialize with in-memory system store (convenience for tests).
    pub async fn init_memory() -> DbResult<Self> {
        Self::init(SystemStoreConfig::InMemory).await
    }

    /// Get the system store.
    pub fn system_store(&self) -> &SystemStore {
        &self.system_store
    }

    /// Per-user lock map used to serialise admin RMW ops (GrantRole /
    /// RevokeRole) and close the §B9 read-modify-write race.
    pub fn admin_user_locks(&self) -> &Arc<DashMap<String, Arc<Mutex<()>>, THasher>> {
        &self.admin_user_locks
    }

    pub fn active_migrations(&self) -> &Arc<DashMap<String, Arc<MigrationCoordinator>, THasher>> {
        &self.active_migrations
    }

    /// Base directory for durable repos. `Some` when the system store
    /// is redb-backed (production), `None` for in-memory (tests).
    pub fn data_root(&self) -> Option<&std::path::Path> {
        self.data_root.as_deref()
    }

    pub fn db_count(&self) -> usize {
        self.dbs.len()
    }

    pub fn has_db(&self, name: &str) -> bool {
        self.dbs.contains_key(name)
    }

    pub fn list_dbs(&self) -> Vec<String> {
        self.dbs.iter().map(|r| r.key().clone()).collect()
    }

    pub fn get_db(&self, name: &str) -> Option<DbInstance> {
        self.dbs.get(name).map(|r| r.clone())
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    pub(super) fn factory_from_meta(engine: &str, path: Option<&str>) -> Option<BoxRepoFactory> {
        // Each backend is gated by its cargo feature; an unknown engine
        // string OR a backend that wasn't built into this binary returns
        // `None`. The system_store's recorded engine name doesn't
        // disappear when the feature is off — we just refuse to
        // re-attach the repo.
        match engine {
            "in_memory" => Some(BoxRepoFactory::in_memory()),
            #[cfg(feature = "sled")]
            "sled" => path.map(BoxRepoFactory::sled),
            #[cfg(feature = "fjall")]
            "fjall" => path.map(BoxRepoFactory::fjall),
            _ => None,
        }
    }

    pub(super) fn extract_storage_type(factory: &BoxRepoFactory) -> String {
        match factory {
            BoxRepoFactory::InMemory(_) => "in_memory",
            #[cfg(feature = "sled")]
            BoxRepoFactory::Sled(_) => "sled",
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(_) => "fjall",
            // The buffer layer doesn't have an identity of its own
            // — recurse to the underlying backend so reflection
            // sees the real engine.
            BoxRepoFactory::MemBuffer(f) => return Self::extract_storage_type(&f.inner),
            BoxRepoFactory::Cached(f) => return Self::extract_storage_type(&f.inner),
        }
        .to_string()
    }

    pub(super) fn extract_path(factory: &BoxRepoFactory) -> Option<String> {
        match factory {
            BoxRepoFactory::InMemory(_) => None,
            #[cfg(feature = "sled")]
            BoxRepoFactory::Sled(f) => Some(f.path.to_string_lossy().to_string()),
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::MemBuffer(f) => Self::extract_path(&f.inner),
            BoxRepoFactory::Cached(f) => Self::extract_path(&f.inner),
        }
    }

    /// Canonical table-ref string for validator binding tracking.
    /// Format: `"db/repo/table"` — matches the form used by
    /// `ValidatorRegistry::add_binding` / `bound_tables`.
    pub(super) fn table_ref_str(db: &str, repo: &str, table: &str) -> String {
        format!("{}/{}/{}", db, repo, table)
    }

    /// Build an [`FnCtx`] with globals, registry, net gateway, the
    /// function's secret_grants from [`function_meta`], and the given
    /// [`Actor`] (R2).
    pub(super) fn build_invoke_ctx(&self, fn_name: &str, actor: Actor) -> FnCtx {
        let grants = self
            .function_meta(fn_name)
            .map(|m| m.secret_grants)
            .unwrap_or_default();
        FnCtx::with_globals(self.globals.clone())
            .with_registry(self.functions.clone())
            .with_net(self.build_net_gateway())
            .with_secret_grants(grants)
            .with_actor(actor)
    }

    /// Build a [`NetGateway`] from the current allowlist.
    ///
    /// Always returns a gateway so that allowlist-denial is a catchable
    /// runtime error, not a "no net gateway" trap.
    pub(super) fn build_net_gateway(&self) -> Arc<dyn NetGateway> {
        Arc::new(super::super::curl_gateway::CurlNetGateway::new(
            self.net_allowlist.to_vec(),
        ))
    }
}
