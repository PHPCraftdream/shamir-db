use shamir_types::types::value::QueryValue;

use crate::access::{AccessError, Action, Actor, ResourceMeta, ResourcePath};
use crate::{DbError, DbResult};

use super::super::ShamirDb;

impl ShamirDb {
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
        // Existence guard under the group lock — root auth path returns
        // before any existence check.
        if self.system_store.load_group(group_id).await?.is_none() {
            return Err(DbError::NotFound(format!(
                "group id {} not found",
                group_id
            )));
        }
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
        // Existence guard under the group lock — root auth path returns
        // before any existence check.
        if self.system_store.load_group(group_id).await?.is_none() {
            return Err(DbError::NotFound(format!(
                "group id {} not found",
                group_id
            )));
        }
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
}
