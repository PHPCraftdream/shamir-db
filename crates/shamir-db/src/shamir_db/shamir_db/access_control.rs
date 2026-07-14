use shamir_collections::TFxMap;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
// CRIT-6 part B / audit #440 — `Security`/`FunctionMeta` carry the
// declared SECURITY INVOKER / DEFINER semantics; previously dead weight,
// now consulted by `effective_fn_actor`. Re-exported by `shamir_engine`
// (which aliases `shamir_wasm_host as function`).
use shamir_engine::function::{FunctionMeta, Security};

use crate::access::{
    permits, trace_access, AccessError, Action, Actor, Mode, ResourceMeta, ResourcePath,
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
            // Root — full persisted meta, settings key "root_meta". Mirrors
            // the FunctionNamespace arm above. Default (absent key) is
            // System-owned, `0o755` — traverse/list stay open exactly as
            // today; only "write to root" (creating a top-level database)
            // narrows from everyone-writable to owner-only (task #552).
            ResourcePath::Root => match self.system_store.load_setting("root_meta").await {
                Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
                Ok(None) => Ok(ResourceMeta {
                    owner: Actor::System,
                    group: None,
                    mode: 0o755,
                }),
                Err(e) => {
                    log::warn!("resource_meta: failed to load root meta: {e}");
                    Err(e)
                }
            },
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

    /// Create a group with the given name. Returns the allocated group id.
    ///
    /// Thin `Actor::System` wrapper around [`create_group_as`](Self::create_group_as)
    /// — see that method's doc comment for the group-CRUD `Manage(Root)`
    /// self-defense rationale (task #546). Kept for existing callers
    /// (tests, offline/CLI tooling) that don't carry an `Actor`; every
    /// wire-reachable admin dispatcher call (`admin_access.rs`) goes
    /// through `create_group_as` with the real actor.
    ///
    /// Group ids are allocated monotonically from a counter stored in the
    /// `settings` table under the key `"next_group_id"`. Id 0 is
    /// reserved/unused; allocation starts from 1.
    pub async fn create_group(&self, name: &str) -> DbResult<u64> {
        self.create_group_as(name, &Actor::System).await
    }

    /// Create a group, self-defending with the `Manage(Root)` gate.
    ///
    /// Task #546 hardening: group-CRUD previously did NO authorization
    /// check itself — safety relied ENTIRELY on the dispatcher
    /// (`admin_access.rs::handle_create_group`) pre-calling
    /// `authorize_access(Root, Manage)` before reaching this method. That
    /// dispatcher check still runs (this is a deliberately redundant,
    /// structurally-enforced inline check, not a replacement for it) —
    /// the two compose rather than conflict: `Actor::System` bypasses
    /// both, and a real dispatcher call that already authorized simply
    /// pays one extra (cheap, System-bypassed-or-already-passing) check.
    /// The goal is that a FUTURE caller who reaches this method WITHOUT
    /// going through the dispatcher (e.g. a new internal code path) can't
    /// silently skip the gate.
    ///
    /// Group ids are allocated monotonically from a counter stored in the
    /// `settings` table under the key `"next_group_id"`. Id 0 is
    /// reserved/unused; allocation starts from 1.
    pub async fn create_group_as(&self, name: &str, actor: &Actor) -> DbResult<u64> {
        self.authorize_access(actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(|e| DbError::Validation(e.to_string()))?;

        // Serialise the whole read-modify-write (rare op, bounded contention).
        // §84 / #570: this same global lock is ALSO acquired by
        // `rename_group_as`'s name-uniqueness scan+write, so a create can't
        // race a rename — nor another create — onto the same name. It is the
        // single point of serialization for the whole group-NAME namespace.
        let _guard = self.group_id_lock.lock().await;

        // Load the existing-group snapshot ONCE, reused both for the
        // name-uniqueness guard below and for counter seeding in the
        // `next_group_id`-absent branch (avoids a redundant second scan).
        let existing = self.system_store.load_groups().await?;

        // Name-uniqueness guard (task #570): reject if ANY existing group
        // already holds `name`. Performed WHILE holding `group_id_lock` so a
        // concurrent create or rename of the same name serializes against
        // this scan → only one passes before the other's `save_group` lands.
        // (No `group_id != gid` exclusion is needed here, unlike
        // `rename_group_as`: the new id isn't allocated yet, so there is no
        // "self" record to exclude.)
        if existing
            .iter()
            .any(|g| g.get("name").and_then(|v| v.as_str()) == Some(name))
        {
            return Err(DbError::KeyExists(format!("group '{name}' already exists")));
        }

        let current = match self
            .system_store
            .load_setting("next_group_id")
            .await?
            .and_then(|v| v.as_u64())
        {
            Some(v) => v,
            // Counter absent: seed past the highest EXISTING group id so a
            // lost/missing setting can't collide with a live group.
            None => existing
                .iter()
                .filter_map(|g| g.get("group_id").and_then(|v| v.as_u64()))
                .max()
                .map_or(1, |m| m + 1),
        };
        let group_id = current;

        // Durability: bump the counter BEFORE writing the group, so a crash
        // in between only LEAKS an id (monotonic) — it can never overwrite the
        // next group on restart.
        self.system_store
            .save_setting("next_group_id", &QueryValue::Int((current + 1) as i64))
            .await?;
        self.system_store
            .save_group(group_id, name, &[], actor.to_owner_id())
            .await?;
        Ok(group_id)
    }

    /// Drop a group by id.
    ///
    /// Thin `Actor::System` wrapper — see
    /// [`drop_group_as`](Self::drop_group_as).
    pub async fn drop_group(&self, group_id: u64) -> DbResult<()> {
        self.drop_group_as(group_id, &Actor::System).await
    }

    /// Drop a group by id, self-defending with EITHER the `Manage(Root)`
    /// gate OR `Manage(Group{name})` (task #552 — a group's own creator
    /// manages their own group without needing global root admin). See
    /// [`create_group_as`](Self::create_group_as)'s doc comment for the
    /// task #546 rationale (redundant-with-dispatcher, composes with
    /// `Actor::System` bypass, guards a future non-dispatcher caller).
    pub async fn drop_group_as(&self, group_id: u64, actor: &Actor) -> DbResult<()> {
        self.authorize_group_manage_or_root(group_id, actor).await?;
        // §81 / #563: hold the per-group_id lock across the delete so a
        // concurrent in-flight add/remove-member/set-owner/rename can't
        // observe the pre-drop record, get descheduled, and then `save_group`
        // it back AFTER `remove_group` has run — resurrecting the dropped
        // group with stale fields (the delete-vs-RMW TOCTOU).
        let group_lock = self
            .group_member_locks()
            .entry(group_id)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _group_guard = group_lock.lock().await;
        self.system_store.remove_group(group_id).await
    }

    /// Rename an existing group.
    ///
    /// Thin `Actor::System` wrapper — see
    /// [`rename_group_as`](Self::rename_group_as).
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
        self.rename_group_as(group_ref, to, &Actor::System).await
    }

    /// Rename an existing group, self-defending with EITHER the
    /// `Manage(Root)` gate OR `Manage(Group{name})` (task #552). See
    /// [`create_group_as`](Self::create_group_as)'s doc comment for the
    /// task #546 rationale.
    pub async fn rename_group_as(
        &self,
        group_ref: &crate::query::admin::GroupRef,
        to: &str,
        actor: &Actor,
    ) -> DbResult<()> {
        let gid = self.resolve_group_id(group_ref).await?;
        self.authorize_group_manage_or_root(gid, actor).await?;

        // §81 / #563: hold the per-group_id lock across the whole
        // read(members+owner)→write(save_group) RMW so a concurrent
        // add/remove-member/set-owner/drop on the SAME group_id can't lose
        // this rename (or have this rename silently revert a racing
        // member-add/chown via a stale-read overwrite).
        let group_lock = self
            .group_member_locks()
            .entry(gid)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _group_guard = group_lock.lock().await;

        // §84 / #570: ALSO hold the global `group_id_lock` across the
        // name-uniqueness scan → `save_group` sequence — the SAME lock
        // `create_group_as` acquires for its (now also name-uniqueness-
        // guarded) allocation. This closes the cross-`group_id` race that the
        // per-group_id lock above can't reach (it only serialises mutations
        // on THIS id; the global lock serialises the shared NAME namespace
        // across BOTH allocation paths). Now two concurrent renames targeting
        // two DIFFERENT group_ids to the same `to`, or a create racing a
        // rename, can no longer both pass the scan before either write lands.
        //
        // Lock-order / deadlock note: `group_id_lock` is only ever acquired
        // alone in `create_group_as`, or nested here INSIDE a single
        // `group_member_locks` entry — never the reverse order anywhere in
        // the codebase, so no lock-order cycle is possible.
        let _name_guard = self.group_id_lock.lock().await;

        // Uniqueness guard: reject if a *different* group already owns `to`.
        let groups = self.system_store.load_groups().await?;
        let conflict = groups.iter().any(|g| {
            g.get("name").and_then(|v| v.as_str()) == Some(to)
                && g.get("group_id").and_then(|v| v.as_u64()) != Some(gid)
        });
        if conflict {
            return Err(DbError::KeyExists(format!("group '{}' already exists", to)));
        }

        // Preserve membership AND ownership across the name rewrite —
        // renaming must not touch ownership (task #552).
        let members = self.group_members(gid).await?;
        let owner = self
            .system_store
            .load_group(gid)
            .await?
            .and_then(|rec| ResourceMeta::owner_field(&rec))
            .unwrap_or(Actor::System)
            .to_owner_id();
        self.system_store
            .save_group(gid, to, &members, owner)
            .await?;
        Ok(())
    }

    /// Add a user to a group.
    ///
    /// Thin `Actor::System` wrapper — see
    /// [`add_group_member_as`](Self::add_group_member_as).
    pub async fn add_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        self.add_group_member_as(group_id, user_id, &Actor::System)
            .await
    }

    /// Add a user to a group, self-defending with EITHER the
    /// `Manage(Root)` gate OR `Manage(Group{name})` (task #552). See
    /// [`create_group_as`](Self::create_group_as)'s doc comment for the
    /// task #546 rationale.
    pub async fn add_group_member_as(
        &self,
        group_id: u64,
        user_id: u64,
        actor: &Actor,
    ) -> DbResult<()> {
        self.authorize_group_manage_or_root(group_id, actor).await?;
        // §81 / #563: serialise the whole load→mutate→save_group RMW against
        // concurrent mutations on the SAME group_id (chown/rename/remove/
        // drop), closing the last-writer-wins-on-whole-record race.
        let group_lock = self
            .group_member_locks()
            .entry(group_id)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _group_guard = group_lock.lock().await;
        self.system_store.add_group_member(group_id, user_id).await
    }

    /// Remove a user from a group.
    ///
    /// Thin `Actor::System` wrapper — see
    /// [`remove_group_member_as`](Self::remove_group_member_as).
    pub async fn remove_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        self.remove_group_member_as(group_id, user_id, &Actor::System)
            .await
    }

    /// Remove a user from a group, self-defending with EITHER the
    /// `Manage(Root)` gate OR `Manage(Group{name})` (task #552). See
    /// [`create_group_as`](Self::create_group_as)'s doc comment for the
    /// task #546 rationale.
    pub async fn remove_group_member_as(
        &self,
        group_id: u64,
        user_id: u64,
        actor: &Actor,
    ) -> DbResult<()> {
        self.authorize_group_manage_or_root(group_id, actor).await?;
        // §81 / #563: serialise the whole load→mutate→save_group RMW against
        // concurrent mutations on the SAME group_id (chown/rename/add/drop),
        // closing the last-writer-wins-on-whole-record race.
        let group_lock = self
            .group_member_locks()
            .entry(group_id)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _group_guard = group_lock.lock().await;
        self.system_store
            .remove_group_member(group_id, user_id)
            .await
    }

    /// Shared self-defense gate for the group-CRUD `*_as` methods (task
    /// #552): succeeds if EITHER `Manage(Root)` OR `Manage(Group{name})`
    /// passes for `actor`. `permits()` already resolves `Manage` as
    /// owner-only, so once the `Group` `resource_meta` arm reports the
    /// real persisted owner, this naturally allows a group's own creator
    /// to manage it without needing global root admin, while a stranger
    /// (no Root-Manage, not the owner) is denied.
    ///
    /// Resolves `group_id` back to the group's `name` via `load_group` so
    /// the check can build a `ResourcePath::Group { name }` — groups are
    /// id-keyed everywhere else, but `resource_meta`/`authorize_access`
    /// address groups by name (mirroring `User`/`Function`).
    ///
    /// `pub(crate)` (not private): the wire dispatcher
    /// (`execute::admin_access`) calls this directly before invoking the
    /// corresponding `*_as` method, so the OR-gate is actually reached from
    /// the only path real clients use — see the task #552 review finding
    /// that the dispatcher's OWN, older, unconditional `Manage(Root)`
    /// pre-check made this method's OR-logic unreachable dead code.
    pub(crate) async fn authorize_group_manage_or_root(
        &self,
        group_id: u64,
        actor: &Actor,
    ) -> DbResult<()> {
        if self
            .authorize_access(actor, &ResourcePath::Root, Action::Manage)
            .await
            .is_ok()
        {
            return Ok(());
        }
        let name = self
            .system_store
            .load_group(group_id)
            .await?
            .and_then(|rec| rec.get("name").and_then(|v| v.as_str()).map(str::to_string));
        if let Some(name) = name {
            if self
                .authorize_access(actor, &ResourcePath::group(name), Action::Manage)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(DbError::Validation(
            AccessError {
                actor: actor.clone(),
                path: ResourcePath::Root.to_string(),
                action: Action::Manage,
            }
            .to_string(),
        ))
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
            crate::query::admin::GroupRef::Id { id } => {
                // Wire-supplied group ids feed `QueryValue::Int`/
                // `FilterValue::Int` (both `i64`-based) downstream — an
                // `id > i64::MAX` would silently wrap to a negative number
                // on `as i64`. Server-generated group ids (monotonic
                // counter from 1) never approach this range; only a
                // caller-supplied `GroupRef::Id` can.
                if *id > i64::MAX as u64 {
                    return Err(DbError::Validation(format!(
                        "group id {id} exceeds the valid i64 range"
                    )));
                }
                Ok(*id)
            }
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
    /// denied path, and the action. The engine-level observability trace
    /// ([`trace_access`]) is still emitted first — it is NOT part of the
    /// enforcement decision (it always returns `Ok`); the real check is
    /// everything below this call.
    pub async fn authorize_access(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        // Engine-level observability trace (R2) — always emitted, always Ok.
        // NOT the enforcement gate; see `trace_access`'s doc comment.
        trace_access(actor, path, action)?;

        // Admin bypass — the common live path. Both `System` (anonymous
        // default) and `Admin(_)` (a real superuser session carrying its
        // principal64 id) short-circuit the gate; `Admin` differs from
        // `System` only in ownership attribution, not in gate semantics.
        if matches!(actor, Actor::System | Actor::Admin(_)) {
            return Ok(());
        }

        let user_id = match actor {
            Actor::User(id) => *id,
            Actor::System | Actor::Admin(_) => unreachable!(),
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
    /// **definitively loaded** from the catalogue AND carries an explicit,
    /// present `owner` field. The caller is returned unchanged — never
    /// `Actor::System` via a `ResourceMeta`/`from_record` default — in
    /// EITHER of two cases:
    ///
    /// - the record itself is absent, or the load errors (`load_function`
    ///   returns anything other than `Ok(Some(_))`); or
    /// - the record loads but its `owner` field is missing (a legacy
    ///   record predating the field, or a partially-written/corrupted
    ///   one) — `ResourceMeta::from_record`'s `unwrap_or(Actor::System)`
    ///   default is deliberately correct for every OTHER caller (DDL
    ///   introspection, `access_tree`, `resource_meta`) but would silently
    ///   escalate here, so escalation reads the owner via
    ///   [`ResourceMeta::owner_field`] instead, which distinguishes
    ///   "absent" (`None`) from "explicitly System" (`Some(Actor::System)`).
    pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
        // Load the raw function record directly so we can distinguish
        // "record found" from "error / not present" (the latter must not
        // escalate the caller to the open()-default owner of System).
        let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else {
            return caller.clone();
        };
        let res_meta = ResourceMeta::from_record(&rec);
        let fn_meta = FunctionMeta::from_record(&rec);
        // Fail-closed owner lookup for escalation: `None` (owner field
        // absent) must resolve to the caller, never to
        // `from_record`'s System default. See the doc comment above.
        let escalated_owner = || ResourceMeta::owner_field(&rec).unwrap_or_else(|| caller.clone());
        match fn_meta.security {
            // Explicit definer request → always run as the function owner,
            // irrespective of the legacy POSIX setuid mode bit.
            Security::Definer => escalated_owner(),
            // Explicit (or defaulted) invoker: honour the legacy setuid
            // bit for backward compatibility — see the doc note above.
            Security::Invoker => {
                if Mode::is_setuid(res_meta.mode) {
                    escalated_owner()
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
        // Task #559: enumerate principals from the injected
        // PrincipalResolver (the real directory) instead of Store B's
        // `users_table()`. With no resolver installed the principals
        // section is empty — the design doc's "absent resolver → names
        // resolve to null" degrade (NOT the old hash-based bridge).
        if let Some(resolver) = self.principal_resolver() {
            for p in resolver.list() {
                name_of.insert(p.principal64, p.name.clone());
                let mut m = new_map();
                m.insert("id".to_string(), QueryValue::Int(p.principal64 as i64));
                m.insert("name".to_string(), QueryValue::Str(p.name));
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
