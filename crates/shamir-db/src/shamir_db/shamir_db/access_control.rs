use serde_json::json;
use std::collections::HashMap;

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
            ResourcePath::FunctionFolder { path } => {
                // Read persisted meta if the folder was explicitly created;
                // fall back to open so functions with slash-names under
                // implicit (never-created) folders are not denied (#118).
                let path_key = path.join("/");
                let rec = self.system_store.load_function_folder(&path_key).await;
                rec.ok()
                    .flatten()
                    .map(|r| ResourceMeta::from_record(&r))
                    .unwrap_or_default()
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
    ///
    /// # Fail-closed guarantee
    ///
    /// Privilege escalation only happens when the function record is
    /// **definitively loaded** from the catalogue with both `setuid` set
    /// and a real (non-default) owner stored. On any error or
    /// not-found the caller is returned unchanged — never `Actor::System`
    /// via a `ResourceMeta::open()` default.
    pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
        // Load the raw function record directly so we can distinguish
        // "record found" from "error / not present" (the latter must not
        // escalate the caller to the open()-default owner of System).
        let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else {
            return caller.clone();
        };
        let meta = ResourceMeta::from_record(&rec);
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
pub(super) fn access_node(
    name: &str,
    kind: &str,
    meta: &ResourceMeta,
    name_of: &HashMap<u64, String>,
    group_name_of: &HashMap<u64, String>,
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
