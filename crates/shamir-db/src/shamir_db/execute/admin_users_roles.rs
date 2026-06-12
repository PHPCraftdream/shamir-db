//! Admin handlers: CreateUser, DropUser, CreateRole, DropRole, GrantRole, RevokeRole.

use std::sync::Arc;

use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, hash_password};

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_user(
        &self,
        op: &crate::query::auth::CreateUserOp,
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

        // Authorization (owner-delegation): a global admin (Manage on
        // root) may create any user; a database owner may create users
        // scoped to their own database. System bypasses.
        self.authorize_user_lifecycle(op.database.as_deref())
            .await
            .map_err(err_access)?;
        // Hash the password at rest with Argon2id (PHC string).
        // This `users.password_hash` field is RBAC/admin metadata,
        // NOT a live-auth credential — the wire login path is
        // SCRAM-Argon2id in `shamir-connect` over StoredKey /
        // ServerKey, which never reads this field. Hashing here is
        // defense-in-depth for the at-rest secret; no verify-side
        // change is required.
        let password_hash = crate::query::auth::SecretString::new(
            hash_password(op.password.reveal()).map_err(|e| err(e.to_string()))?,
        );
        let user = crate::query::auth::User {
            name: op.create_user.clone(),
            password_hash,
            roles: op.roles.clone(),
            profile: op.profile.clone(),
            database: op.database.clone(),
        };
        let user_json = serde_json::to_value(&user).map_err(|e| err(e.to_string()))?;
        let table = self
            .shamir
            .system_store()
            .users_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: json!({"name": op.create_user}).into(),
            value: user_json.into(),
        };
        table
            .execute_set(&set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({"created_user": op.create_user})))
    }

    pub(super) async fn handle_drop_user(
        &self,
        op: &crate::query::auth::DropUserOp,
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

        let table = self
            .shamir
            .system_store()
            .users_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let interner = table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);

        // Authorization (owner-delegation): resolve the target user's
        // stored database scope so a database owner can only drop users
        // bound to their own database. A non-existent user resolves to
        // `None` scope → only a global admin (or System) may proceed.
        let scope = {
            let lookup = crate::query::read::ReadQuery::new("users").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String(op.drop_user.clone()),
                },
            );
            let existing = table
                .read(&lookup, &ctx)
                .await
                .map_err(|e| err(e.to_string()))?;
            existing.records.first().and_then(|rec| {
                rec.get("database")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
        };
        self.authorize_user_lifecycle(scope.as_deref())
            .await
            .map_err(err_access)?;

        let del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("users"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(op.drop_user.clone()),
            },
        };
        let result = table
            .execute_delete(&del_op, &ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"dropped_user": op.drop_user, "existed": result.affected > 0}),
        ))
    }

    pub(super) async fn handle_create_role(
        &self,
        op: &crate::query::auth::CreateRoleOp,
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

        // Role management is global-admin only (Manage on the root).
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let role = crate::query::auth::Role {
            name: op.create_role.clone(),
            permissions: op.permissions.clone(),
        };
        let role_json = serde_json::to_value(&role).map_err(|e| err(e.to_string()))?;
        let table = self
            .shamir
            .system_store()
            .roles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("roles"),
            key: json!({"name": op.create_role}).into(),
            value: role_json.into(),
        };
        table
            .execute_set(&set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(json!({"created_role": op.create_role})))
    }

    pub(super) async fn handle_drop_role(
        &self,
        op: &crate::query::auth::DropRoleOp,
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

        // Role management is global-admin only (Manage on the root).
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let table = self
            .shamir
            .system_store()
            .roles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let interner = table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("roles"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(op.drop_role.clone()),
            },
        };
        let result = table
            .execute_delete(&del_op, &ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"dropped_role": op.drop_role, "existed": result.affected > 0}),
        ))
    }

    pub(super) async fn handle_grant_role(
        &self,
        op: &crate::query::auth::GrantRoleOp,
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

        // Role grants are global-admin only (Manage on the root).
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let user_lock = self
            .shamir
            .admin_user_locks()
            .entry(op.user.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _user_guard = user_lock.lock().await;

        // Read user, add role, write back
        let table = self
            .shamir
            .system_store()
            .users_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let interner = table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query =
            crate::query::read::ReadQuery::new("users").filter(crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(op.user.clone()),
            });
        let result = table
            .read(&query, &ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        if result.records.is_empty() {
            return Err(err_code(
                "not_found",
                format!("User '{}' not found", op.user),
            ));
        }
        let mut user_json = result.records[0].clone();
        if let Some(roles) = user_json.get_mut("roles").and_then(|r| r.as_array_mut()) {
            if !roles.contains(&json!(op.grant_role)) {
                roles.push(json!(op.grant_role));
            }
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: json!({"name": op.user}).into(),
            value: user_json.into(),
        };
        table
            .execute_set(&set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"granted_role": op.grant_role, "user": op.user}),
        ))
    }

    pub(super) async fn handle_revoke_role(
        &self,
        op: &crate::query::auth::RevokeRoleOp,
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

        // Role revokes are global-admin only (Manage on the root).
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;
        let user_lock = self
            .shamir
            .admin_user_locks()
            .entry(op.user.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _user_guard = user_lock.lock().await;

        let table = self
            .shamir
            .system_store()
            .users_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let interner = table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query =
            crate::query::read::ReadQuery::new("users").filter(crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(op.user.clone()),
            });
        let result = table
            .read(&query, &ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        if result.records.is_empty() {
            return Err(err_code(
                "not_found",
                format!("User '{}' not found", op.user),
            ));
        }
        let mut user_json = result.records[0].clone();
        if let Some(roles) = user_json.get_mut("roles").and_then(|r| r.as_array_mut()) {
            roles.retain(|r| r != &json!(op.revoke_role));
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: json!({"name": op.user}).into(),
            value: user_json.into(),
        };
        table
            .execute_set(&set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"revoked_role": op.revoke_role, "user": op.user}),
        ))
    }
}
