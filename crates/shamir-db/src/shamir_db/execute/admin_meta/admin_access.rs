//! Admin handlers: Chmod, Chown, Chgrp, CreateGroup, DropGroup, RenameGroup, AddGroupMember, RemoveGroupMember, AccessTree.

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::access::OWNER_SYSTEM;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, to_qv};

/// Shared error code for every "target id does not resolve to a real
/// principal/group" rejection below (`chown`'s OWNER_SYSTEM guard and
/// resolver-gated owner check, `chgrp`'s group check, `add_group_member`'s
/// group-id check and resolver-gated member check) — kept identical so
/// callers can pattern-match on one code regardless of which op tripped it.
const ERR_INVALID_OWNER: &str = "invalid_owner";

impl ShamirAdminExecutor {
    /// Existence check for a group id: groups ARE id-keyed (a direct point
    /// lookup via `load_group`, not a scan), so a group-id-exists check has
    /// always been possible unconditionally — unlike the user-id-exists
    /// checks, which had to wait for task #559's `PrincipalResolver` and are
    /// gated on one being installed (task #561). Used by
    /// `handle_add_group_member` to validate the group side of a
    /// `GroupRef::Id` (which `resolve_group_id` passes through unchecked) and
    /// by `handle_chgrp` to validate `op.group` (task #561 §2).
    pub(super) async fn group_id_exists(&self, group_id: u64) -> Result<bool, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        Ok(self
            .shamir
            .system_store()
            .load_group(group_id)
            .await
            .map_err(|e| err(e.to_string()))?
            .is_some())
    }
}

