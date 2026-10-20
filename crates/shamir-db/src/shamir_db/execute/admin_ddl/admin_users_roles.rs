//! Admin handlers: CreateUser, DropUser, GrantRole, RevokeRole.
//!
//! Task #559 re-targeted the storage half of each handler off shamir-db's
//! own (historically ineffective) Store B `users_table()`/`roles_table()`
//! onto the injected [`UserAdminPort`] (implemented by the embedding layer
//! over the real durable directory). The authorization gates stay EXACTLY
//! where they were:
//!   - `authorize_user_lifecycle` for create/drop (owner-delegation),
//!   - `Manage(Root)` for grant/revoke.
//!
//! `CreateRole`/`DropRole`/`RenameRole` were deleted entirely (a "role" is
//! now a plain string label on directory users — no role object exists).
//! Without an installed port these four handlers return `not_supported`
//! (hard cutover, not a soft Store-B fallback).

use crate::access::{Action, ResourcePath};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_user(
        &self,
        op: &crate::query::auth::CreateUserOp,
    ) -> Result<QueryResult, BatchError> {
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

        // Storage half: route through the injected UserAdminPort. Argon2id
        // derivation happens INSIDE the port impl (shamir-db never touches
        // SCRAM crypto). Without a port this is a hard `not_supported` —
        // the retirement of Store B is a behavioural cutover, not a soft
        // fallback.
        let Some(port) = self.shamir.user_admin_port() else {
            return Err(err_code(
                "not_supported",
                "user administration is not configured on this server".to_string(),
            ));
        };
        port.create_user(
            &op.create_user,
            op.password.reveal(),
            op.roles.clone(),
            op.database.clone(),
        )
        .await
        .map_err(|e| err_code("query", e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_user": @(QueryValue::Str(op.create_user.clone())),
        })))
    }

    pub(super) async fn handle_drop_user(
        &self,
        op: &crate::query::auth::DropUserOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Resolve the target user's database scope so a database owner can
        // only drop users bound to their own database. This used to read
        // Store B's `users_table()`; it now reads the injected
        // PrincipalResolver. With no resolver installed (or an unknown
        // user) scope resolves to `None` → only a global admin may proceed
        // (documented safe-but-degraded behaviour, design doc §3.1).
        let scope = self
            .shamir
            .principal_resolver()
            .and_then(|r| r.resolve_by_name(&op.drop_user))
            .and_then(|info| info.database);

        self.authorize_user_lifecycle(scope.as_deref())
            .await
            .map_err(err_access)?;

        let Some(port) = self.shamir.user_admin_port() else {
            return Err(err_code(
                "not_supported",
                "user administration is not configured on this server".to_string(),
            ));
        };
        let existed = port
            .drop_user(&op.drop_user)
            .await
            .map_err(|e| err_code("query", e.to_string()))?;
        Ok(admin_result(mpack!({
            "dropped_user": @(QueryValue::Str(op.drop_user.clone())),
            "existed": @(QueryValue::Bool(existed)),
        })))
    }

    pub(super) async fn handle_grant_role(
        &self,
        op: &crate::query::auth::GrantRoleOp,
    ) -> Result<QueryResult, BatchError> {
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

        let Some(port) = self.shamir.user_admin_port() else {
            return Err(err_code(
                "not_supported",
                "user administration is not configured on this server".to_string(),
            ));
        };
        // No per-user `admin_user_locks()` acquisition: the directory-side
        // `grant_role` does its read-modify-write under the directory's own
        // `write_lock`, which serialises the whole RMW atomically — the
        // shamir-db-level lock would be redundant double-locking (harmless
        // but dead weight).
        //
        // Surface `not_found` for an unknown target user (mirrors the
        // pre-#559 handler's explicit code + `set_superuser`'s precedent);
        // any other directory error falls back to the generic `query`.
        if let Err(e) = port.grant_role(&op.user, &op.grant_role).await {
            let msg = e.to_string();
            let code = if msg.contains("user not found") {
                "not_found"
            } else {
                "query"
            };
            return Err(err_code(code, msg));
        }
        Ok(admin_result(mpack!({
            "granted_role": @(QueryValue::Str(op.grant_role.clone())),
            "user": @(QueryValue::Str(op.user.clone())),
        })))
    }

    pub(super) async fn handle_revoke_role(
        &self,
        op: &crate::query::auth::RevokeRoleOp,
    ) -> Result<QueryResult, BatchError> {
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

        let Some(port) = self.shamir.user_admin_port() else {
            return Err(err_code(
                "not_supported",
                "user administration is not configured on this server".to_string(),
            ));
        };
        if let Err(e) = port.revoke_role(&op.user, &op.revoke_role).await {
            let msg = e.to_string();
            let code = if msg.contains("user not found") {
                "not_found"
            } else {
                "query"
            };
            return Err(err_code(code, msg));
        }
        Ok(admin_result(mpack!({
            "revoked_role": @(QueryValue::Str(op.revoke_role.clone())),
            "user": @(QueryValue::Str(op.user.clone())),
        })))
    }
}
