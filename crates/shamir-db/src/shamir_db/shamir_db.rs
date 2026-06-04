use base64::Engine;
use serde_json::json;

use crate::access::{
    authorize, permits, principal_id, AccessError, Action, Actor, Mode, ResourceMeta, ResourcePath,
    OWNER_SYSTEM,
};
use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::function::DbGateway;
use crate::engine::query::batch::{BatchError, BatchOp, BatchRequest, QueryEntry};
use crate::engine::query::read::ReadQuery;
use crate::engine::query::write::InsertOp;
use crate::engine::query::TableRef;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::{TableConfig, TableManager};
use crate::types::common::new_map;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult};
use async_trait::async_trait;
use dashmap::DashMap;
use shamir_engine::function::{
    compile_rust_source, BatchContext, CreateFunctionOptions, EnvPolicy, FnBatch, FnCtx,
    FunctionError, FunctionMeta, FunctionRegistry, GlobalVars, NetGateway, Params, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_engine::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::engine::migration::MigrationCoordinator;

use super::system_store::{SystemStore, SystemStoreConfig};

const SYSTEM_DB_NAME: &str = "__system__";

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
    dbs: Arc<DashMap<String, DbInstance>>,
    system_store: SystemStore,
    /// Serialises admin RMW ops (GrantRole/RevokeRole) per user_name
    /// to close the §B9 read-modify-write race when two concurrent
    /// admin commands target the same user.  Entries leak by design
    /// (each unique user occupies a slot forever), but admin ops are
    /// rare so the memory cost is negligible.
    admin_user_locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
    active_migrations: Arc<DashMap<String, Arc<MigrationCoordinator>>>,
    /// Live function registry (builtins + WASM functions loaded on open).
    functions: Arc<FunctionRegistry>,
    /// Shared Wasmtime engine used for all WASM function invocations.
    wasm_engine: Arc<WasmEngine>,
    /// Database-global variables shared across all function invocations.
    globals: Arc<GlobalVars>,
    /// Network egress allowlist (host patterns). Default empty = deny all.
    net_allowlist: Arc<Vec<String>>,
    /// Per-function metadata (visibility, security, secret_grants).
    /// Populated on create/load, updated on rename, removed on drop.
    /// Wrapped in `Arc` so all clones share one map — function metadata
    /// is process-global, exactly like `functions`/`globals`.
    function_meta: Arc<DashMap<String, FunctionMeta>>,
    /// Serialises group-id allocation so concurrent create_group calls
    /// can't read-modify-write the same `next_group_id`. Group creation is
    /// rare, so holding this across the (bounded) await sequence is fine.
    group_id_lock: Arc<Mutex<()>>,
    /// Live validator registry (compiled WASM validators loaded on open).
    validators: Arc<ValidatorRegistry>,
    /// Base directory for durable repos, derived from the system store
    /// config. `Some(p)` when the system store is redb-backed (production),
    /// `None` for in-memory (tests). Wire-created repos default to a
    /// durable redb engine under this root; in-memory homes fall back to
    /// in-memory repos — coherent with the home's durability class.
    data_root: Option<std::path::PathBuf>,
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
            SystemStoreConfig::Redb(p) => p.parent().map(|d| d.to_path_buf()),
        };

        let system_store = SystemStore::init(config).await?;

        let dbs = Arc::new(DashMap::new());
        let admin_user_locks = Arc::new(DashMap::new());
        let active_migrations = Arc::new(DashMap::new());
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
            function_meta: Arc::new(DashMap::new()),
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
    pub fn admin_user_locks(&self) -> &Arc<DashMap<String, Arc<Mutex<()>>>> {
        &self.admin_user_locks
    }

    pub fn active_migrations(&self) -> &Arc<DashMap<String, Arc<MigrationCoordinator>>> {
        &self.active_migrations
    }

    /// Base directory for durable repos. `Some` when the system store
    /// is redb-backed (production), `None` for in-memory (tests).
    pub fn data_root(&self) -> Option<&std::path::Path> {
        self.data_root.as_deref()
    }

    fn factory_from_meta(engine: &str, path: Option<&str>) -> Option<BoxRepoFactory> {
        // Each backend is gated by its cargo feature; an unknown engine
        // string OR a backend that wasn't built into this binary returns
        // `None`. The system_store's recorded engine name doesn't
        // disappear when the feature is off — we just refuse to
        // re-attach the repo.
        match engine {
            "in_memory" => Some(BoxRepoFactory::in_memory()),
            #[cfg(feature = "redb")]
            "redb" => path.map(BoxRepoFactory::redb),
            #[cfg(feature = "sled")]
            "sled" => path.map(BoxRepoFactory::sled),
            #[cfg(feature = "fjall")]
            "fjall" => path.map(BoxRepoFactory::fjall),
            #[cfg(feature = "nebari")]
            "nebari" => path.map(BoxRepoFactory::nebari),
            #[cfg(feature = "persy")]
            "persy" => path.map(BoxRepoFactory::persy),
            #[cfg(feature = "canopy")]
            "canopy" => path.map(BoxRepoFactory::canopy),
            _ => None,
        }
    }

    pub fn db_count(&self) -> usize {
        self.dbs.len()
    }

    pub fn has_db(&self, name: &str) -> bool {
        self.dbs.contains_key(name)
    }

    pub async fn create_db(&self, name: &str) -> DbInstance {
        self.create_db_as(name, Actor::System).await
    }

    /// Like [`create_db`] but stamps the new database's owner as `actor`
    /// instead of `System`. Mode stays `0o777` (open).
    pub async fn create_db_as(&self, name: &str, actor: Actor) -> DbInstance {
        let db = DbInstance::new();
        self.dbs.insert(name.to_string(), db.clone());

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Persist to system store
        if let Err(e) = self
            .system_store
            .save_database(
                name,
                &json!({
                    "name": name,
                    "created_at": created_at,
                }),
                &ResourceMeta::owned_by(actor),
            )
            .await
        {
            log::warn!("shamir_db::create_db: failed to persist '{}': {}", name, e);
        }

        db
    }

    pub fn get_db(&self, name: &str) -> Option<DbInstance> {
        self.dbs.get(name).map(|r| r.clone())
    }

    pub fn list_dbs(&self) -> Vec<String> {
        self.dbs.iter().map(|r| r.key().clone()).collect()
    }

    pub async fn remove_db(&self, name: &str) -> bool {
        if name == SYSTEM_DB_NAME {
            return false;
        }

        let removed = self.dbs.remove(name).is_some();

        if removed {
            if let Err(e) = self.system_store.remove_database(name).await {
                log::warn!(
                    "shamir_db::remove_db: failed to remove '{}' from system store: {}",
                    name,
                    e
                );
            }
        }

        removed
    }

    pub async fn add_repo(&self, db_name: &str, config: RepoConfig) -> DbResult<()> {
        self.add_repo_as(db_name, config, Actor::System).await
    }

    /// Like [`add_repo`] but stamps the repo (and its inline tables) with
    /// the given actor as owner.
    pub async fn add_repo_as(
        &self,
        db_name: &str,
        config: RepoConfig,
        actor: Actor,
    ) -> DbResult<()> {
        // Owned clone (cheap Arc) — never hold the DashMap shard guard
        // across the `add_repo` / recovery awaits below.
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        let repo_name = config.name.clone();
        let storage_type = Self::extract_storage_type(&config.factory);
        let path = Self::extract_path(&config.factory);
        // Capture the inline table list before `config` is moved into
        // `db.add_repo`, so the per-repo table catalogue can be persisted
        // alongside the repo record (I.2).
        let inline_tables: Vec<(String, bool)> = config
            .tables
            .iter()
            .map(|t| (t.name.clone(), t.enable_indexes))
            .collect();

        db.add_repo(config).await?;

        // CRIT-A: run V2 WAL crash recovery before the repo is reachable
        // by callers. For a freshly created repo `list_inflight` is empty
        // so this is a cheap no-op; for a *re-attached* on-disk repo it
        // replays any inflight tx left by a prior crash. Recovery failure
        // is propagated — a repo that cannot recover must not be served.
        if let Some(repo) = db.get_repo(&repo_name) {
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

        let meta = ResourceMeta::owned_by(actor.clone());

        // Persist to system store
        if let Err(e) = self
            .system_store
            .save_repository(db_name, &repo_name, &storage_type, path.as_deref(), &meta)
            .await
        {
            log::warn!(
                "shamir_db::add_repo: failed to persist '{}/{}': {}",
                db_name,
                repo_name,
                e
            );
        }

        // Persist the inline table catalogue so these tables are re-created
        // on the next open (I.2). Best-effort per table, matching the
        // repo-record persistence above.
        for (table_name, enable_indexes) in &inline_tables {
            if let Err(e) = self
                .system_store
                .save_table(db_name, &repo_name, table_name, *enable_indexes, &meta)
                .await
            {
                log::warn!(
                    "shamir_db::add_repo: failed to persist table catalogue '{}/{}/{}': {}",
                    db_name,
                    repo_name,
                    table_name,
                    e
                );
            }
        }

        Ok(())
    }

    /// Drain every repo's in-memory MemBuffers to their durable backing.
    ///
    /// Called on graceful shutdown to close the ~500 ms buffered-commit
    /// loss window. For each repo the tx-info store and every table's
    /// data + info stores are flushed. In-memory stores are no-ops.
    /// Best-effort: individual errors are logged and skipped; returns the
    /// first error encountered (if any) after attempting all repos/tables.
    pub async fn flush_all(&self) -> DbResult<()> {
        let mut first_err: Option<DbError> = None;
        let db_names = self.list_dbs();
        for db_name in &db_names {
            let Some(db) = self.get_db(db_name) else {
                continue;
            };
            for repo_name in db.list_repos() {
                let Some(repo) = db.get_repo(&repo_name) else {
                    continue;
                };

                if let Err(e) = repo.flush_buffers().await {
                    log::warn!("flush_all: {}/{}: {}", db_name, repo_name, e);
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Create a table in a repo and persist it to the table catalogue so it
    /// survives a restart (I.2).
    ///
    /// Delegates to [`DbInstance::create_table`] (the same path that lazily
    /// instantiates the `TableManager` on first access) and then records the
    /// table in the system store. Persistence is best-effort: a failed
    /// catalogue write is logged, not propagated, mirroring `add_repo`.
    pub async fn add_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
    ) -> DbResult<()> {
        self.add_table_as(
            db_name,
            repo_name,
            table_name,
            enable_indexes,
            Actor::System,
        )
        .await
    }

    /// Like [`add_table`] but stamps the new table with the given actor as
    /// owner.
    pub async fn add_table_as(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
        actor: Actor,
    ) -> DbResult<()> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        let mut config = TableConfig::new(table_name);
        if enable_indexes {
            config = config.with_indexes();
        }
        db.get_repo(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?
            .add_table(config);

        if let Err(e) = self
            .system_store
            .save_table(
                db_name,
                repo_name,
                table_name,
                enable_indexes,
                &ResourceMeta::owned_by(actor),
            )
            .await
        {
            log::warn!(
                "shamir_db::add_table: failed to persist table catalogue '{}/{}/{}': {}",
                db_name,
                repo_name,
                table_name,
                e
            );
        }

        Ok(())
    }

    /// Drop a table from a repo and remove it from the table catalogue.
    /// Returns whether the table existed in the running instance.
    pub async fn drop_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<bool> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let removed = db.drop_table(repo_name, table_name)?;

        // Always clear the catalogue entry (idempotent), even if the
        // in-memory table was already gone, so a stale record can't
        // resurrect the table on the next open.
        if let Err(e) = self
            .system_store
            .remove_table(db_name, repo_name, table_name)
            .await
        {
            log::warn!(
                "shamir_db::drop_table: failed to remove table catalogue '{}/{}/{}': {}",
                db_name,
                repo_name,
                table_name,
                e
            );
        }

        Ok(removed)
    }

    fn extract_storage_type(factory: &BoxRepoFactory) -> String {
        match factory {
            BoxRepoFactory::InMemory(_) => "in_memory",
            #[cfg(feature = "sled")]
            BoxRepoFactory::Sled(_) => "sled",
            #[cfg(feature = "redb")]
            BoxRepoFactory::Redb(_) => "redb",
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(_) => "fjall",
            #[cfg(feature = "nebari")]
            BoxRepoFactory::Nebari(_) => "nebari",
            #[cfg(feature = "persy")]
            BoxRepoFactory::Persy(_) => "persy",
            #[cfg(feature = "canopy")]
            BoxRepoFactory::Canopy(_) => "canopy",
            // The buffer layer doesn't have an identity of its own
            // — recurse to the underlying backend so reflection
            // sees the real engine.
            BoxRepoFactory::MemBuffer(f) => return Self::extract_storage_type(&f.inner),
            BoxRepoFactory::Cached(f) => return Self::extract_storage_type(&f.inner),
        }
        .to_string()
    }

    fn extract_path(factory: &BoxRepoFactory) -> Option<String> {
        match factory {
            BoxRepoFactory::InMemory(_) => None,
            #[cfg(feature = "sled")]
            BoxRepoFactory::Sled(f) => Some(f.path.to_string_lossy().to_string()),
            #[cfg(feature = "redb")]
            BoxRepoFactory::Redb(f) => Some(f.path.to_string_lossy().to_string()),
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => Some(f.path.to_string_lossy().to_string()),
            #[cfg(feature = "nebari")]
            BoxRepoFactory::Nebari(f) => Some(f.path.to_string_lossy().to_string()),
            #[cfg(feature = "persy")]
            BoxRepoFactory::Persy(f) => Some(f.path.to_string_lossy().to_string()),
            #[cfg(feature = "canopy")]
            BoxRepoFactory::Canopy(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::MemBuffer(f) => Self::extract_path(&f.inner),
            BoxRepoFactory::Cached(f) => Self::extract_path(&f.inner),
        }
    }

    pub async fn remove_repo(&self, db_name: &str, repo_name: &str) -> bool {
        if let Some(db) = self.get_db(db_name) {
            let removed = db.remove_repo(repo_name).await;
            if removed {
                if let Err(e) = self
                    .system_store
                    .remove_repository(db_name, repo_name)
                    .await
                {
                    log::warn!(
                        "shamir_db::remove_repo: failed to remove '{}/{}' from system store: {}",
                        db_name,
                        repo_name,
                        e
                    );
                }
            }
            removed
        } else {
            false
        }
    }

    /// Direct table access shortcut.
    ///
    /// The returned `TableManager` has the global `ValidatorRegistry`
    /// injected (S3) so the write path can resolve validator bindings.
    pub async fn get_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<TableManager> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let mut table = db.get_table(repo_name, table_name).await?;
        table.set_validator_registry(self.validators().clone());
        Ok(table)
    }

    // ========================================================================
    // Function lifecycle API (slice 4)
    // ========================================================================

    /// Access the live function registry.
    ///
    /// Built-in functions (e.g. `argon2id`) live in the registry but are NOT
    /// persisted in the durable catalogue. Calling `drop_function` on a
    /// builtin removes it from the live registry only; it will not
    /// resurrect on restart (the next `init` calls `with_builtins()` and
    /// re-registers it). This is acceptable — builtins are an
    /// implementation detail, not user-managed functions.
    pub fn functions(&self) -> &Arc<FunctionRegistry> {
        &self.functions
    }

    /// Register a WASM function from pre-compiled binary bytes.
    ///
    /// If `replace` is false and a function with the same name already exists
    /// (either a builtin or a previously registered function), returns
    /// [`DbError::Function`].
    pub async fn create_function_from_wasm(
        &self,
        name: &str,
        wasm: &[u8],
        replace: bool,
    ) -> DbResult<()> {
        self.create_function_from_wasm_as(name, wasm, replace, Actor::System)
            .await
    }

    /// Like [`create_function_from_wasm`] but with an explicit [`Actor`].
    pub async fn create_function_from_wasm_as(
        &self,
        name: &str,
        wasm: &[u8],
        replace: bool,
        actor: Actor,
    ) -> DbResult<()> {
        let opts = CreateFunctionOptions {
            replace,
            ..CreateFunctionOptions::default()
        };
        self.create_function_with_opts_as(name, FunctionSource::Wasm(wasm), opts, actor)
            .await
    }

    /// Compile a Rust source string to WASM and register the function.
    ///
    /// Returns [`DbError::Function`] with a toolchain message if cargo or the
    /// `wasm32-unknown-unknown` target is not installed.
    pub async fn create_function_from_source(
        &self,
        name: &str,
        source: &str,
        replace: bool,
    ) -> DbResult<()> {
        self.create_function_from_source_as(name, source, replace, Actor::System)
            .await
    }

    /// Like [`create_function_from_source`] but with an explicit [`Actor`].
    pub async fn create_function_from_source_as(
        &self,
        name: &str,
        source: &str,
        replace: bool,
        actor: Actor,
    ) -> DbResult<()> {
        let opts = CreateFunctionOptions {
            replace,
            ..CreateFunctionOptions::default()
        };
        self.create_function_with_opts_as(name, FunctionSource::Source(source), opts, actor)
            .await
    }

    /// Canonical function creation with full options (slice 9).
    ///
    /// `source_or_wasm` is either a pre-compiled binary or Rust source.
    /// `opts` carries replace, visibility, security, and secret_grants.
    pub async fn create_function_with_opts(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        opts: CreateFunctionOptions,
    ) -> DbResult<()> {
        self.create_function_with_opts_as(name, source, opts, Actor::System)
            .await
    }

    /// Like [`create_function_with_opts`] but with an explicit [`Actor`].
    pub async fn create_function_with_opts_as(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        opts: CreateFunctionOptions,
        actor: Actor,
    ) -> DbResult<()> {
        self.authorize_access(&actor, &ResourcePath::FunctionNamespace, Action::Create)
            .await
            .map_err(|e| DbError::Function(e.to_string()))?;
        let (wasm, lang_tag, source_str) = match source {
            FunctionSource::Wasm(bytes) => (bytes.to_vec(), "wasm", None),
            FunctionSource::Source(src) => {
                let compiled = compile_rust_source(src).map_err(|e| match e {
                    FunctionError::ToolchainUnavailable(msg) => {
                        DbError::Function(format!("toolchain unavailable: {}", msg))
                    }
                    other => DbError::Function(other.to_string()),
                })?;
                (compiled, "rust", Some(src.to_string()))
            }
        };

        // Validate the wasm by compiling it.
        let wf = WasmFunction::from_binary(self.wasm_engine.clone(), &wasm, WasmLimits::default())
            .map_err(|e| DbError::Function(e.to_string()))?;

        let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm);
        let wasm_hash = format!("{:016x}", fxhash::hash64(&wasm));

        if !opts.replace && self.functions.contains(name) {
            return Err(DbError::Function(format!(
                "function '{}' already exists",
                name
            )));
        }

        let meta = FunctionMeta::new(opts.visibility, opts.security, opts.secret_grants.clone());

        let version = 1u64;
        let mut record = json!({
            "name": name,
            "wasm_b64": wasm_b64,
            "wasm_hash": wasm_hash,
            "lang": lang_tag,
            "source": source_str,
            "version": version,
        });
        meta.inject_into(&mut record);
        self.system_store
            .save_function(name, &record, &ResourceMeta::owned_by(actor.clone()))
            .await?;

        if opts.replace {
            self.functions.replace(name, Arc::new(wf));
        } else {
            self.functions
                .register(name, Arc::new(wf))
                .map_err(|e| DbError::Function(e.to_string()))?;
        }

        // Populate in-memory metadata.
        let _ = self.function_meta.remove(name);
        let _ = self.function_meta.insert(name.to_string(), meta);

        Ok(())
    }

    /// Drop a function by name. Returns `true` if it existed.
    ///
    /// For built-in functions (not in the durable catalogue), this only
    /// removes from the live registry. For user functions, both the durable
    /// record and the live entry are removed.
    pub async fn drop_function(&self, name: &str) -> DbResult<bool> {
        self.drop_function_as(name, Actor::System).await
    }

    /// Like [`drop_function`] but with an explicit [`Actor`].
    pub async fn drop_function_as(&self, name: &str, actor: Actor) -> DbResult<bool> {
        self.authorize_access(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Delete,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let existed = self.functions.remove(name);
        let _ = self.function_meta.remove(name);
        // Best-effort catalogue removal — ignore if there was no durable record
        // (e.g. builtins don't have one).
        if let Err(e) = self.system_store.remove_function(name).await {
            log::warn!(
                "shamir_db::drop_function: failed to remove '{}' from catalogue: {}",
                name,
                e
            );
        }
        Ok(existed)
    }

    /// Rename a function. The underlying WASM module is not recompiled.
    ///
    /// Fails if `from` does not exist or `to` is already taken.
    pub async fn rename_function(&self, from: &str, to: &str) -> DbResult<()> {
        self.rename_function_as(from, to, Actor::System).await
    }

    /// Like [`rename_function`] but with an explicit [`Actor`].
    pub async fn rename_function_as(&self, from: &str, to: &str, actor: Actor) -> DbResult<()> {
        self.authorize_access(
            &actor,
            &ResourcePath::Function {
                name: from.to_string(),
            },
            Action::Write,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        // Load the old catalogue record to re-key it.
        let fn_records = self.system_store.load_functions().await?;
        let old_record = fn_records
            .iter()
            .find(|r| r["name"].as_str() == Some(from))
            .cloned();

        // Rename in the live registry first.
        self.functions
            .rename(from, to)
            .map_err(|e| DbError::Function(e.to_string()))?;

        // Migrate in-memory metadata.
        if let Some((_, meta)) = self.function_meta.remove(from) {
            self.function_meta.insert(to.to_string(), meta);
        }

        // If there was a durable record, re-key it, preserving the
        // existing owner/group/mode.
        if let Some(mut rec) = old_record {
            let existing_meta = ResourceMeta::from_record(&rec);
            self.system_store.remove_function(from).await?;
            rec["name"] = json!(to);
            self.system_store
                .save_function(to, &rec, &existing_meta)
                .await?;
        }

        Ok(())
    }

    /// List all registered function names (sorted alphabetically).
    ///
    /// Includes builtins (e.g. `argon2id`) since they live in the live
    /// registry.
    pub async fn list_functions(&self) -> DbResult<Vec<String>> {
        let mut names = self.functions.list();
        names.sort();
        Ok(names)
    }

    /// Look up a function's in-memory metadata.
    ///
    /// Returns `None` for builtins (they have no catalogue entry).
    pub fn function_meta(&self, name: &str) -> Option<FunctionMeta> {
        self.function_meta.get(name).map(|r| r.value().clone())
    }

    /// Build an [`FnCtx`] with globals, registry, net gateway, the
    /// function's secret_grants from [`function_meta`], and the given
    /// [`Actor`] (R2).
    fn build_invoke_ctx(&self, fn_name: &str, actor: Actor) -> FnCtx {
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

    // ========================================================================
    // Validator lifecycle API (S1)
    // ========================================================================

    /// Access the live validator registry.
    pub fn validators(&self) -> &Arc<ValidatorRegistry> {
        &self.validators
    }

    /// Register a WASM validator from pre-compiled binary bytes.
    ///
    /// Returns the `RecordId` assigned to the new validator.
    /// If `replace` is false and a validator with the same name already
    /// exists, returns [`DbError::Validation`].
    pub async fn create_validator_from_wasm(
        &self,
        name: &str,
        wasm: &[u8],
        replace: bool,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Wasm(wasm), replace, Actor::System)
            .await
    }

    /// Like [`create_validator_from_wasm`] but with an explicit [`Actor`].
    pub async fn create_validator_from_wasm_as(
        &self,
        name: &str,
        wasm: &[u8],
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Wasm(wasm), replace, actor)
            .await
    }

    /// Compile a Rust source string to WASM and register the validator.
    ///
    /// Returns the `RecordId` assigned to the new validator.
    pub async fn create_validator_from_source(
        &self,
        name: &str,
        source: &str,
        replace: bool,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Source(source), replace, Actor::System)
            .await
    }

    /// Like [`create_validator_from_source`] but with an explicit [`Actor`].
    pub async fn create_validator_from_source_as(
        &self,
        name: &str,
        source: &str,
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        self.create_validator_inner(name, FunctionSource::Source(source), replace, actor)
            .await
    }

    /// Internal: compile/validate WASM, persist, register.
    async fn create_validator_inner(
        &self,
        name: &str,
        source: FunctionSource<'_>,
        replace: bool,
        actor: Actor,
    ) -> DbResult<RecordId> {
        let (wasm, lang_tag, source_str) = match source {
            FunctionSource::Wasm(bytes) => (bytes.to_vec(), "wasm", None),
            FunctionSource::Source(src) => {
                let compiled = compile_rust_source(src).map_err(|e| match e {
                    FunctionError::ToolchainUnavailable(msg) => {
                        DbError::Validation(format!("toolchain unavailable: {}", msg))
                    }
                    other => DbError::Validation(other.to_string()),
                })?;
                (compiled, "rust", Some(src.to_string()))
            }
        };

        // Validate the wasm by compiling it.
        let wf = WasmFunction::from_binary(self.wasm_engine.clone(), &wasm, WasmLimits::default())
            .map_err(|e| DbError::Validation(e.to_string()))?;

        let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm);
        let wasm_hash = format!("{:016x}", fxhash::hash64(&wasm));

        if !replace && self.validators.id_for_name(name).is_some() {
            return Err(DbError::Validation(format!(
                "validator '{}' already exists",
                name
            )));
        }

        // Determine the RecordId: on replace, reuse existing; otherwise new.
        let id = if replace {
            self.validators.id_for_name(name).unwrap_or_default()
        } else {
            RecordId::new()
        };

        let record = json!({
            "name": name,
            "_id": id.to_string(),
            "wasm_b64": wasm_b64,
            "wasm_hash": wasm_hash,
            "lang": lang_tag,
            "source": source_str,
            "bound_in": [],
        });
        // Persist before registering so a crash can't leave a live entry
        // without a catalogue record.
        self.system_store
            .save_validator(name, &record, &ResourceMeta::owned_by(actor))
            .await?;

        if replace {
            // Remove old entry if it exists; ignore errors.
            if let Some(old_id) = self.validators.id_for_name(name) {
                self.validators.remove(&old_id);
            }
        }
        self.validators
            .register(id, name, Arc::new(wf))
            .map_err(|e| DbError::Validation(e.to_string()))?;

        Ok(id)
    }

    /// Drop a validator by name.
    ///
    /// Returns `Ok(true)` if the validator existed and was removed.
    /// Returns `Err` if the validator is still bound to tables.
    pub async fn drop_validator(&self, name: &str) -> DbResult<bool> {
        self.drop_validator_as(name, Actor::System).await
    }

    /// Like [`drop_validator`] but with an explicit [`Actor`].
    pub async fn drop_validator_as(&self, name: &str, _actor: Actor) -> DbResult<bool> {
        let id = match self.validators.id_for_name(name) {
            Some(id) => id,
            None => return Ok(false),
        };

        // Refuse if bound.
        if self.validators.is_bound(&id) {
            let tables = self.validators.bound_tables(&id);
            return Err(DbError::Validation(format!(
                "cannot drop validator '{}': still bound to tables: {}",
                name,
                tables.join(", ")
            )));
        }

        let existed = self.validators.remove(&id);
        // Best-effort catalogue removal.
        if let Err(e) = self.system_store.remove_validator(name).await {
            log::warn!(
                "shamir_db::drop_validator: failed to remove '{}' from catalogue: {}",
                name,
                e
            );
        }
        Ok(existed)
    }

    /// Rename a validator. The underlying WASM module is not recompiled.
    /// Id and bindings are unchanged.
    pub async fn rename_validator(&self, from: &str, to: &str) -> DbResult<()> {
        self.rename_validator_as(from, to, Actor::System).await
    }

    /// Like [`rename_validator`] but with an explicit [`Actor`].
    pub async fn rename_validator_as(&self, from: &str, to: &str, _actor: Actor) -> DbResult<()> {
        // Load the old catalogue record to re-key it.
        let old_record = self.system_store.load_validator(from).await?;

        // Rename in the live registry first.
        self.validators
            .rename(from, to)
            .map_err(|e| DbError::Validation(e.to_string()))?;

        // If there was a durable record, re-key it, preserving the
        // existing owner/group/mode.
        if let Some(mut rec) = old_record {
            let existing_meta = ResourceMeta::from_record(&rec);
            self.system_store.remove_validator(from).await?;
            rec["name"] = json!(to);
            self.system_store
                .save_validator(to, &rec, &existing_meta)
                .await?;
        }

        Ok(())
    }

    /// Resolve a validator name to its `RecordId`.
    pub fn validator_id(&self, name: &str) -> Option<RecordId> {
        self.validators.id_for_name(name)
    }

    /// List all registered validators as `(id, name)` pairs.
    pub fn list_validators(&self) -> Vec<(RecordId, String)> {
        self.validators.list()
    }

    // ========================================================================
    // Validator binding API (S2)
    // ========================================================================

    /// Canonical table-ref string for validator binding tracking.
    /// Format: `"db/repo/table"` — matches the form used by
    /// `ValidatorRegistry::add_binding` / `bound_tables`.
    fn table_ref_str(db: &str, repo: &str, table: &str) -> String {
        format!("{}/{}/{}", db, repo, table)
    }

    /// Bind a validator to a table on specified write operations.
    ///
    /// The validator must already exist in the registry (created via
    /// `create_validator_from_wasm` / `create_validator_from_source`).
    /// `priority` must be in `[1000, 9999]`, `ops` must be non-empty.
    /// Bind is idempotent: re-binding the same validator updates its
    /// `ops` and `priority`.
    pub async fn bind_validator(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
        ops: Vec<WriteOp>,
        priority: u16,
    ) -> DbResult<()> {
        self.bind_validator_as(
            db_name,
            repo_name,
            table_name,
            validator_name,
            ops,
            priority,
            Actor::System,
        )
        .await
    }

    /// Like [`bind_validator`] but with an explicit [`Actor`].
    #[allow(clippy::too_many_arguments)]
    pub async fn bind_validator_as(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
        ops: Vec<WriteOp>,
        priority: u16,
        _actor: Actor,
    ) -> DbResult<()> {
        // 1. Resolve name → id.
        let validator_id = self.validators.id_for_name(validator_name).ok_or_else(|| {
            DbError::Validation(format!("validator '{}' not found", validator_name))
        })?;

        // 2. Validate priority range.
        if !(1000..=9999).contains(&priority) {
            return Err(DbError::Validation(format!(
                "validator priority must be in [1000, 9999], got {}",
                priority
            )));
        }

        // 3. Validate ops non-empty.
        if ops.is_empty() {
            return Err(DbError::Validation(
                "validator binding ops must be non-empty".to_string(),
            ));
        }

        // 4. Get the TableManager.
        let table = self.get_table(db_name, repo_name, table_name).await?;

        // 5. Add the binding to the table's info-twin.
        let binding = ValidatorBinding {
            validator_id,
            ops: ops.into(),
            priority,
        };
        table.add_validator_binding(binding).await?;

        // 6. Update the global registry's bound_in tracking.
        let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
        self.validators.add_binding(&validator_id, &table_ref);

        // 7. Persist bound_in in the validator catalogue record.
        self.persist_validator_bound_in(validator_name, &validator_id)
            .await;

        Ok(())
    }

    /// Unbind a validator from a table.
    ///
    /// Returns `Ok(true)` if the binding existed and was removed.
    pub async fn unbind_validator(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
    ) -> DbResult<bool> {
        self.unbind_validator_as(
            db_name,
            repo_name,
            table_name,
            validator_name,
            Actor::System,
        )
        .await
    }

    /// Like [`unbind_validator`] but with an explicit [`Actor`].
    pub async fn unbind_validator_as(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        validator_name: &str,
        _actor: Actor,
    ) -> DbResult<bool> {
        let validator_id = self.validators.id_for_name(validator_name).ok_or_else(|| {
            DbError::Validation(format!("validator '{}' not found", validator_name))
        })?;

        let table = self.get_table(db_name, repo_name, table_name).await?;
        let removed = table.remove_validator_binding(&validator_id).await?;

        if removed {
            let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
            self.validators.remove_binding(&validator_id, &table_ref);

            // Persist bound_in update in the validator catalogue.
            self.persist_validator_bound_in(validator_name, &validator_id)
                .await;
        }

        Ok(removed)
    }

    /// List the validator bindings for a specific table.
    pub async fn list_validator_bindings(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<Vec<ValidatorBinding>> {
        let table = self.get_table(db_name, repo_name, table_name).await?;
        Ok((*table.validator_bindings()).clone())
    }

    /// Best-effort update of the `bound_in` array in the validator's
    /// catalogue record. Logged on failure — does not propagate errors
    /// (the live registry is the source of truth; the catalogue is
    /// durability insurance for `init` reload).
    async fn persist_validator_bound_in(&self, name: &str, id: &RecordId) {
        let tables = self.validators.bound_tables(id);
        let bound_json: Vec<serde_json::Value> =
            tables.into_iter().map(serde_json::Value::String).collect();

        if let Ok(Some(mut rec)) = self.system_store.load_validator(name).await {
            let existing_meta = ResourceMeta::from_record(&rec);
            rec["bound_in"] = serde_json::Value::Array(bound_json);
            if let Err(e) = self
                .system_store
                .save_validator(name, &rec, &existing_meta)
                .await
            {
                log::warn!(
                    "shamir_db::persist_validator_bound_in: failed to update '{}': {}",
                    name,
                    e
                );
            }
        }
    }

    /// Invoke a function by name with the given parameters.
    ///
    /// Each call gets a fresh per-invocation batch context (no data sharing
    /// between calls). Use [`invoke_function_with_batch`] for multi-call
    /// batched invocation.
    pub async fn invoke_function(&self, name: &str, params: Params) -> DbResult<QueryValue> {
        self.invoke_function_as(name, params, Actor::System).await
    }

    /// Like [`invoke_function`] but with an explicit [`Actor`].
    pub async fn invoke_function_as(
        &self,
        name: &str,
        params: Params,
        caller: Actor,
    ) -> DbResult<QueryValue> {
        self.authorize_access(
            &caller,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let actor = self.effective_fn_actor(name, &caller).await;
        let ctx = self.build_invoke_ctx(name, actor);
        self.functions
            .invoke(name, &ctx, &FnBatch::new(), &params)
            .await
            .map_err(|e| DbError::Function(e.to_string()))
    }

    /// Invoke a function sharing an existing batch context.
    ///
    /// Multiple invocations sharing the same `batch` can exchange data
    /// through it ("function A writes, function B reads").
    pub async fn invoke_function_with_batch(
        &self,
        name: &str,
        params: Params,
        batch: &Arc<BatchContext>,
    ) -> DbResult<QueryValue> {
        self.invoke_function_with_batch_as(name, params, batch, Actor::System)
            .await
    }

    /// Like [`invoke_function_with_batch`] but with an explicit [`Actor`].
    pub async fn invoke_function_with_batch_as(
        &self,
        name: &str,
        params: Params,
        batch: &Arc<BatchContext>,
        caller: Actor,
    ) -> DbResult<QueryValue> {
        self.authorize_access(
            &caller,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let actor = self.effective_fn_actor(name, &caller).await;
        let ctx = self.build_invoke_ctx(name, actor);
        self.functions
            .invoke(name, &ctx, &FnBatch::with_context(batch.clone()), &params)
            .await
            .map_err(|e| DbError::Function(e.to_string()))
    }

    /// Access the database-global variables store.
    pub fn globals(&self) -> &Arc<GlobalVars> {
        &self.globals
    }

    /// Create a fresh batch context for use with [`invoke_function_with_batch`].
    pub fn new_batch_context(&self) -> Arc<BatchContext> {
        Arc::new(BatchContext::new())
    }

    /// Set the network egress allowlist (host patterns for HTTP fetch).
    ///
    /// Default is empty (deny all). Must be called before any function
    /// invocation that uses `ctx.http_fetch()`.
    pub fn set_net_allowlist(&mut self, allowlist: Vec<String>) {
        self.net_allowlist = Arc::new(allowlist);
    }

    /// Build a [`NetGateway`] from the current allowlist.
    ///
    /// Always returns a gateway so that allowlist-denial is a catchable
    /// runtime error, not a "no net gateway" trap.
    fn build_net_gateway(&self) -> Arc<dyn NetGateway> {
        Arc::new(super::curl_gateway::CurlNetGateway::new(
            self.net_allowlist.to_vec(),
        ))
    }

    /// Invoke a function with database read/write access (slice 8b).
    ///
    /// Builds an [`FnCtx`] that carries a [`DbGateway`] routing through
    /// [`ShamirDb::execute`] so the function can call
    /// `ctx.db().table("t").insert(doc)` / `.query(filter)` / `.get(key)`.
    ///
    /// Each db host-import call is autocommitted independently (no enclosing
    /// transaction). Full transactional integration (RYOW/SSI) is deferred
    /// until functions execute as batch ops.
    ///
    /// `db_name` is the database to route through, `repo` is the default
    /// repository for table operations.
    pub async fn invoke_function_in_db(
        &self,
        db_name: &str,
        repo: &str,
        name: &str,
        params: Params,
    ) -> DbResult<QueryValue> {
        self.invoke_function_in_db_as(db_name, repo, name, params, Actor::System)
            .await
    }

    /// Like [`invoke_function_in_db`] but with an explicit [`Actor`].
    pub async fn invoke_function_in_db_as(
        &self,
        db_name: &str,
        repo: &str,
        name: &str,
        params: Params,
        caller: Actor,
    ) -> DbResult<QueryValue> {
        self.authorize_access(
            &caller,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let actor = self.effective_fn_actor(name, &caller).await;
        let gateway = Arc::new(FacadeDbGateway {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        });
        let grants = self
            .function_meta(name)
            .map(|m| m.secret_grants)
            .unwrap_or_default();
        let ctx = FnCtx::with_globals(self.globals.clone())
            .with_registry(self.functions.clone())
            .with_db(gateway, repo.to_string())
            .with_net(self.build_net_gateway())
            .with_secret_grants(grants)
            .with_actor(actor);
        self.functions
            .invoke(name, &ctx, &FnBatch::new(), &params)
            .await
            .map_err(|e| DbError::Function(e.to_string()))
    }

    /// Like [`invoke_function_in_db`] but shares an existing batch context.
    pub async fn invoke_function_in_db_with_batch(
        &self,
        db_name: &str,
        repo: &str,
        name: &str,
        params: Params,
        batch: &Arc<BatchContext>,
    ) -> DbResult<QueryValue> {
        self.invoke_function_in_db_with_batch_as(db_name, repo, name, params, batch, Actor::System)
            .await
    }

    /// Like [`invoke_function_in_db_with_batch`] but with an explicit [`Actor`].
    pub async fn invoke_function_in_db_with_batch_as(
        &self,
        db_name: &str,
        repo: &str,
        name: &str,
        params: Params,
        batch: &Arc<BatchContext>,
        caller: Actor,
    ) -> DbResult<QueryValue> {
        self.authorize_access(
            &caller,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let actor = self.effective_fn_actor(name, &caller).await;
        let gateway = Arc::new(FacadeDbGateway {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        });
        let grants = self
            .function_meta(name)
            .map(|m| m.secret_grants)
            .unwrap_or_default();
        let ctx = FnCtx::with_globals(self.globals.clone())
            .with_registry(self.functions.clone())
            .with_db(gateway, repo.to_string())
            .with_net(self.build_net_gateway())
            .with_secret_grants(grants)
            .with_actor(actor);
        self.functions
            .invoke(name, &ctx, &FnBatch::with_context(batch.clone()), &params)
            .await
            .map_err(|e| DbError::Function(e.to_string()))
    }

    // ========================================================================
    // Resource metadata resolver + groups (P3 metadata plates)
    // ========================================================================

    /// Resolve the [`ResourceMeta`] for a given [`ResourcePath`].
    ///
    /// - **Record / Index** inherit their Table's meta.
    /// - **Root** and unknown / missing paths default to [`ResourceMeta::open`].
    /// - All other mode-bearing objects read from the persistent catalogue.
    /// - **FunctionNamespace** is stored as a settings entry keyed
    ///   `"fn_namespace_meta"`, defaulting to `open()`.
    pub async fn resource_meta(&self, path: &ResourcePath) -> ResourceMeta {
        let table_path = match path {
            // Record and Index inherit their Table's meta.
            ResourcePath::Record {
                db, store, table, ..
            }
            | ResourcePath::Index {
                db, store, table, ..
            } => ResourcePath::table(db, store, table),
            ResourcePath::Table { .. } => path.clone(),
            _ => path.clone(),
        };

        match &table_path {
            ResourcePath::Database { db } => {
                let rec = self.system_store.load_database(db).await;
                rec.ok()
                    .flatten()
                    .map(|r| ResourceMeta::from_record(&r))
                    .unwrap_or_default()
            }
            ResourcePath::Store { db, store } => {
                let rec = self.system_store.load_repository(db, store).await;
                rec.ok()
                    .flatten()
                    .map(|r| ResourceMeta::from_record(&r))
                    .unwrap_or_default()
            }
            ResourcePath::Table { db, store, table } => {
                let rec = self.system_store.load_table_record(db, store, table).await;
                rec.ok()
                    .flatten()
                    .map(|r| ResourceMeta::from_record(&r))
                    .unwrap_or_default()
            }
            ResourcePath::Function { name } => {
                let rec = self.system_store.load_function(name).await;
                rec.ok()
                    .flatten()
                    .map(|r| ResourceMeta::from_record(&r))
                    .unwrap_or_default()
            }
            ResourcePath::FunctionFolder { .. } => {
                // Folder meta persistence is a later stage; default to open.
                ResourceMeta::open()
            }
            ResourcePath::FunctionNamespace => {
                let val = self
                    .system_store
                    .load_setting("fn_namespace_meta")
                    .await
                    .ok()
                    .flatten();
                val.map(|v| ResourceMeta::from_record(&v))
                    .unwrap_or_default()
            }
            ResourcePath::Root | ResourcePath::User { .. } | ResourcePath::Group { .. } => {
                ResourceMeta::open()
            }
            // Record/Index already resolved to Table above; if something
            // slips through, return open.
            ResourcePath::Record { .. } | ResourcePath::Index { .. } => ResourceMeta::open(),
        }
    }

    /// Durable write of [`ResourceMeta`] for a mode-bearing resource.
    ///
    /// Loads the existing catalogue record, injects the new meta fields,
    /// and writes it back. This is the storage API; DDL wiring (chmod/chown)
    /// is deferred to a later slice.
    pub async fn set_resource_meta(
        &self,
        path: &ResourcePath,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        match path {
            ResourcePath::Database { db } => {
                let rec = self
                    .system_store
                    .load_database(db)
                    .await?
                    .ok_or_else(|| DbError::NotFound(format!("database '{}' not found", db)))?;
                let mut rec = rec;
                meta.inject_into(&mut rec);
                self.system_store.save_database_meta(db, &rec).await
            }
            ResourcePath::Store { db, store } => {
                let rec = self
                    .system_store
                    .load_repository(db, store)
                    .await?
                    .ok_or_else(|| {
                        DbError::NotFound(format!("store '{}/{}' not found", db, store))
                    })?;
                let mut rec = rec;
                meta.inject_into(&mut rec);
                self.system_store.save_repository_meta(&rec).await
            }
            ResourcePath::Table { db, store, table } => {
                let rec = self
                    .system_store
                    .load_table_record(db, store, table)
                    .await?
                    .ok_or_else(|| {
                        DbError::NotFound(format!("table '{}/{}/{}' not found", db, store, table))
                    })?;
                let mut rec = rec;
                meta.inject_into(&mut rec);
                self.system_store.save_table_meta(&rec).await
            }
            ResourcePath::Function { name } => {
                let rec = self
                    .system_store
                    .load_function(name)
                    .await?
                    .ok_or_else(|| DbError::NotFound(format!("function '{}' not found", name)))?;
                let mut rec = rec;
                meta.inject_into(&mut rec);
                self.system_store
                    .save_function_meta_record(name, &rec)
                    .await
            }
            ResourcePath::FunctionFolder { .. } => {
                // Folder meta persistence is a later stage.
                Err(DbError::NotFound(format!(
                    "resource path '{}' does not support set_resource_meta yet",
                    path
                )))
            }
            ResourcePath::FunctionNamespace => {
                let mut rec = serde_json::json!({"key": "fn_namespace_meta"});
                meta.inject_into(&mut rec);
                self.system_store
                    .save_setting("fn_namespace_meta", &rec)
                    .await
            }
            // Root, User, Group, Record, Index — not directly settable via
            // catalogue in this slice. Root is always open; Record/Index
            // inherit from their Table.
            _ => Err(DbError::NotFound(format!(
                "resource path '{}' does not support set_resource_meta in this slice",
                path
            ))),
        }
    }

    /// Create a group with the given name. Returns the allocated group id.
    ///
    /// Group ids are allocated monotonically from a counter stored in the
    /// `settings` table under the key `"next_group_id"`. Id 0 is
    /// reserved/unused; allocation starts from 1.
    pub async fn create_group(&self, name: &str) -> DbResult<u64> {
        // Serialise the whole read-modify-write (rare op, bounded contention).
        let _guard = self.group_id_lock.lock().await;

        let current = match self
            .system_store
            .load_setting("next_group_id")
            .await?
            .and_then(|v| v.as_u64())
        {
            Some(v) => v,
            // Counter absent: seed past the highest EXISTING group id so a
            // lost/missing setting can't collide with a live group.
            None => {
                let max = self
                    .system_store
                    .load_groups()
                    .await?
                    .iter()
                    .filter_map(|g| g["group_id"].as_u64())
                    .max();
                max.map_or(1, |m| m + 1)
            }
        };
        let group_id = current;

        // Durability: bump the counter BEFORE writing the group, so a crash
        // in between only LEAKS an id (monotonic) — it can never overwrite the
        // next group on restart.
        self.system_store
            .save_setting("next_group_id", &serde_json::json!(current + 1))
            .await?;
        self.system_store.save_group(group_id, name, &[]).await?;
        Ok(group_id)
    }

    /// Drop a group by id.
    pub async fn drop_group(&self, group_id: u64) -> DbResult<()> {
        self.system_store.remove_group(group_id).await
    }

    /// Add a user to a group.
    pub async fn add_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        self.system_store.add_group_member(group_id, user_id).await
    }

    /// Remove a user from a group.
    pub async fn remove_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        self.system_store
            .remove_group_member(group_id, user_id)
            .await
    }

    /// Resolve a [`GroupRef`] to a numeric group id.
    ///
    /// `GroupRef::Id` returns the id directly. `GroupRef::Name` scans
    /// the groups table for a matching name. Returns `Err` if the name
    /// does not resolve to any group.
    pub async fn resolve_group_id(
        &self,
        group_ref: &crate::query::admin::GroupRef,
    ) -> DbResult<u64> {
        match group_ref {
            crate::query::admin::GroupRef::Id { id } => Ok(*id),
            crate::query::admin::GroupRef::Name { name } => {
                let groups = self.system_store.load_groups().await?;
                let id = groups
                    .iter()
                    .find(|g| g["name"].as_str() == Some(name.as_str()))
                    .and_then(|g| g["group_id"].as_u64())
                    .ok_or_else(|| DbError::NotFound(format!("group '{}' not found", name)))?;
                Ok(id)
            }
        }
    }

    /// Get the members of a group.
    pub async fn group_members(&self, group_id: u64) -> DbResult<Vec<u64>> {
        let rec = self.system_store.load_group(group_id).await?;
        Ok(rec
            .and_then(|r| {
                r["members"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            })
            .unwrap_or_default())
    }

    /// Check whether a user belongs to a group.
    pub async fn user_in_group(&self, user_id: u64, group_id: u64) -> DbResult<bool> {
        let members = self.group_members(group_id).await?;
        Ok(members.contains(&user_id))
    }

    // ========================================================================
    // Shomer enforcement gate (P4)
    // ========================================================================

    /// Enforcing authorization gate.
    ///
    /// Performs the full POSIX-style check:
    /// 1. `Actor::System` → `Ok` immediately (admin bypass, zero overhead
    ///    beyond the branch — the common live path).
    /// 2. **Traversal**: for each ancestor in `path.ancestors()` (nearest →
    ///    Root), the actor needs `Execute` on it. Resolves meta, computes
    ///    `in_group`, and checks [`permits`].
    /// 3. **Target**: resolves `resource_meta(path)`, computes `in_group`,
    ///    and checks [`permits`] for the requested `action`.
    ///
    /// On denial, builds an [`AccessError`] identifying the actor, the
    /// denied path, and the action. The engine-level trace
    /// ([`authorize`]) is still emitted for observability.
    pub async fn authorize_access(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        // Engine-level trace (R2) — always emitted.
        authorize(actor, path, action)?;

        // Admin bypass — the common live path.
        if matches!(actor, Actor::System) {
            return Ok(());
        }

        let user_id = match actor {
            Actor::User(id) => *id,
            Actor::System => unreachable!(),
        };

        // Traversal: each ancestor needs Execute.
        for anc in path.ancestors() {
            let anc_meta = self.resource_meta(&anc).await;
            let in_group = self.resolve_in_group(user_id, &anc_meta).await;
            if !permits(actor, &anc_meta, Action::Execute, in_group) {
                return Err(AccessError {
                    actor: actor.clone(),
                    path: anc.to_string(),
                    action: Action::Execute,
                });
            }
        }

        // Target check.
        let meta = self.resource_meta(path).await;
        let in_group = self.resolve_in_group(user_id, &meta).await;
        if permits(actor, &meta, action, in_group) {
            Ok(())
        } else {
            Err(AccessError {
                actor: actor.clone(),
                path: path.to_string(),
                action,
            })
        }
    }

    /// Resolve whether the user belongs to the group specified in `meta`.
    ///
    /// Returns `false` if the meta has no group or the lookup fails.
    async fn resolve_in_group(&self, user_id: u64, meta: &ResourceMeta) -> bool {
        match meta.group {
            Some(gid) => self.user_in_group(user_id, gid).await.unwrap_or(false),
            None => false,
        }
    }

    /// Resolve the effective actor for function invocation.
    ///
    /// If the function's metadata has the setuid flag set, the function
    /// runs with its owner's authority (definer rights). Otherwise the
    /// caller's actor is used unchanged.
    pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
        let meta = self.resource_meta(&ResourcePath::function(fn_name)).await;
        if Mode::is_setuid(meta.mode) {
            meta.owner
        } else {
            caller.clone()
        }
    }

    /// Assemble the access-control tree as structured JSON.
    ///
    /// Shape (see [`shamir_query_types::admin::AccessTreeOp`]):
    /// ```json
    /// {
    ///   "resources": { "name": "/", "kind": "root", "owner": 0,
    ///                  "owner_name": "system", "group": null,
    ///                  "group_name": null, "mode": 511, "setuid": false,
    ///                  "children": [ /* databases → stores → tables */ ] },
    ///   "functions": [ { "name": "...", "owner": .., "mode": .., "setuid": .. } ],
    ///   "principals": {
    ///     "users":  [ { "id": .., "name": ".." } ],
    ///     "groups": [ { "id": .., "name": "..", "members": [ {id,name} ] } ]
    ///   }
    /// }
    /// ```
    ///
    /// `depth` caps the resource hierarchy (`0`=root, `1`=databases,
    /// `2`=stores, `3`=tables; `None`=full). `db_filter` restricts the
    /// resource tree to one database. Pure read-only assembly — the admin
    /// gate is applied by the caller (the DDL dispatch authorizes `Manage`
    /// on the root; the offline CLI runs as `System`).
    pub async fn access_tree(
        &self,
        depth: Option<u32>,
        db_filter: Option<&str>,
    ) -> DbResult<serde_json::Value> {
        use std::collections::HashMap;

        // ── principals first, so resource nodes resolve owner/group names ──
        let mut name_of: HashMap<u64, String> = HashMap::new();
        name_of.insert(OWNER_SYSTEM, "system".to_string());
        let mut users_json: Vec<serde_json::Value> = Vec::new();
        for rec in self.system_store.load_users().await? {
            if let Some(uname) = rec.get("name").and_then(|v| v.as_str()) {
                let id = principal_id(uname);
                name_of.insert(id, uname.to_string());
                users_json.push(json!({ "id": id, "name": uname }));
            }
        }
        users_json.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        let mut group_name_of: HashMap<u64, String> = HashMap::new();
        let mut groups_json: Vec<serde_json::Value> = Vec::new();
        for rec in self.system_store.load_groups().await? {
            let Some(gid) = rec.get("group_id").and_then(|v| v.as_u64()) else {
                continue;
            };
            let gname = rec
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            group_name_of.insert(gid, gname.clone());
            let members: Vec<serde_json::Value> = rec
                .get("members")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.as_u64())
                        .map(|m| json!({ "id": m, "name": name_of.get(&m).cloned() }))
                        .collect()
                })
                .unwrap_or_default();
            groups_json.push(json!({ "id": gid, "name": gname, "members": members }));
        }
        groups_json.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        // ── resource hierarchy (Root → Database → Store → Table) ──
        let max_depth = depth.unwrap_or(3).min(3);
        let root_meta = self.resource_meta(&ResourcePath::Root).await;
        let mut root = access_node("/", "root", &root_meta, &name_of, &group_name_of);

        if max_depth >= 1 {
            let dbs: Vec<String> = match db_filter {
                Some(d) => self.list_dbs().into_iter().filter(|x| x == d).collect(),
                None => self.list_dbs(),
            };
            let mut db_children: Vec<serde_json::Value> = Vec::new();
            for dbname in dbs {
                let dm = self.resource_meta(&ResourcePath::database(&dbname)).await;
                let mut dbnode = access_node(&dbname, "database", &dm, &name_of, &group_name_of);
                if max_depth >= 2 {
                    if let Some(inst) = self.get_db(&dbname) {
                        let mut store_children: Vec<serde_json::Value> = Vec::new();
                        for store in inst.list_repos() {
                            let sm = self
                                .resource_meta(&ResourcePath::store(&dbname, &store))
                                .await;
                            let mut snode =
                                access_node(&store, "store", &sm, &name_of, &group_name_of);
                            if max_depth >= 3 {
                                if let Ok(tables) = inst.list_tables(&store) {
                                    let mut tnodes: Vec<serde_json::Value> = Vec::new();
                                    for t in tables {
                                        let tm = self
                                            .resource_meta(&ResourcePath::table(
                                                &dbname, &store, &t,
                                            ))
                                            .await;
                                        tnodes.push(access_node(
                                            &t,
                                            "table",
                                            &tm,
                                            &name_of,
                                            &group_name_of,
                                        ));
                                    }
                                    snode["children"] = serde_json::Value::Array(tnodes);
                                }
                            }
                            store_children.push(snode);
                        }
                        dbnode["children"] = serde_json::Value::Array(store_children);
                    }
                }
                db_children.push(dbnode);
            }
            root["children"] = serde_json::Value::Array(db_children);
        }

        // ── functions (flat for now; folders land in a later slice) ──
        let mut functions: Vec<serde_json::Value> = Vec::new();
        for fname in self.list_functions().await? {
            let fm = self.resource_meta(&ResourcePath::function(&fname)).await;
            let mut fnode = access_node(&fname, "function", &fm, &name_of, &group_name_of);
            if let Some(obj) = fnode.as_object_mut() {
                obj.remove("children");
                obj.insert(
                    "builtin".to_string(),
                    serde_json::Value::Bool(self.function_meta(&fname).is_none()),
                );
            }
            functions.push(fnode);
        }

        Ok(json!({
            "resources": root,
            "functions": functions,
            "principals": { "users": users_json, "groups": groups_json },
        }))
    }
}