impl ShamirAdminExecutor {
    pub(super) async fn handle_chmod(
        &self,
        op: &crate::query::admin::ChmodOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        let path = op
            .chmod
            .to_path()
            .ok_or_else(|| err("invalid resource reference".to_string()))?;
        self.shamir
            .authorize_access(&self.actor, &path, Action::Manage)
            .await
            .map_err(err_access)?;
        let mut meta = self
            .shamir
            .resource_meta(&path)
            .await
            .map_err(|e| err(e.to_string()))?;
        meta.mode = op.mode;
        self.shamir
            .set_resource_meta(&path, &meta)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "chmod": @(to_qv(&op.chmod)),
            "mode": @(QueryValue::Int(op.mode as i64)),
        })))
    }

    pub(super) async fn handle_chown(
        &self,
        op: &crate::query::admin::ChownOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        let path = op
            .chown
            .to_path()
            .ok_or_else(|| err("invalid resource reference".to_string()))?;
        self.shamir
            .authorize_access(&self.actor, &path, Action::Manage)
            .await
            .map_err(err_access)?;

        // Forbid handing a resource to OWNER_SYSTEM unless the actor already
        // bypasses the gate (System, or a real superuser session — Admin) —
        // otherwise a non-admin owner (who only needed to hold Manage to
        // reach this handler) could one-way lock themselves (or anyone
        // else's resource they manage) out: only System/Admin can Manage a
        // System-owned resource thereafter. `Actor::Admin` must be included
        // here (not just `System`) because `session_actor` maps every real
        // superuser wire session to `Actor::Admin(principal64(..))`, never
        // to bare `Actor::System` (task #555) — without this, no live
        // superuser session could ever legitimately chown to System.
        if op.owner == OWNER_SYSTEM && !matches!(self.actor, Actor::System | Actor::Admin(_)) {
            return Err(err_code(
                ERR_INVALID_OWNER,
                "chown to the System owner is only permitted for the System actor".to_string(),
            ));
        }
        // Task #561: when a `PrincipalResolver` is installed, require the
        // (non-`OWNER_SYSTEM`) owner id to resolve to a real principal before
        // writing it into the catalogue. This is the user-target half of
        // task #543's originally-deferred validation, now coherent because
        // task #559 landed a real resolver backed by the durable
        // `FjallUserDirectory`. GATED on a resolver being installed: chown is
        // a core ACL op that has never required an injected port, and
        // hard-failing it in every embedded/no-directory deployment (and the
        // many tests that build a bare `ShamirDb` without one) would be
        // unjustified scope creep — absence means "cannot check, so don't".
        if op.owner != OWNER_SYSTEM {
            if let Some(resolver) = self.shamir.principal_resolver() {
                if resolver.resolve(op.owner).is_none() {
                    return Err(err_code(
                        ERR_INVALID_OWNER,
                        format!(
                            "chown target owner id {} does not resolve to a known principal",
                            op.owner
                        ),
                    ));
                }
            }
        }

        let mut meta = self
            .shamir
            .resource_meta(&path)
            .await
            .map_err(|e| err(e.to_string()))?;
        meta.owner = Actor::from_owner_id(op.owner);
        self.shamir
            .set_resource_meta(&path, &meta)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "chown": @(to_qv(&op.chown)),
            "owner": @(QueryValue::Int(op.owner as i64)),
        })))
    }

    pub(super) async fn handle_chgrp(
        &self,
        op: &crate::query::admin::ChgrpOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        let path = op
            .chgrp
            .to_path()
            .ok_or_else(|| err("invalid resource reference".to_string()))?;
        self.shamir
            .authorize_access(&self.actor, &path, Action::Manage)
            .await
            .map_err(err_access)?;

        // Task #561: when `op.group` is `Some(gid)`, require the group to
        // actually exist via the already-existing `group_id_exists` point
        // lookup. This is UNCONDITIONAL (no resolver/port dependency) — groups
        // have always been directly, id-keyed checkable, so this half of
        // task #543's deferred validation never depended on the identity-model
        // work (#548/#549, closed by #559). `None` (clearing the group) is
        // never checked — there is nothing to validate against.
        if let Some(gid) = op.group {
            if !self.group_id_exists(gid).await? {
                return Err(err_code(
                    ERR_INVALID_OWNER,
                    format!("chgrp target group id {gid} does not exist"),
                ));
            }
        }

        let mut meta = self
            .shamir
            .resource_meta(&path)
            .await
            .map_err(|e| err(e.to_string()))?;
        meta.group = op.group;
        self.shamir
            .set_resource_meta(&path, &meta)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "chgrp": @(to_qv(&op.chgrp)),
            "group": @(match op.group { Some(g) => QueryValue::Int(g as i64), None => QueryValue::Null }),
        })))
    }

    pub(super) async fn handle_create_group(
        &self,
        op: &crate::query::admin::CreateGroupOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Groups are global; managing them requires Manage on the root.
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let group_id = self
            .shamir
            .create_group_as(&op.create_group, &self.actor)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_group": @(QueryValue::Str(op.create_group.clone())),
            "group_id": @(QueryValue::Int(group_id as i64)),
        })))
    }

    pub(super) async fn handle_drop_group(
        &self,
        op: &crate::query::admin::DropGroupOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };

        // if_exists: resolve_group_id may fail for non-existent group → no-op.
        let group_id = match self.shamir.resolve_group_id(&op.drop_group).await {
            Ok(id) => id,
            Err(e) => {
                if op.if_exists {
                    return Ok(admin_result(mpack!({
                        "dropped_group_id": @(QueryValue::Null),
                        "existed": false,
                    })));
                }
                return Err(err(e.to_string()));
            }
        };
        // Groups are managed by EITHER Manage(Root) OR Manage(Group{name})
        // (task #552) — a group's own creator can drop it without needing
        // global root admin. Checked HERE, at the actual wire entry point,
        // not just inside `drop_group_as` — a prior revision left this
        // dispatcher's own unconditional Manage(Root)-only check in place,
        // which pre-rejected every non-Root-Manage caller before the OR-gate
        // inside `drop_group_as` was ever reached, making the whole feature
        // unreachable from any real client.
        self.shamir
            .authorize_group_manage_or_root(group_id, &self.actor)
            .await
            .map_err(|e| err_code("access_denied", e.to_string()))?;
        self.shamir
            .drop_group_as(group_id, &self.actor)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "dropped_group_id": @(QueryValue::Int(group_id as i64)),
            "existed": true,
        })))
    }

    pub(super) async fn handle_rename_group(
        &self,
        op: &crate::query::admin::RenameGroupOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };

        // Resolve the source group; rename requires it to exist (no if_exists).
        let group_id = self
            .shamir
            .resolve_group_id(&op.rename_group)
            .await
            .map_err(|e| err(e.to_string()))?;
        // Groups are managed by EITHER Manage(Root) OR Manage(Group{name})
        // (task #552) — see `handle_drop_group`'s comment for why this must
        // be checked here, at the wire entry point, not just inside
        // `rename_group_as` (which re-checks redundantly, by design).
        self.shamir
            .authorize_group_manage_or_root(group_id, &self.actor)
            .await
            .map_err(|e| err_code("access_denied", e.to_string()))?;
        self.shamir
            .rename_group_as(&op.rename_group, &op.to, &self.actor)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "renamed_group": @(QueryValue::Int(group_id as i64)),
            "to": @(QueryValue::Str(op.to.clone())),
        })))
    }

    pub(super) async fn handle_add_group_member(
        &self,
        op: &crate::query::admin::AddGroupMemberOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };

        let group_id = self
            .shamir
            .resolve_group_id(&op.add_group_member)
            .await
            .map_err(|e| err(e.to_string()))?;
        // Groups are managed by EITHER Manage(Root) OR Manage(Group{name})
        // (task #552) — see `handle_drop_group`'s comment for why this must
        // be checked here, at the wire entry point, not just inside
        // `add_group_member_as` (which re-checks redundantly, by design).
        // Checked BEFORE the `GroupRef::Id` existence check below so an
        // unauthorized caller learns nothing about whether a numeric group
        // id is real.
        self.shamir
            .authorize_group_manage_or_root(group_id, &self.actor)
            .await
            .map_err(|e| err_code("access_denied", e.to_string()))?;

        // `resolve_group_id` only validates existence for `GroupRef::Name`
        // (it scans `load_groups()` for a match); `GroupRef::Id { id }`
        // passes the id straight through with NO existence check. Without
        // this, `add_group_member(GroupRef::Id { id: <nonexistent> }, ...)`
        // would let `system_store::add_group_member` silently fabricate a
        // phantom group record (empty name, one member) at that id — so
        // the GROUP side still needs an explicit existence check here,
        // covering the `Id` case (the `Name` case is already guaranteed to
        // exist by `resolve_group_id`, so this is a cheap no-op there).
        if !self.group_id_exists(group_id).await? {
            return Err(err_code(
                ERR_INVALID_OWNER,
                format!("add_group_member target group id {group_id} does not exist"),
            ));
        }

        // `op.user` is a wire-supplied u64 that feeds `QueryValue::Int`
        // (i64-based) below — an `op.user > i64::MAX` would silently wrap to
        // a negative number on `as i64`. Reject it explicitly rather than
        // let it through with surprising/undefined filtering behavior.
        if op.user > i64::MAX as u64 {
            return Err(err_code(
                "query",
                format!("member user id {} exceeds the valid i64 range", op.user),
            ));
        }

        // Task #561: when a `PrincipalResolver` is installed, require the
        // member id (`op.user`) to resolve to a real principal before adding
        // it to the group. This is the user-target half of task #543's
        // originally-deferred validation for this op, now coherent because
        // task #559 landed a real resolver. GATED on a resolver being
        // installed (same rationale as `handle_chown`'s owner check): absence
        // means "cannot check, so don't". The GROUP-side check above is
        // unaffected (it is unconditional and closes a narrower, orthogonal
        // gap — `resolve_group_id` never validates `GroupRef::Id`).
        if let Some(resolver) = self.shamir.principal_resolver() {
            if resolver.resolve(op.user).is_none() {
                return Err(err_code(
                    ERR_INVALID_OWNER,
                    format!(
                        "add_group_member target user id {} does not resolve to a known principal",
                        op.user
                    ),
                ));
            }
        }

        self.shamir
            .add_group_member_as(group_id, op.user, &self.actor)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "added_to_group": @(QueryValue::Int(group_id as i64)),
            "user": @(QueryValue::Int(op.user as i64)),
        })))
    }

    pub(super) async fn handle_remove_group_member(
        &self,
        op: &crate::query::admin::RemoveGroupMemberOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };

        let group_id = self
            .shamir
            .resolve_group_id(&op.remove_group_member)
            .await
            .map_err(|e| err(e.to_string()))?;
        // Groups are managed by EITHER Manage(Root) OR Manage(Group{name})
        // (task #552) — see `handle_drop_group`'s comment for why this must
        // be checked here, at the wire entry point, not just inside
        // `remove_group_member_as` (which re-checks redundantly, by design).
        self.shamir
            .authorize_group_manage_or_root(group_id, &self.actor)
            .await
            .map_err(|e| err_code("access_denied", e.to_string()))?;

        // `op.user` is a wire-supplied u64 that feeds `QueryValue::Int`
        // (i64-based) below — an `op.user > i64::MAX` would silently wrap to
        // a negative number on `as i64`. Reject it explicitly rather than
        // let it through with surprising/undefined filtering behavior.
        if op.user > i64::MAX as u64 {
            return Err(err_code(
                "query",
                format!("member user id {} exceeds the valid i64 range", op.user),
            ));
        }

        // Deliberately NOT validating op.user's existence here (unlike
        // add_group_member): removing a membership is a set-removal —
        // removing an id that was never a member (because it never
        // resolved to a real user, or because it already isn't a member)
        // is a harmless, idempotent no-op, not a state that can orphan or
        // dangling-write anything. Nothing is created or written that
        // didn't already (potentially) exist.
        self.shamir
            .remove_group_member_as(group_id, op.user, &self.actor)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "removed_from_group": @(QueryValue::Int(group_id as i64)),
            "user": @(QueryValue::Int(op.user as i64)),
        })))
    }

    pub(super) async fn handle_access_tree(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::AccessTree(op) = batch_op else {
            unreachable!("handle_access_tree called with non-AccessTree op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Admin-only: reading the whole access fabric requires
        // `Manage` on the root. `System` bypasses; a non-admin
        // `User` actor is denied here.
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let tree = self
            .shamir
            .access_tree(op.depth, op.db.as_deref())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            mpack!({ "access_tree": @(QueryValue::from(tree)) }),
        ))
    }
}
