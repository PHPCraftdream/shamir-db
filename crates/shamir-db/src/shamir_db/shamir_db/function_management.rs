use base64::Engine;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::{DbError, DbResult};
use shamir_engine::function::{
    compile_rust_source, BatchContext, CreateFunctionOptions, FnAdapter, FnBatch, FnCtx,
    FunctionError, FunctionMeta, FunctionRegistry, GlobalVars, Params, ShamirFunction,
    WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;

use super::{ArtifactKind, FunctionSource, ShamirDb};

impl ShamirDb {
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

    /// Register a native (in-process) procedural function by closure.
    ///
    /// This is the MVP in-memory path: the closure is wrapped in
    /// [`FnAdapter`] and inserted into the live [`FunctionRegistry`]. No
    /// catalogue row is written — the registration is lost on restart. For a
    /// persisted native function, write a `kind = Native` row manually (see
    /// Deliverable 3 for the validator analogue).
    ///
    /// `replace` controls overwrite semantics: `false` errors on name
    /// collision, `true` silently overwrites.
    pub fn register_fn<F, Fut>(&self, name: &str, replace: bool, f: F) -> DbResult<()>
    where
        F: Fn(&FnCtx, &FnBatch, &Params) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<QueryValue, FunctionError>> + Send + 'static,
    {
        let adapter = Arc::new(FnAdapter(f)) as Arc<dyn ShamirFunction>;
        if replace {
            self.functions.replace(name, adapter);
            Ok(())
        } else {
            self.functions
                .register(name, adapter)
                .map_err(|e| DbError::Function(e.to_string()))
        }
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
        // `secret_grants` names OS-seeded process environment variables
        // (`GlobalVars::seed_env`) — a resource class the creator has NO
        // defined rights over at all (there is no "which secrets can this
        // actor grant" concept anywhere in this codebase yet). Without this
        // gate, any actor holding bare `Create` on `FunctionNamespace` could
        // request `secret_grants: ["ADMIN_DB_PASSWORD"]` on their own new
        // function and exfiltrate host secrets by calling it. Deliberately
        // admin-only (`Manage(Root)`) pending a real secrets-ACL — do not
        // invent a finer-grained check that doesn't actually exist (task
        // #554, per the signed-off design in
        // `docs/design/root-user-group-dac-posture-550-decision.md` §3).
        if !opts.secret_grants.is_empty() {
            self.authorize_access(&actor, &ResourcePath::Root, Action::Manage)
                .await
                .map_err(|e| DbError::Function(e.to_string()))?;
        }
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
        let mut hasher = rustc_hash::FxHasher::default();
        wasm.hash(&mut hasher);
        let wasm_hash = format!("{:016x}", hasher.finish());

        if !opts.replace && self.functions.contains(name) {
            return Err(DbError::Function(format!(
                "function '{}' already exists",
                name
            )));
        }

        let meta = FunctionMeta::new(
            opts.visibility,
            opts.security,
            opts.secret_grants.clone(),
            opts.net_grants.clone(),
        );

        let version = 1u64;
        let mut m = shamir_types::types::common::new_map();
        m.insert(
            "name".to_string(),
            shamir_types::types::value::QueryValue::Str(name.to_string()),
        );
        m.insert(
            "wasm_b64".to_string(),
            shamir_types::types::value::QueryValue::Str(wasm_b64),
        );
        m.insert(
            "wasm_hash".to_string(),
            shamir_types::types::value::QueryValue::Str(wasm_hash),
        );
        m.insert(
            "lang".to_string(),
            shamir_types::types::value::QueryValue::Str(lang_tag.to_string()),
        );
        m.insert(
            "source".to_string(),
            match source_str {
                Some(s) => shamir_types::types::value::QueryValue::Str(s),
                None => shamir_types::types::value::QueryValue::Null,
            },
        );
        m.insert(
            "version".to_string(),
            shamir_types::types::value::QueryValue::Int(version as i64),
        );
        // Phase 0 parity plumbing: tag the artifact origin. Today every row
        // persisted here is WASM; later phases introduce `Native` rows. The
        // field defaults to `Wasm` on read for any pre-existing row without
        // it (see `ArtifactKind::from_record`), so this is a no-op for
        // behaviour — purely additive catalogue metadata.
        m.insert(
            super::KIND_FIELD.to_string(),
            ArtifactKind::Wasm.as_query_value(),
        );
        let mut record = shamir_types::types::value::QueryValue::Map(m);
        meta.inject_into(&mut record);
        self.system_store
            .save_function(name, &record, &ResourceMeta::owned_enforced(actor.clone()))
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
            .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(from))
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
            if let shamir_types::types::value::QueryValue::Map(ref mut map) = rec {
                map.insert(
                    "name".to_string(),
                    shamir_types::types::value::QueryValue::Str(to.to_string()),
                );
            }
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

    /// List all registered functions as `(name, kind)` pairs (sorted by name).
    ///
    /// The `kind` is resolved per-name against the persisted catalogue record:
    /// a function whose catalogue row carries `kind = Native` reports
    /// [`ArtifactKind::Native`]; everything else (including builtins, which
    /// have no catalogue row) reports [`ArtifactKind::Wasm`]. Builtins default
    /// to `Wasm` because they are in-process Rust types that bypass the
    /// catalogue — the historical default for any row without an explicit
    /// `kind` field (see [`ArtifactKind::from_record`]).
    ///
    /// This is the introspection accessor behind the `list functions` wire
    /// surface; the catalogue is the source of truth for `kind`.
    pub async fn list_functions_with_kind(&self) -> DbResult<Vec<(String, ArtifactKind)>> {
        let mut names = self.functions.list();
        names.sort();
        // One targeted lookup per name — O(N) in the function count, but each
        // lookup is O(1) against the name-keyed catalogue table. A full scan
        // would also be correct but does unnecessary work for large catalogues.
        let mut out = Vec::with_capacity(names.len());
        for name in &names {
            let kind = match self.system_store.load_function(name).await? {
                Some(rec) => ArtifactKind::from_record(&rec),
                None => ArtifactKind::Wasm, // builtin / no catalogue row
            };
            out.push((name.clone(), kind));
        }
        Ok(out)
    }

    /// Look up a function's in-memory metadata.
    ///
    /// Returns `None` for builtins (they have no catalogue entry).
    pub fn function_meta(&self, name: &str) -> Option<FunctionMeta> {
        self.function_meta.get(name).map(|r| r.value().clone())
    }

    // ========================================================================
    // Function folder lifecycle API (#118)
    // ========================================================================

    /// Create a function folder with `mkdir -p` semantics.
    ///
    /// For each prefix of `path_segments` that does not already have a
    /// persisted record, a new folder record is created with `actor` as
    /// owner. Existing folders are not overwritten.
    ///
    /// Returns the list of newly created path keys (slash-joined).
    pub async fn create_function_folder_as(
        &self,
        path_segments: &[String],
        actor: Actor,
    ) -> DbResult<Vec<String>> {
        let mut created = Vec::new();
        for i in 1..=path_segments.len() {
            let prefix = &path_segments[..i];
            let path_key = prefix.join("/");
            // Check if already exists.
            let existing = self.system_store.load_function_folder(&path_key).await?;
            if existing.is_some() {
                continue;
            }
            let segments_list: Vec<shamir_types::types::value::QueryValue> = prefix
                .iter()
                .map(|s| shamir_types::types::value::QueryValue::Str(s.clone()))
                .collect();
            let mut m = shamir_types::types::common::new_map();
            m.insert(
                "path".to_string(),
                shamir_types::types::value::QueryValue::Str(path_key.clone()),
            );
            m.insert(
                "segments".to_string(),
                shamir_types::types::value::QueryValue::List(segments_list),
            );
            let record = shamir_types::types::value::QueryValue::Map(m);
            self.system_store
                .save_function_folder(
                    &path_key,
                    &record,
                    &ResourceMeta::owned_enforced(actor.clone()),
                )
                .await?;
            created.push(path_key);
        }
        Ok(created)
    }

    /// List all persisted function folder path keys.
    pub async fn list_function_folders(&self) -> DbResult<Vec<String>> {
        let records = self.system_store.load_function_folders().await?;
        let mut paths: Vec<String> = records
            .iter()
            .filter_map(|r| {
                r.get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        paths.sort();
        Ok(paths)
    }

    /// Rename a function folder subtree by rekeying all records whose path is
    /// `from` or prefixed by `from + "/"`, preserving each record's
    /// `ResourceMeta` (owner / group / mode). Does NOT touch stored functions
    /// (they live in a flat namespace keyed by name, not by folder path).
    ///
    /// Guards: `from` must exist; `to` must be free (no record at `to` itself
    /// nor any descendant of `to`). The migration is collected before any
    /// mutation, then applied remove-old / save-new, so no partial state is
    /// left if a write fails mid-way (the destination guard already guarantees
    /// the target subtree is empty, so writes cannot collide with surviving
    /// old keys).
    pub async fn rename_function_folder_as(
        &self,
        from: &[String],
        to: &[String],
        actor: Actor,
    ) -> DbResult<()> {
        self.authorize_access(
            &actor,
            &ResourcePath::FunctionFolder {
                path: from.to_vec(),
            },
            Action::Write,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;

        let from_key = from.join("/");
        let to_key = to.join("/");

        // Guard: source must exist.
        if self
            .system_store
            .load_function_folder(&from_key)
            .await?
            .is_none()
        {
            return Err(DbError::NotFound(format!(
                "function folder '{}' not found",
                from_key
            )));
        }
        // Guard: destination must be free — no record at `to` itself nor any
        // descendant (`to` subtree empty).
        if self
            .system_store
            .load_function_folder(&to_key)
            .await?
            .is_some()
        {
            return Err(DbError::KeyExists(format!(
                "function folder '{}' already exists",
                to_key
            )));
        }
        let all = self.system_store.load_function_folders().await?;
        for rec in &all {
            if let Some(p) = rec.get("path").and_then(|v| v.as_str()) {
                if p == to_key || p.starts_with(&format!("{}/", to_key)) {
                    return Err(DbError::KeyExists(format!(
                        "function folder '{}' already exists",
                        p
                    )));
                }
            }
        }

        // Collect the migrations up-front: (old_key, new_key, new_record, meta).
        let prefix_with_slash = format!("{}/", from_key);
        let mut migrations: Vec<(String, String, QueryValue, ResourceMeta)> = Vec::new();
        for rec in all {
            let path = match rec.get("path").and_then(|v| v.as_str()) {
                Some(p) => p.to_string(),
                None => continue,
            };
            let is_self = path == from_key;
            let is_descendant = path.starts_with(&prefix_with_slash);
            if !is_self && !is_descendant {
                continue;
            }
            // Compute the new key by replacing the `from_key` prefix.
            let new_key = if is_self {
                to_key.clone()
            } else {
                // descendant: strip `from_key + "/"`, prepend `to_key + "/"`.
                format!("{}/{}", to_key, &path[prefix_with_slash.len()..])
            };
            let mut new_rec = rec.clone();
            if let QueryValue::Map(ref mut m) = new_rec {
                m.insert("path".to_string(), QueryValue::Str(new_key.clone()));
                // Reconstruct the segments vector for the new key.
                let new_segments: Vec<QueryValue> = new_key
                    .split('/')
                    .map(|s| QueryValue::Str(s.to_string()))
                    .collect();
                m.insert("segments".to_string(), QueryValue::List(new_segments));
            }
            let meta = ResourceMeta::from_record(&new_rec);
            migrations.push((path, new_key, new_rec, meta));
        }

        // Apply: remove old keys first, then save new records with preserved
        // meta. The destination guard already guaranteed no key collisions.
        for (old_key, _, _, _) in &migrations {
            self.system_store.remove_function_folder(old_key).await?;
        }
        for (_, new_key, new_rec, meta) in &migrations {
            self.system_store
                .save_function_folder(new_key, new_rec, meta)
                .await?;
        }

        Ok(())
    }

    /// Thin wrapper around [`rename_function_folder_as`] with `Actor::System`.
    pub async fn rename_function_folder(&self, from: &[String], to: &[String]) -> DbResult<()> {
        self.rename_function_folder_as(from, to, Actor::System)
            .await
    }

    // ========================================================================
    // Function invocation
    // ========================================================================

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
        let gateway = Arc::new(super::db_gateway::FacadeDbGateway {
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
            .with_net(self.build_net_gateway(name))
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
        let gateway = Arc::new(super::db_gateway::FacadeDbGateway {
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
            .with_net(self.build_net_gateway(name))
            .with_secret_grants(grants)
            .with_actor(actor);
        self.functions
            .invoke(name, &ctx, &FnBatch::with_context(batch.clone()), &params)
            .await
            .map_err(|e| DbError::Function(e.to_string()))
    }
}