/// Build one access-tree node as JSON, resolving the owner/group ids to
/// names via the supplied lookups. Callers attach `children` afterwards
/// (leaf nodes keep the empty array; functions drop it).
fn access_node(
    name: &str,
    kind: &str,
    meta: &ResourceMeta,
    name_of: &std::collections::HashMap<u64, String>,
    group_name_of: &std::collections::HashMap<u64, String>,
) -> serde_json::Value {
    let owner_id = meta.owner.to_owner_id();
    json!({
        "name": name,
        "kind": kind,
        "owner": owner_id,
        "owner_name": name_of.get(&owner_id).cloned(),
        "group": meta.group,
        "group_name": meta.group.and_then(|g| group_name_of.get(&g).cloned()),
        "mode": meta.mode,
        "setuid": Mode::is_setuid(meta.mode),
        "children": [],
    })
}

// ── FacadeDbGateway ──────────────────────────────────────────────────

/// [`DbGateway`] implementation that routes through [`ShamirDb::execute`].
///
/// Each method builds a single-op [`BatchRequest`] and submits it via
/// `execute`, which commits independently (autocommit-per-op).
///
/// # Re-entrancy note
///
/// For this slice functions are invoked standalone (not from within an
/// `execute` call), so routing back through `execute` is safe. When
/// functions are later invoked *as batch ops*, the gateway must inherit
/// the batch's transaction instead of opening a new `execute` — otherwise
/// it would deadlock on the batch planner.
struct FacadeDbGateway {
    shamir: ShamirDb,
    db_name: String,
    /// Effective actor of the invoking function (caller, or function owner
    /// under setuid).  The gateway runs the function's DB access AS this
    /// actor so per-table ACLs apply — NOT as System.
    actor: Actor,
}

