use base64::Engine;
use serde_json::json;

use crate::access::{authorize, Action, Actor, ResourcePath};
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
    function_meta: DashMap<String, FunctionMeta>,
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
        let system_store = SystemStore::init(config).await?;

        let dbs = Arc::new(DashMap::new());
        let admin_user_locks = Arc::new(DashMap::new());
        let active_migrations = Arc::new(DashMap::new());
        let wasm_engine =
            Arc::new(WasmEngine::new().map_err(|e| DbError::Function(e.to_string()))?);
        let functions = Arc::new(FunctionRegistry::with_builtins());
        let globals = Arc::new(GlobalVars::new());
        globals.seed_env(&policy);

        let shamir = Self {
            dbs,
            system_store,
            admin_user_locks,
            active_migrations,
            functions,
            wasm_engine,
            globals,
            net_allowlist: Arc::new(Vec::new()),
            function_meta: DashMap::new(),
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

        // Persist to system store
        if let Err(e) = self
            .system_store
            .save_repository(db_name, &repo_name, &storage_type, path.as_deref())
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
                .save_table(db_name, &repo_name, table_name, *enable_indexes)
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
            .save_table(db_name, repo_name, table_name, enable_indexes)
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

    /// Direct table access shortcut
    pub async fn get_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<TableManager> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        db.get_table(repo_name, table_name).await
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(&actor, &ResourcePath::FunctionNamespace, Action::Create)
            .map_err(|e| DbError::Function(e.to_string()))?;
        let opts = CreateFunctionOptions {
            replace,
            ..CreateFunctionOptions::default()
        };
        self.create_function_with_opts(name, FunctionSource::Wasm(wasm), opts)
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(&actor, &ResourcePath::FunctionNamespace, Action::Create)
            .map_err(|e| DbError::Function(e.to_string()))?;
        let opts = CreateFunctionOptions {
            replace,
            ..CreateFunctionOptions::default()
        };
        self.create_function_with_opts(name, FunctionSource::Source(source), opts)
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(&actor, &ResourcePath::FunctionNamespace, Action::Create)
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
        self.system_store.save_function(name, &record).await?;

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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Delete,
        )
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(
            &actor,
            &ResourcePath::Function {
                name: from.to_string(),
            },
            Action::Write,
        )
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

        // If there was a durable record, re-key it.
        if let Some(mut rec) = old_record {
            self.system_store.remove_function(from).await?;
            rec["name"] = json!(to);
            self.system_store.save_function(to, &rec).await?;
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

    /// Build an [`FnCtx`] with globals, registry, net gateway, and the
    /// function's secret_grants from [`function_meta`].
    fn build_invoke_ctx(&self, fn_name: &str) -> FnCtx {
        let grants = self
            .function_meta(fn_name)
            .map(|m| m.secret_grants)
            .unwrap_or_default();
        FnCtx::with_globals(self.globals.clone())
            .with_registry(self.functions.clone())
            .with_net(self.build_net_gateway())
            .with_secret_grants(grants)
    }

    /// Invoke a function by name with the given parameters.
    ///
    /// Each call gets a fresh per-invocation batch context (no data sharing
    /// between calls). Use [`invoke_function_with_batch`] for multi-call
    /// batched invocation.
    pub async fn invoke_function(&self, name: &str, params: Params) -> DbResult<QueryValue> {
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .map_err(|e| DbError::Function(e.to_string()))?;
        let ctx = self.build_invoke_ctx(name);
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .map_err(|e| DbError::Function(e.to_string()))?;
        let ctx = self.build_invoke_ctx(name);
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .map_err(|e| DbError::Function(e.to_string()))?;
        let gateway = Arc::new(FacadeDbGateway {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        });
        let grants = self
            .function_meta(name)
            .map(|m| m.secret_grants)
            .unwrap_or_default();
        let ctx = FnCtx::with_globals(self.globals.clone())
            .with_registry(self.functions.clone())
            .with_db(gateway, repo.to_string())
            .with_net(self.build_net_gateway())
            .with_secret_grants(grants);
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
        let actor = Actor::System; // TODO(Shomer): from the authenticated principal on the wire path
        authorize(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Execute,
        )
        .map_err(|e| DbError::Function(e.to_string()))?;
        let gateway = Arc::new(FacadeDbGateway {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        });
        let grants = self
            .function_meta(name)
            .map(|m| m.secret_grants)
            .unwrap_or_default();
        let ctx = FnCtx::with_globals(self.globals.clone())
            .with_registry(self.functions.clone())
            .with_db(gateway, repo.to_string())
            .with_net(self.build_net_gateway())
            .with_secret_grants(grants);
        self.functions
            .invoke(name, &ctx, &FnBatch::with_context(batch.clone()), &params)
            .await
            .map_err(|e| DbError::Function(e.to_string()))
    }
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
            queries,
            return_all: false,
            return_only: Some(vec!["r".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute(&self.db_name, &req)
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
            queries,
            return_all: false,
            return_only: Some(vec!["i".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute(&self.db_name, &req)
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
            queries,
            return_all: false,
            return_only: Some(vec!["q".to_string()]),
            limits: crate::engine::query::batch::BatchLimits::default(),
        };

        let resp = self
            .shamir
            .execute(&self.db_name, &req)
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
