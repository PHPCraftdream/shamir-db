//! Admin handlers: Chmod, Chown, Chgrp, CreateGroup, DropGroup, AddGroupMember, RemoveGroupMember, AccessTree.

use serde_json::json;

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

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
        let mut meta = self.shamir.resource_meta(&path).await;
        meta.mode = op.mode;
        self.shamir
            .set_resource_meta(&path, &meta)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "chmod": serde_json::to_value(&op.chmod).map_err(|e| err(e.to_string()))?,
            "mode": op.mode,
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
        let mut meta = self.shamir.resource_meta(&path).await;
        meta.owner = Actor::from_owner_id(op.owner);
        self.shamir
            .set_resource_meta(&path, &meta)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "chown": serde_json::to_value(&op.chown).map_err(|e| err(e.to_string()))?,
            "owner": op.owner,
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
        let mut meta = self.shamir.resource_meta(&path).await;
        meta.group = op.group;
        self.shamir
            .set_resource_meta(&path, &meta)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "chgrp": serde_json::to_value(&op.chgrp).map_err(|e| err(e.to_string()))?,
            "group": op.group,
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
            .create_group(&op.create_group)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "created_group": op.create_group,
            "group_id": group_id,
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
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Groups are global; managing them requires Manage on the root.
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let group_id = self
            .shamir
            .resolve_group_id(&op.drop_group)
            .await
            .map_err(|e| err(e.to_string()))?;
        self.shamir
            .drop_group(group_id)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "dropped_group_id": group_id,
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
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Groups are global; managing them requires Manage on the root.
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let group_id = self
            .shamir
            .resolve_group_id(&op.add_group_member)
            .await
            .map_err(|e| err(e.to_string()))?;
        self.shamir
            .add_group_member(group_id, op.user)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "added_to_group": group_id,
            "user": op.user,
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
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Groups are global; managing them requires Manage on the root.
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let group_id = self
            .shamir
            .resolve_group_id(&op.remove_group_member)
            .await
            .map_err(|e| err(e.to_string()))?;
        self.shamir
            .remove_group_member(group_id, op.user)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({
            "removed_from_group": group_id,
            "user": op.user,
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
        Ok(admin_result(json!({ "access_tree": tree })))
    }
}
