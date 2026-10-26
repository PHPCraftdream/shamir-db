use shamir_types::types::value::QueryValue;

use crate::access::{Actor, Mode, ResourceMeta, ResourcePath};
use crate::{DbError, DbResult};

use super::super::ShamirDb;

/// Second-layer guard: decode `ResourceMeta` only from a `Map` record; a
/// non-map row reaching this point (catalogue corruption that slipped past
/// the read-level guards) is an Internal error, never default-open meta.
fn checked_meta(record: &QueryValue, kind: &str) -> DbResult<ResourceMeta> {
    match record {
        QueryValue::Map(_) => Ok(ResourceMeta::from_record(record)),
        _ => Err(DbError::Internal(format!(
            "corrupt {kind} catalogue record: not a map"
        ))),
    }
}

impl ShamirDb {
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
    ///
    /// # Fail-closed guarantee
    ///
    /// A real catalogue-read error (`Err`) is distinguished from a
    /// legitimate "record genuinely absent" (`Ok(None)`) and propagated as
    /// `Err` — it must NEVER collapse into [`ResourceMeta::default`] (which
    /// is [`ResourceMeta::open`], owner=System, mode `0o777`). Doing so
    /// would turn a transient/structural storage failure into a full auth
    /// bypass on the affected resource. The sole intentional `Ok(None)` →
    /// `open()` fallback is `FunctionFolder` (#118, implicit folders that
    /// were never explicitly `CREATE`d) — see that branch's comment.
    pub async fn resource_meta(&self, path: &ResourcePath) -> DbResult<ResourceMeta> {
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
            ResourcePath::Database { db } => match self.system_store.load_database(db).await {
                Ok(Some(r)) => checked_meta(&r, "database"),
                Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
                Err(e) => {
                    log::warn!("resource_meta: failed to load database '{}' meta: {e}", db);
                    Err(e)
                }
            },
            ResourcePath::Store { db, store } => {
                match self.system_store.load_repository(db, store).await {
                    Ok(Some(r)) => checked_meta(&r, "repository"),
                    Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
                    Err(e) => {
                        log::warn!(
                            "resource_meta: failed to load store '{}/{}' meta: {e}",
                            db,
                            store
                        );
                        Err(e)
                    }
                }
            }
            ResourcePath::Table { db, store, table } => {
                match self.system_store.load_table_record(db, store, table).await {
                    Ok(Some(r)) => checked_meta(&r, "table"),
                    Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
                    Err(e) => {
                        log::warn!(
                            "resource_meta: failed to load table '{}/{}/{}' meta: {e}",
                            db,
                            store,
                            table
                        );
                        Err(e)
                    }
                }
            }
            ResourcePath::Function { name } => match self.system_store.load_function(name).await {
                Ok(Some(r)) => checked_meta(&r, "function"),
                Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
                Err(e) => {
                    log::warn!(
                        "resource_meta: failed to load function '{}' meta: {e}",
                        name
                    );
                    Err(e)
                }
            },
            ResourcePath::FunctionFolder { path } => {
                let path_key = path.join("/");
                match self.system_store.load_function_folder(&path_key).await {
                    Ok(Some(r)) => Ok(ResourceMeta::from_record(&r)),
                    // Read persisted meta if the folder was explicitly created;
                    // fall back to open so functions with slash-names under
                    // implicit (never-created) folders are not denied (#118).
                    // This is a genuine "never created" case, NOT an error —
                    // do not change this arm.
                    Ok(None) => Ok(ResourceMeta::default()),
                    Err(e) => {
                        log::warn!(
                            "resource_meta: failed to load function folder '{}' meta: {e}",
                            path_key
                        );
                        Err(e)
                    }
                }
            }
            ResourcePath::FunctionNamespace => {
                match self.system_store.load_setting("fn_namespace_meta").await {
                    Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
                    Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
                    Err(e) => {
                        log::warn!("resource_meta: failed to load function namespace meta: {e}");
                        Err(e)
                    }
                }
            }
            // Root — full persisted meta, settings key "root_meta". Mirrors
            // the FunctionNamespace arm above. Default (absent key) is
            // System-owned, `0o751` — Other keeps the EXECUTE (traverse) bit
            // but loses READ, so top-level database ENUMERATION (`List`,
            // Read-mapped) is closed to non-owner/non-System actors by
            // default (task #615/#620), while traversal into a nested
            // resource an actor separately holds rights on (Database/Table/
            // Function/User/Group) still works — `authorize_access`'s
            // ancestor-traversal check requires Execute on every ancestor
            // INCLUDING Root for any nested path, so dropping Root's
            // Execute bit too (0o750) would collaterally deny access to
            // every nested resource for every non-owner actor, not just
            // Root listing (discovered via full-suite regression while
            // implementing task #620 — do not repeat that mistake). "Write
            // to root" (creating a top-level database) was already
            // narrowed to owner-only (task #552). An operator who wants
            // open listing can explicitly `chmod` via `set_resource_meta`.
            ResourcePath::Root => match self.system_store.load_setting("root_meta").await {
                Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
                Ok(None) => Ok(ResourceMeta {
                    owner: Actor::System,
                    group: None,
                    mode: 0o751,
                }),
                Err(e) => {
                    log::warn!("resource_meta: failed to load root meta: {e}");
                    Err(e)
                }
            },
            // WasmCompiler — persisted meta, settings key "wasm_compiler_meta".
            // Absent key -> System-owned, 0o755 (task #607 — user's explicit
            // directive: POSIX-style mode gate, not an OS sandbox). Distinct
            // from Root's default (0o751, task #615/#620) — WasmCompiler's
            // open-Other default is intentional and out of scope here.
            ResourcePath::WasmCompiler => {
                match self.system_store.load_setting("wasm_compiler_meta").await {
                    Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
                    Ok(None) => Ok(ResourceMeta {
                        owner: Actor::System,
                        group: None,
                        mode: 0o755,
                    }),
                    Err(e) => {
                        log::warn!("resource_meta: failed to load wasm compiler meta: {e}");
                        Err(e)
                    }
                }
            }
            // User — a FIXED, computed 3-tier rule, never persisted.
            // Task #559: the owner is the REAL principal64 resolved from
            // the directory via the injected PrincipalResolver. With a
            // resolver installed, `resolve_by_name` yields the user's true
            // minted id, so the user owns their own path (System bypasses;
            // the user themselves gets Read+Manage; everyone else denied
            // via the `0o750` mode). With NO resolver installed, name→id
            // resolution is unavailable — the design doc's "absent resolver
            // → names resolve to null" framing means we DEGRADE to a
            // System-owned open-ish meta (no synthetic hash-based owner is
            // synthesised — that interim bridge is retired here, NOT kept
            // alive). Only System can then manage a User resource in the
            // no-directory case; regular access-control on opaque ids is
            // otherwise unchanged.
            ResourcePath::User { name } => {
                let owner = self
                    .principal_resolver()
                    .and_then(|r| r.resolve_by_name(name))
                    .map(|info| Actor::User(info.principal64))
                    .unwrap_or(Actor::System);
                Ok(ResourceMeta {
                    owner,
                    group: None,
                    mode: 0o750,
                })
            }
            // Group — persisted `owner` on the existing group record,
            // computed mode. `group: Some(group_id)` makes a group's own
            // members a real permission class (roster-read for members).
            // Confirmed not-found falls back to `ResourceMeta::open()`
            // (mirrors the FunctionFolder "never created" convention above —
            // a nonexistent group is not an error case for meta resolution).
            // Any OTHER error from `resolve_group_id` (e.g. a real storage/
            // catalogue-read failure) must propagate as `Err`, not collapse
            // into a fail-open default-open ResourceMeta.
            ResourcePath::Group { name } => {
                let group_ref = crate::query::admin::GroupRef::Name { name: name.clone() };
                let group_id = match self.resolve_group_id(&group_ref).await {
                    Ok(id) => id,
                    Err(DbError::NotFound(_)) => return Ok(ResourceMeta::open()),
                    Err(e) => return Err(e),
                };
                match self.system_store.load_group(group_id).await {
                    Ok(Some(rec)) => Ok(ResourceMeta {
                        owner: ResourceMeta::owner_field(&rec).unwrap_or(Actor::System),
                        group: Some(group_id),
                        mode: 0o750,
                    }),
                    Ok(None) => Ok(ResourceMeta::open()),
                    Err(e) => {
                        log::warn!("resource_meta: failed to load group '{}' meta: {e}", name);
                        Err(e)
                    }
                }
            }
            // Record/Index already resolved to Table above; if something
            // slips through, return open.
            ResourcePath::Record { .. } | ResourcePath::Index { .. } => Ok(ResourceMeta::open()),
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
            ResourcePath::FunctionFolder { path: segments } => {
                let path_key = segments.join("/");
                let rec = self
                    .system_store
                    .load_function_folder(&path_key)
                    .await?
                    .ok_or_else(|| {
                        DbError::NotFound(format!("function folder '{}' not found", path_key))
                    })?;
                let mut rec = rec;
                meta.inject_into(&mut rec);
                self.system_store
                    .save_function_folder_meta(&path_key, &rec)
                    .await
            }
            ResourcePath::FunctionNamespace => {
                let mut m = shamir_types::types::common::new_map();
                m.insert(
                    "key".to_string(),
                    QueryValue::Str("fn_namespace_meta".to_string()),
                );
                let mut rec = QueryValue::Map(m);
                meta.inject_into(&mut rec);
                self.system_store
                    .save_setting("fn_namespace_meta", &rec)
                    .await
            }
            // Root — mirrors the FunctionNamespace write arm above, keyed
            // "root_meta" (task #552).
            //
            // Guardrail: reject a chmod/chown that would leave Root with a
            // non-System owner AND no owner-Execute bit. `Actor::System`
            // bypasses `permits()` unconditionally and can always recover;
            // a non-System owner without owner-Execute would have no way
            // back in. This checks the NEW meta being written (`meta.owner`
            // / `meta.mode`), not the meta being replaced — a review of an
            // earlier revision of this guard found that checking the OLD
            // (`current`) owner instead let the lockout through via two
            // ordinary, separately-authorized calls: (1) while Root is
            // still System-owned, chmod clears owner-Execute (allowed —
            // System doesn't need the bit); (2) a SEPARATE chown then hands
            // Root to a non-System actor without touching mode, inheriting
            // the already-cleared bit — the old check only looked at
            // `current.owner` (still System at the time of call 2) and
            // never fired. Checking the actor/bits actually being persisted
            // closes both the single-call and the two-call sequence.
            ResourcePath::Root => {
                if meta.owner != Actor::System
                    && !Mode::is_set(
                        meta.mode,
                        shamir_types::access::PermClass::Owner,
                        shamir_types::access::Perm::Execute,
                    )
                {
                    return Err(DbError::Validation(format!(
                        "chmod/chown would leave Root owned by non-System owner {} without \
                         owner-Execute (unrecoverable self-lockout — System always bypasses \
                         permits() and can always recover, but a non-System owner cannot)",
                        meta.owner
                    )));
                }
                let mut m = shamir_types::types::common::new_map();
                m.insert("key".to_string(), QueryValue::Str("root_meta".to_string()));
                let mut rec = QueryValue::Map(m);
                meta.inject_into(&mut rec);
                self.system_store.save_setting("root_meta", &rec).await
            }
            // WasmCompiler — mirrors the FunctionNamespace write arm above,
            // keyed "wasm_compiler_meta" (task #607). No Root-style
            // self-lockout guardrail: that guard is specific to Root's
            // "always need a way back in via System bypass" concern, not
            // applicable here — a locked-down WasmCompiler simply means
            // Rust-source compilation is denied, recoverable via System.
            ResourcePath::WasmCompiler => {
                let mut m = shamir_types::types::common::new_map();
                m.insert(
                    "key".to_string(),
                    QueryValue::Str("wasm_compiler_meta".to_string()),
                );
                let mut rec = QueryValue::Map(m);
                meta.inject_into(&mut rec);
                self.system_store
                    .save_setting("wasm_compiler_meta", &rec)
                    .await
            }
            // Group — mirrors the FunctionNamespace write arm's shape, but
            // only `owner` is settable (per the design doc, group `mode`
            // stays fixed/computed at 0o750 — no demonstrated need for
            // per-group chmod yet).
            ResourcePath::Group { name } => {
                let group_ref = crate::query::admin::GroupRef::Name { name: name.clone() };
                let group_id = self.resolve_group_id(&group_ref).await?;
                // §81 / #563: hold the per-group_id lock across the whole
                // load→set-owner→save_group RMW so a concurrent
                // add/remove-member/rename/drop on the SAME group_id can't
                // lose this chown (or get its own change lost by this chown's
                // stale-read overwrite).
                let group_lock = self
                    .group_member_locks()
                    .entry(group_id)
                    .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
                    .clone();
                let _group_guard = group_lock.lock().await;
                self.system_store
                    .set_group_owner(group_id, meta.owner.to_owner_id())
                    .await
            }
            // User, Record, Index — not directly settable via catalogue in
            // this slice. User meta is a fixed, computed rule (never
            // persisted — see the `resource_meta` arm above); Record/Index
            // inherit from their Table.
            _ => Err(DbError::NotFound(format!(
                "resource path '{}' does not support set_resource_meta in this slice",
                path
            ))),
        }
    }
}