impl FacadeDbGateway {
    /// Convert a `QueryValue` key into a JSON filter suitable for a `ReadQuery`.
    ///
    /// Key convention:
    /// - `QueryValue::Map` → conjunction of `Eq` filters on each entry.
    /// - Scalar `QueryValue` (e.g. `Int`, `Str`) → `Eq` on the `"id"` field.
    fn key_to_filter(key: &QueryValue) -> serde_json::Value {
        match key {
            QueryValue::Map(entries) => {
                if entries.is_empty() {
                    return json!(null);
                }
                let filters: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|(field, val)| {
                        let json_val = serde_json::to_value(val).unwrap_or(json!(null));
                        json!({
                            "op": "eq",
                            "field": [field],
                            "value": json_val
                        })
                    })
                    .collect();
                if filters.len() == 1 {
                    return filters.into_iter().next().unwrap_or(json!(null));
                }
                json!({
                    "op": "and",
                    "filters": filters
                })
            }
            other => {
                let json_val = serde_json::to_value(other).unwrap_or(json!(null));
                json!({
                    "op": "eq",
                    "field": ["id"],
                    "value": json_val
                })
            }
        }
    }

    fn batch_err_to_string(e: BatchError) -> String {
        format!("{e:?}")
    }
}

#[async_trait]
impl DbGateway for FacadeDbGateway {
    async fn get(
        &self,
        repo: &str,
        table: &str,
        key: QueryValue,
    ) -> Result<Option<QueryValue>, String> {
        let filter = Self::key_to_filter(&key);
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let where_clause = if filter.is_null() {
            None
        } else {
            Some(
                serde_json::from_value(filter)
                    .map_err(|e| format!("get: filter parse error: {e}"))?,
            )
        };

        let read_query = ReadQuery {
            from: table_ref,
            select: crate::engine::query::read::Select::all(),
            r#where: where_clause,
            group_by: None,
            order_by: None,
            pagination: crate::engine::query::read::Pagination::None,
            count_total: false,
        };

        let mut queries = new_map();
        queries.insert(
            "r".to_string(),
            QueryEntry {
                op: BatchOp::Read(read_query),
                return_result: true,
            },
        );
        let req = BatchRequest {
            id: json!("db_get"),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["r".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;

        let result = match resp.results.get("r") {
            Some(r) => r,
            None => return Ok(None),
        };

        match result.records.first() {
            Some(rec) => {
                let qv = serde_json::from_value(rec.clone())
                    .map_err(|e| format!("get: record decode error: {e}"))?;
                Ok(Some(qv))
            }
            None => Ok(None),
        }
    }

    async fn insert(&self, repo: &str, table: &str, doc: QueryValue) -> Result<QueryValue, String> {
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let json_val =
            serde_json::to_value(&doc).map_err(|e| format!("insert: doc encode error: {e}"))?;

        let insert_op = InsertOp {
            insert_into: table_ref,
            values: vec![json_val],
        };

        let mut queries = new_map();
        queries.insert(
            "i".to_string(),
            QueryEntry {
                op: BatchOp::Insert(insert_op),
                return_result: true,
            },
        );
        let req = BatchRequest {
            id: json!("db_insert"),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["i".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;

        let result = match resp.results.get("i") {
            Some(r) => r,
            None => return Err("insert: no result returned".to_string()),
        };

        match result.records.first() {
            Some(rec) => {
                let qv = serde_json::from_value(rec.clone())
                    .map_err(|e| format!("insert: record decode error: {e}"))?;
                Ok(qv)
            }
            None => Err("insert: empty result".to_string()),
        }
    }

    async fn query(
        &self,
        repo: &str,
        table: &str,
        filter: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, String> {
        let table_ref = if repo == "main" {
            TableRef::new(table)
        } else {
            TableRef::with_repo(repo, table)
        };

        let where_clause = match filter {
            Some(f) => {
                let json_filter = Self::key_to_filter(&f);
                if json_filter.is_null() {
                    None
                } else {
                    Some(
                        serde_json::from_value(json_filter)
                            .map_err(|e| format!("query: filter parse error: {e}"))?,
                    )
                }
            }
            None => None,
        };

        let read_query = ReadQuery {
            from: table_ref,
            select: crate::engine::query::read::Select::all(),
            r#where: where_clause,
            group_by: None,
            order_by: None,
            pagination: crate::engine::query::read::Pagination::None,
            count_total: false,
        };

        let mut queries = new_map();
        queries.insert(
            "q".to_string(),
            QueryEntry {
                op: BatchOp::Read(read_query),
                return_result: true,
            },
        );
        let req = BatchRequest {
            id: json!("db_query"),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: false,
            return_only: Some(vec!["q".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute_as(self.actor.clone(), &self.db_name, &req)
            .await
            .map_err(Self::batch_err_to_string)?;

        let result = match resp.results.get("q") {
            Some(r) => r,
            None => return Ok(Vec::new()),
        };

        result
            .records
            .iter()
            .map(|rec| {
                serde_json::from_value(rec.clone())
                    .map_err(|e| format!("query: record decode error: {e}"))
            })
            .collect()
    }
}
