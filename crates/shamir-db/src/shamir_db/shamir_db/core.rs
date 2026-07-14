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
use shamir_engine::validator::{RecordValidator, ValidatorRegistry, WasmRecordValidator};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::engine::migration::MigrationCoordinator;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;

use super::super::ports::{PrincipalResolver, UserAdminPort};
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
    /// Per-`group_id` lock map used to serialise group-record
    /// read-modify-write mutations (`add_group_member_as` /
    /// `remove_group_member_as` / `set_resource_meta(Group)` /
    /// `rename_group_as` / `drop_group_as`) and close the §81 / #563
    /// last-writer-wins-on-whole-record race when two concurrent mutations
    /// target the SAME `group_id`. Keyed by `group_id` (a `u64`, unlike
    /// `admin_user_locks`'s `String` key, since groups are id-keyed not
    /// name-keyed). Entries leak by design (each unique group occupies a
    /// slot forever), but group mutations are rare so the memory cost is
    /// negligible — exactly the `admin_user_locks`/`repo_create_locks`
    /// tradeoff.
    pub(super) group_member_locks: Arc<DashMap<u64, Arc<Mutex<()>>, THasher>>,
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
    /// Serialises the whole `handle_create_db` sequence (exists-check →
    /// authorize → create) so two concurrent `CREATE DATABASE IF NOT
    /// EXISTS` (or plain `CREATE DATABASE`) calls for the SAME name can't
    /// both observe "does not exist" and both proceed to create — see
    /// task #546 (create-DB/create-repo TOCTOU). DB creation is rare, so
    /// holding this across the (bounded) await sequence mirrors
    /// `group_id_lock`'s established pattern.
    pub(super) db_create_lock: Arc<Mutex<()>>,
    /// Per-database lock serialising `handle_create_repo`'s exists-check →
    /// authorize → create sequence, keyed by `db_name`. Mirrors
    /// `admin_user_locks`'s per-key pattern: entries leak by design (each
    /// unique db occupies a slot forever), which is fine since database
    /// creation is rare and the map is bounded by the number of distinct
    /// databases ever created.
    pub(super) repo_create_locks: Arc<DashMap<String, Arc<Mutex<()>>, THasher>>,
    /// Live validator registry (compiled WASM validators loaded on open).
    pub(super) validators: Arc<ValidatorRegistry>,
    /// Base directory for durable repos, derived from the system store
    /// config. `Some(p)` when the system store is redb-backed (production),
    /// `None` for in-memory (tests). Wire-created repos default to a
    /// durable redb engine under this root; in-memory homes fall back to
    /// in-memory repos — coherent with the home's durability class.
    pub(super) data_root: Option<std::path::PathBuf>,
    /// Injected write-side user-administration port (task #559). `None`
    /// for embedded/no-directory deployments and tests; `Some` when the
    /// embedding layer (`shamir-server`) wires its directory-backed impl.
    /// When `None`, the four re-targeted user-admin handlers return
    /// `not_supported` (hard cutover off Store B).
    pub(super) user_admin_port: Option<Arc<dyn UserAdminPort>>,
    /// Injected read-only principal resolver (task #559). `None` for
    /// embedded/no-directory deployments; `Some` lets `access_tree` /
    /// `ListOp::Users` / owner-delegation scope lookup read real directory
    /// state. When `None`, names resolve to `None` (degraded but safe).
    pub(super) principal_resolver: Option<Arc<dyn PrincipalResolver>>,
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
        let group_member_locks = Arc::new(DashMap::with_hasher(THasher::default()));
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
            group_member_locks,
            active_migrations,
            functions,
            wasm_engine,
            globals,
            net_allowlist: Arc::new(Vec::new()),
            function_meta: Arc::new(DashMap::with_hasher(THasher::default())),
            group_id_lock: Arc::new(Mutex::new(())),
            db_create_lock: Arc::new(Mutex::new(())),
            repo_create_locks: Arc::new(DashMap::with_hasher(THasher::default())),
            validators,
            data_root,
            user_admin_port: None,
            principal_resolver: None,
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
            // Dispatch on artifact kind: Native rows have no WASM bytes —
            // skip materialisation. The embedder re-registers the native
            // artifact at startup; until then the function is absent and any
            // invocation fails closed ("function not found").
            if super::ArtifactKind::from_record(rec) == super::ArtifactKind::Native {
                log::info!(
                    "shamir_db::init: skipping native function '{}' — re-register at startup",
                    name
                );
                continue;
            }
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
            // Dispatch on artifact kind: Native rows have no WASM bytes —
            // skip materialisation entirely. The embedder re-registers the
            // native artifact at startup. Table-level bindings (stored on
            // each table's info-twin) are restored independently and still
            // reference this validator_id; any write will fail closed via
            // ValidatorFailure::Missing until the artifact is re-registered.
            let kind = super::ArtifactKind::from_record(rec);
            if kind == super::ArtifactKind::Native {
                log::info!(
                    "shamir_db::init: skipping native validator '{}' — re-register at startup",
                    name
                );
                continue;
            }
            if kind == super::ArtifactKind::Declarative {
                // Declarative validators live in the table catalogue and are
                // compiled by boot_compile_schemas — not from the validator
                // catalogue.
                log::info!(
                    "shamir_db::init: skipping declarative validator '{}' — \
                     compiled from table catalogue",
                    name
                );
                continue;
            }
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
                    // Wrap in WasmRecordValidator — the registry stores
                    // Arc<dyn RecordValidator> since Phase 0.
                    let rv = Arc::new(WasmRecordValidator::new(Arc::new(wf)))
                        as Arc<dyn RecordValidator>;
                    if shamir.validators.register(id, &name, rv).is_err() {
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

        // Phase A2: compile declarative schemas from the table catalogue.
        // Runs after load_validators (WASM/native) so both code and
        // declarative validators coexist in the registry. The table_records
        // were already loaded above for the repo bootstrap.
        shamir.boot_compile_schemas(&table_records).await?;

        // Phase 4 boot diagnostic: surface native catalogue rows whose
        // in-process artifact was NOT re-registered by the embedder. The boot
        // loops above skip `kind = Native` rows (no wasm bytes); the embedder
        // is expected to call `register_fn` / `register_native_validator` at
        // startup to rehydrate them. Any row still missing a live artifact at
        // this point will fail closed on first use ("not found"). Emit a
        // visible `warn!` so a silent fail-closed becomes an actionable
        // startup signal rather than a mystery runtime error.
        let unresolved = shamir.unresolved_native_artifacts_inner(&fn_records, &val_records);
        if !unresolved.is_empty() {
            log::warn!(
                "shamir_db::init: {} native artifact(s) persisted in the catalogue but \
                 NOT re-registered at startup: {:?}. Writes to tables bound to an \
                 unresolved native validator will fail closed until the embedder \
                 re-registers them.",
                unresolved.len(),
                unresolved
            );
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

    /// Per-`group_id` lock map used to serialise group-record read-modify-write
    /// mutations (add/remove member, set owner, rename, drop) and close the
    /// §81 / #563 last-writer-wins-on-whole-record race when two concurrent
    /// mutations target the same `group_id`.
    pub fn group_member_locks(&self) -> &Arc<DashMap<u64, Arc<Mutex<()>>, THasher>> {
        &self.group_member_locks
    }

    /// Global lock serialising the `handle_create_db` exists-check →
    /// authorize → create sequence (task #546 TOCTOU close).
    pub fn db_create_lock(&self) -> &Arc<Mutex<()>> {
        &self.db_create_lock
    }

    /// Per-database lock map serialising `handle_create_repo`'s
    /// exists-check → authorize → create sequence, keyed by `db_name`
    /// (task #546 TOCTOU close). Mirrors [`Self::admin_user_locks`]'s
    /// get-or-insert-then-lock usage pattern.
    pub fn repo_create_locks(&self) -> &Arc<DashMap<String, Arc<Mutex<()>>, THasher>> {
        &self.repo_create_locks
    }

    pub fn active_migrations(&self) -> &Arc<DashMap<String, Arc<MigrationCoordinator>, THasher>> {
        &self.active_migrations
    }

    /// Base directory for durable repos. `Some` when the system store
    /// is redb-backed (production), `None` for in-memory (tests).
    pub fn data_root(&self) -> Option<&std::path::Path> {
        self.data_root.as_deref()
    }

    /// The injected write-side user-administration port, if any (task #559).
    /// `None` for embedded/no-directory deployments — the four re-targeted
    /// handlers then return `not_supported`.
    pub fn user_admin_port(&self) -> Option<&Arc<dyn UserAdminPort>> {
        self.user_admin_port.as_ref()
    }

    /// The injected read-only principal resolver, if any (task #559). `None`
    /// for embedded/no-directory deployments — names then resolve to `None`.
    pub fn principal_resolver(&self) -> Option<&Arc<dyn PrincipalResolver>> {
        self.principal_resolver.as_ref()
    }

    /// Builder: install a write-side user-admin port. Mirrors the cheap-clone
    /// `Arc`-backed field pattern — returns `Self` so callers can chain.
    /// Idempotent overwrites are allowed (last-writer-wins); production wiring
    /// calls this exactly once at boot.
    pub fn with_user_admin_port(mut self, port: Arc<dyn UserAdminPort>) -> Self {
        self.user_admin_port = Some(port);
        self
    }

    /// Builder: install a read-only principal resolver. Mirrors
    /// [`Self::with_user_admin_port`].
    pub fn with_principal_resolver(mut self, resolver: Arc<dyn PrincipalResolver>) -> Self {
        self.principal_resolver = Some(resolver);
        self
    }

    /// Names of `kind = Native` catalogue entries (functions + validators)
    /// whose in-process artifact is NOT live in the registry.
    ///
    /// The boot loop skips `kind = Native` rows (they have no WASM bytes);
    /// the embedding application is expected to re-register each native
    /// artifact at startup via [`ShamirDb::register_fn`] /
    /// [`ShamirDb::register_native_validator`]. Any artifact still missing
    /// after startup will fail closed on first use ("not found"). This
    /// accessor turns that silent fail-closed into an actionable signal —
    /// a monitoring layer can poll it, and the boot path emits a `warn!`
    /// listing the unresolved names.
    ///
    /// Returns a sorted, deduplicated `Vec<String>` of names.
    pub async fn unresolved_native_artifacts(&self) -> DbResult<Vec<String>> {
        let fn_records = self.system_store.load_functions().await?;
        let val_records = self.system_store.load_validators().await?;
        Ok(self.unresolved_native_artifacts_inner(&fn_records, &val_records))
    }

    /// Inner join: given the already-loaded function and validator catalogue
    /// records, return the names of `kind = Native` rows whose artifact is
    /// absent from the live registry. Used both by the boot diagnostic
    /// (which already has the records in hand) and by the public
    /// [`Self::unresolved_native_artifacts`] accessor.
    fn unresolved_native_artifacts_inner(
        &self,
        fn_records: &[QueryValue],
        val_records: &[QueryValue],
    ) -> Vec<String> {
        let mut out = Vec::new();
        for rec in fn_records {
            if super::ArtifactKind::from_record(rec) != super::ArtifactKind::Native {
                continue;
            }
            if let Some(name) = rec.get("name").and_then(|v| v.as_str()) {
                if !self.functions.contains(name) {
                    out.push(name.to_string());
                }
            }
        }
        for rec in val_records {
            if super::ArtifactKind::from_record(rec) != super::ArtifactKind::Native {
                continue;
            }
            if let Some(name) = rec.get("name").and_then(|v| v.as_str()) {
                if self.validators.id_for_name(name).is_none() {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        out.dedup();
        out
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

    /// Access the per-DB user scalar layer for `db_name`.
    ///
    /// Returns `None` if the database does not exist. The returned
    /// `Arc<UserScalarLayer>` can be used to register custom native
    /// scalar functions that become available in WHERE filters and
    /// (when marked `.trusted_pure()`) in functional indexes.
    pub fn scalars(
        &self,
        db_name: &str,
    ) -> Option<std::sync::Arc<shamir_funclib::scalar_resolver::UserScalarLayer>> {
        self.get_db(db_name)
            .map(|db| std::sync::Arc::clone(db.scalars()))
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
            #[cfg(feature = "fjall")]
            "fjall" => path.map(BoxRepoFactory::fjall),
            _ => None,
        }
    }

    pub(super) fn extract_storage_type(factory: &BoxRepoFactory) -> String {
        match factory {
            BoxRepoFactory::InMemory(_) => "in_memory",
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
            .with_net(self.build_net_gateway(fn_name))
            .with_secret_grants(grants)
            .with_actor(actor)
    }

    /// Build a [`NetGateway`] scoped to `fn_name`'s effective egress reach:
    /// the function's `net_grants` INTERSECTED with the DB-wide
    /// `net_allowlist` (a function can never exceed the DB's own ceiling).
    ///
    /// Always returns a gateway so that allowlist-denial is a catchable
    /// runtime error, not a "no net gateway" trap.
    ///
    /// # Empty `net_grants` semantics (deliberate, documented choice)
    ///
    /// Unlike `secret_grants` (empty = no secrets granted — the RESTRICTIVE
    /// default), an EMPTY `net_grants` here means "no function-level
    /// restriction": the function gets the FULL `net_allowlist`, exactly as
    /// every function did before per-function `net_grants` existed. This is
    /// intentionally inconsistent with `secret_grants`'s own precedent,
    /// chosen for backward compatibility — flipping the default to
    /// restrictive (empty = no egress) would silently lock every
    /// already-deployed function that calls `ctx.http_fetch()` with no
    /// `net_grants` ever set out of egress it currently has, with no
    /// migration path (see `docs/dev-artifacts/prompts/audit/65-fix-544-*.md` and
    /// `FunctionMeta::net_grants`'s doc comment for the full tradeoff). A
    /// non-empty `net_grants` list narrows the function to LESS than the DB
    /// default — there is no way to grant a function MORE than the DB
    /// ceiling.
    ///
    /// # Intersection is literal-string, not pattern-aware (known limitation)
    ///
    /// The intersection below matches `net_grants` entries against
    /// `net_allowlist` entries by EXACT STRING equality, not by resolving
    /// either side against an actual host the way `check_host_allowed`'s
    /// `glob_matches` does at fetch time. Concretely: if the DB allowlist
    /// has the wildcard pattern `"*.example.com"` and a function's
    /// `net_grants` names a concrete host like `"api.example.com"`, the two
    /// strings don't match, so the intersection is EMPTY and the function is
    /// denied ALL egress — even though `api.example.com` is both covered by
    /// the DB's wildcard and clearly what the grant intended. This fails
    /// CLOSED (never grants more than intended), so it's not a security
    /// hole, but it means an operator narrowing a wildcard DB allowlist to a
    /// concrete per-function host must currently spell `net_grants` with
    /// the exact same pattern string as it appears in `net_allowlist` — a
    /// real usability gap, not fixed by this task (see task #544's closing
    /// report / a follow-up would need to intersect via pattern-subsumption
    /// rather than string equality).
    pub(super) fn build_net_gateway(&self, fn_name: &str) -> Arc<dyn NetGateway> {
        let net_grants = self.function_meta(fn_name).map(|m| m.net_grants);
        let effective = match net_grants {
            // No catalogue entry (builtin) or an explicitly empty grant
            // list: fall back to the full DB-wide allowlist (see doc above).
            None => self.net_allowlist.to_vec(),
            Some(grants) if grants.is_empty() => self.net_allowlist.to_vec(),
            // Non-empty grants: intersect with the DB-wide allowlist so the
            // function is scoped to the OVERLAP, never beyond the DB ceiling.
            Some(grants) => self
                .net_allowlist
                .iter()
                .filter(|host| grants.contains(host))
                .cloned()
                .collect(),
        };
        Arc::new(super::super::curl_gateway::CurlNetGateway::new(effective))
    }
}
