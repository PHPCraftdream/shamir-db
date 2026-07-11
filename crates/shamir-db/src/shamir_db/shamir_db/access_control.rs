use shamir_collections::TFxMap;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
// CRIT-6 part B / audit #440 — `Security`/`FunctionMeta` carry the
// declared SECURITY INVOKER / DEFINER semantics; previously dead weight,
// now consulted by `effective_fn_actor`. Re-exported by `shamir_engine`
// (which aliases `shamir_wasm_host as function`).
use shamir_engine::function::{FunctionMeta, Security};

use crate::access::{
    authorize, permits, principal_id, AccessError, Action, Actor, Mode, ResourceMeta, ResourcePath,
    OWNER_SYSTEM,
};
use crate::{DbError, DbResult};

use super::ShamirDb;

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
                Ok(Some(r)) => Ok(ResourceMeta::from_record(&r)),
                Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
                Err(e) => {
                    log::warn!("resource_meta: failed to load database '{}' meta: {e}", db);
                    Err(e)
                }
            },
            ResourcePath::Store { db, store } => {
                match self.system_store.load_repository(db, store).await {
                    Ok(Some(r)) => Ok(ResourceMeta::from_record(&r)),
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
                    Ok(Some(r)) => Ok(ResourceMeta::from_record(&r)),
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
                Ok(Some(r)) => Ok(ResourceMeta::from_record(&r)),
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
            ResourcePath::Root | ResourcePath::User { .. } | ResourcePath::Group { .. } => {
                Ok(ResourceMeta::open())
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
                    .filter_map(|g| g.get("group_id").and_then(|v| v.as_u64()))
                    .max();
                max.map_or(1, |m| m + 1)
            }
        };
        let group_id = current;

        // Durability: bump the counter BEFORE writing the group, so a crash
        // in between only LEAKS an id (monotonic) — it can never overwrite the
        // next group on restart.
        self.system_store
            .save_setting("next_group_id", &QueryValue::Int((current + 1) as i64))
            .await?;
        self.system_store.save_group(group_id, name, &[]).await?;
        Ok(group_id)
    }

    /// Drop a group by id.
    pub async fn drop_group(&self, group_id: u64) -> DbResult<()> {
        self.system_store.remove_group(group_id).await
    }

    /// Rename an existing group.
    ///
    /// Groups are id-keyed: members and resource references store the
    /// (immutable) `group_id`, never the name. Renaming therefore rewrites
    /// only the display name under the existing `group_id` — no reference
    /// rekey is required.
    ///
    /// - Resolves the source [`GroupRef`] to a `group_id` (NotFound if absent).
    /// - Guards name uniqueness: if some *other* group already holds `to`,
    ///   returns [`DbError::KeyExists`]. Renaming a group to its own current
    ///   name is a tolerated no-op.
    /// - Preserves membership by reloading it before the overwrite.
    pub async fn rename_group(
        &self,
        group_ref: &crate::query::admin::GroupRef,
        to: &str,
    ) -> DbResult<()> {
        let gid = self.resolve_group_id(group_ref).await?;

        // Uniqueness guard: reject if a *different* group already owns `to`.
        let groups = self.system_store.load_groups().await?;
        let conflict = groups.iter().any(|g| {
            g.get("name").and_then(|v| v.as_str()) == Some(to)
                && g.get("group_id").and_then(|v| v.as_u64()) != Some(gid)
        });
        if conflict {
            return Err(DbError::KeyExists(format!("group '{}' already exists", to)));
        }

        // Preserve membership across the name rewrite.
        let members = self.group_members(gid).await?;
        self.system_store.save_group(gid, to, &members).await?;
        Ok(())
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
                    .find(|g| g.get("name").and_then(|v| v.as_str()) == Some(name.as_str()))
                    .and_then(|g| g.get("group_id").and_then(|v| v.as_u64()))
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
                r.get("members")
                    .and_then(|v| v.as_array())
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
            let anc_meta = match self.resource_meta(&anc).await {
                Ok(m) => m,
                Err(e) => {
                    // Fail-closed: a real catalogue-read error on an
                    // ancestor must deny unconditionally — never fall
                    // through to a default-open `permits` check.
                    log::warn!(
                        "authorize_access: resource_meta failed for ancestor '{}' \
                         (actor={actor}, action={}): {e}",
                        anc,
                        Action::Execute
                    );
                    return Err(AccessError {
                        actor: actor.clone(),
                        path: anc.to_string(),
                        action: Action::Execute,
                    });
                }
            };
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
        let meta = match self.resource_meta(path).await {
            Ok(m) => m,
            Err(e) => {
                // Fail-closed: a real catalogue-read error on the target
                // must deny unconditionally — never fall through to a
                // default-open `permits` check.
                log::warn!(
                    "authorize_access: resource_meta failed for target '{}' \
                     (actor={actor}, action={action}): {e}",
                    path
                );
                return Err(AccessError {
                    actor: actor.clone(),
                    path: path.to_string(),
                    action,
                });
            }
        };
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
    /// Combines two previously-independent mechanisms into one decision:
    ///
    /// - **`FunctionMeta::security`** (CRIT-6 part B / audit #440): the
    ///   declared `SECURITY INVOKER` / `SECURITY DEFINER` semantics, stored
    ///   in the catalogue. This is the *modern, explicit* declaration.
    /// - **The POSIX setuid mode bit** on `ResourceMeta`: a legacy,
    ///   bit-level escalation flag carried over from the pre-slice-9
    ///   access model.
    ///
    /// # Decision table
    ///
    /// | `security`   | setuid bit | effective actor     |
    /// |--------------|------------|---------------------|
    /// | `Definer`    | (any)      | function **owner**  |
    /// | `Invoker`    | set        | function **owner** (legacy) |
    /// | `Invoker`    | clear      | **caller**          |
    ///
    /// ## Why `Definer` always escalates
    ///
    /// `SECURITY DEFINER` is an explicit, unconditional request for
    /// owner-privilege execution (mirrors Postgres `SECURITY DEFINER`):
    /// the function runs as its owner regardless of the legacy mode bit.
    /// Before CRIT-6 this declaration was dead weight — it now drives
    /// the decision.
    ///
    /// ## Why `Invoker` still honours the setuid bit
    ///
    /// Two design alternatives were considered:
    ///
    /// 1. **Strict** — `Invoker` is a hard, unblockable guarantee that the
    ///    function runs as the caller, ignoring the setuid bit entirely.
    ///    Most secure in isolation, but it silently changes the runtime
    ///    behaviour of every pre-existing function whose catalogue row
    ///    lacks an explicit `security` field (those default to `Invoker`)
    ///    yet relied on the setuid bit to escalate — a quiet, broad
    ///    behavioural regression.
    /// 2. **Legacy-compatible** (chosen) — an explicit `Invoker`
    ///    declaration still honours the setuid bit, preserving the
    ///    pre-CRIT-6 behaviour for callers that depended on the mode bit
    ///    alone. The primary defect (that `Security::Definer` was never
    ///    applied) is closed because `Definer` now unconditionally
    ///    escalates. The setuid bit becomes a legacy escalation path that
    ///    is *redundant* under `Definer` and *preserved* under `Invoker`
    ///    for backward compatibility.
    ///
    /// This keeps the existing setuid-driven test
    /// (`effective_fn_actor_switches_on_setuid`, which models a function
    /// with no explicit `security` and the setuid bit set) green while
    /// still making `Security::Definer` a real, enforced declaration.
    ///
    /// # Fail-closed guarantee
    ///
    /// Privilege escalation only happens when the function record is
    /// **definitively loaded** from the catalogue with a real (non-default)
    /// owner stored. On any error or not-found the caller is returned
    /// unchanged — never `Actor::System` via a `ResourceMeta::open()`
    /// default.
    pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
        // Load the raw function record directly so we can distinguish
        // "record found" from "error / not present" (the latter must not
        // escalate the caller to the open()-default owner of System).
        let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else {
            return caller.clone();
        };
        let res_meta = ResourceMeta::from_record(&rec);
        let fn_meta = FunctionMeta::from_record(&rec);
        match fn_meta.security {
            // Explicit definer request → always run as the function owner,
            // irrespective of the legacy POSIX setuid mode bit.
            Security::Definer => res_meta.owner,
            // Explicit (or defaulted) invoker: honour the legacy setuid
            // bit for backward compatibility — see the doc note above.
            Security::Invoker => {
                if Mode::is_setuid(res_meta.mode) {
                    res_meta.owner
                } else {
                    caller.clone()
                }
            }
        }
    }

    /// Assemble the access-control tree as a structured [`QueryValue`] map.
    ///
    /// Shape (see [`shamir_query_types::admin::AccessTreeOp`]):
    /// ```text
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
    ) -> DbResult<QueryValue> {
        // ── principals first, so resource nodes resolve owner/group names ──
        let mut name_of: TFxMap<u64, String> = TFxMap::default();
        name_of.insert(OWNER_SYSTEM, "system".to_string());
        let mut users_list: Vec<QueryValue> = Vec::new();
        for rec in self.system_store.load_users().await? {
            if let Some(uname) = rec.get("name").and_then(|v| v.as_str()) {
                let id = principal_id(uname);
                name_of.insert(id, uname.to_string());
                let mut m = new_map();
                m.insert("id".to_string(), QueryValue::Int(id as i64));
                m.insert("name".to_string(), QueryValue::Str(uname.to_string()));
                users_list.push(QueryValue::Map(m));
            }
        }
        users_list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        let mut group_name_of: TFxMap<u64, String> = TFxMap::default();
        let mut groups_list: Vec<QueryValue> = Vec::new();
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
            let members: Vec<QueryValue> = rec
                .get("members")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.as_u64())
                        .map(|m| {
                            let mut mm = new_map();
                            mm.insert("id".to_string(), QueryValue::Int(m as i64));
                            mm.insert(
                                "name".to_string(),
                                name_of
                                    .get(&m)
                                    .map(|n| QueryValue::Str(n.clone()))
                                    .unwrap_or(QueryValue::Null),
                            );
                            QueryValue::Map(mm)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut gm = new_map();
            gm.insert("id".to_string(), QueryValue::Int(gid as i64));
            gm.insert("name".to_string(), QueryValue::Str(gname));
            gm.insert("members".to_string(), QueryValue::List(members));
            groups_list.push(QueryValue::Map(gm));
        }
        groups_list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        // ── resource hierarchy (Root → Database → Store → Table) ──
        let max_depth = depth.unwrap_or(3).min(3);
        let root_meta = self.resource_meta(&ResourcePath::Root).await?;
        let mut root = access_node("/", "root", &root_meta, &name_of, &group_name_of);

        if max_depth >= 1 {
            let dbs: Vec<String> = match db_filter {
                Some(d) => self.list_dbs().into_iter().filter(|x| x == d).collect(),
                None => self.list_dbs(),
            };
            let mut db_children: Vec<QueryValue> = Vec::new();
            for dbname in dbs {
                let dm = self.resource_meta(&ResourcePath::database(&dbname)).await?;
                let mut dbnode = access_node(&dbname, "database", &dm, &name_of, &group_name_of);
                if max_depth >= 2 {
                    if let Some(inst) = self.get_db(&dbname) {
                        let mut store_children: Vec<QueryValue> = Vec::new();
                        for store in inst.list_repos() {
                            let sm = self
                                .resource_meta(&ResourcePath::store(&dbname, &store))
                                .await?;
                            let mut snode =
                                access_node(&store, "store", &sm, &name_of, &group_name_of);
                            if max_depth >= 3 {
                                if let Ok(tables) = inst.list_tables(&store) {
                                    let mut tnodes: Vec<QueryValue> = Vec::new();
                                    for t in tables {
                                        let tm = self
                                            .resource_meta(&ResourcePath::table(
                                                &dbname, &store, &t,
                                            ))
                                            .await?;
                                        tnodes.push(access_node(
                                            &t,
                                            "table",
                                            &tm,
                                            &name_of,
                                            &group_name_of,
                                        ));
                                    }
                                    if let QueryValue::Map(ref mut m) = snode {
                                        m.insert("children".to_string(), QueryValue::List(tnodes));
                                    }
                                }
                            }
                            store_children.push(snode);
                        }
                        if let QueryValue::Map(ref mut m) = dbnode {
                            m.insert("children".to_string(), QueryValue::List(store_children));
                        }
                    }
                }
                db_children.push(dbnode);
            }
            if let QueryValue::Map(ref mut m) = root {
                m.insert("children".to_string(), QueryValue::List(db_children));
            }
        }

        // ── functions (flat for now; folders land in a later slice) ──
        let mut functions: Vec<QueryValue> = Vec::new();
        for fname in self.list_functions().await? {
            let fm = self.resource_meta(&ResourcePath::function(&fname)).await?;
            let mut fnode = access_node(&fname, "function", &fm, &name_of, &group_name_of);
            if let QueryValue::Map(ref mut m) = fnode {
                m.swap_remove("children");
                m.insert(
                    "builtin".to_string(),
                    QueryValue::Bool(self.function_meta(&fname).is_none()),
                );
            }
            functions.push(fnode);
        }

        let mut principals = new_map();
        principals.insert("users".to_string(), QueryValue::List(users_list));
        principals.insert("groups".to_string(), QueryValue::List(groups_list));

        let mut result = new_map();
        result.insert("resources".to_string(), root);
        result.insert("functions".to_string(), QueryValue::List(functions));
        result.insert("principals".to_string(), QueryValue::Map(principals));

        Ok(QueryValue::Map(result))
    }
}

/// Build one access-tree node as a [`QueryValue`] map, resolving the
/// owner/group ids to names via the supplied lookups. Callers attach
/// `children` afterwards (leaf nodes keep the empty list; functions drop it).
pub(super) fn access_node(
    name: &str,
    kind: &str,
    meta: &ResourceMeta,
    name_of: &TFxMap<u64, String>,
    group_name_of: &TFxMap<u64, String>,
) -> QueryValue {
    let owner_id = meta.owner.to_owner_id();
    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str(name.to_string()));
    m.insert("kind".to_string(), QueryValue::Str(kind.to_string()));
    m.insert("owner".to_string(), QueryValue::Int(owner_id as i64));
    m.insert(
        "owner_name".to_string(),
        name_of
            .get(&owner_id)
            .map(|n| QueryValue::Str(n.clone()))
            .unwrap_or(QueryValue::Null),
    );
    m.insert(
        "group".to_string(),
        meta.group
            .map(|g| QueryValue::Int(g as i64))
            .unwrap_or(QueryValue::Null),
    );
    m.insert(
        "group_name".to_string(),
        meta.group
            .and_then(|g| group_name_of.get(&g))
            .map(|n| QueryValue::Str(n.clone()))
            .unwrap_or(QueryValue::Null),
    );
    m.insert("mode".to_string(), QueryValue::Int(meta.mode as i64));
    m.insert(
        "setuid".to_string(),
        QueryValue::Bool(Mode::is_setuid(meta.mode)),
    );
    m.insert("children".to_string(), QueryValue::List(Vec::new()));
    QueryValue::Map(m)
}
