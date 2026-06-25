//! Admin handlers: CreateUser, DropUser, CreateRole, DropRole, RenameRole,
//! GrantRole, RevokeRole.

use std::sync::Arc;

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, hash_password, to_qv};

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
        let user_qv = to_qv(&user);
        let table = self
            .shamir
            .system_store()
            .users_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: mpack!({"name": @(QueryValue::Str(op.create_user.clone()))}),
            value: user_qv,
        };
        // W3d-2: route through the implicit-tx file-WAL path.
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_user": @(QueryValue::Str(op.create_user.clone())),
        })))
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

        // if_exists early-exit: user not found → no-op.
        if op.if_exists {
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
            if existing.records.is_empty() {
                return Ok(admin_result(mpack!({
                    "dropped_user": @(QueryValue::Str(op.drop_user.clone())),
                    "existed": false,
                })));
            }
        }

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
            existing
                .records
                .first()
                .and_then(|rec| rec.get_value_owned("database"))
                .and_then(|v| {
                    if let QueryValue::Str(s) = v {
                        Some(s)
                    } else {
                        None
                    }
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
            select: None,
        };
        // F5a: route the delete through the implicit-tx file-WAL path
        // (`run_implicit_batch_tx` + `execute_delete_tx`) instead of the
        // direct V1-marker `execute_delete`, mirroring the query_runner
        // non-tx Delete branch.
        let repo = self
            .shamir
            .system_store()
            .system_repo()
            .map_err(|e| err(e.to_string()))?;
        let owned_op = del_op.clone();
        let owned_table = table.clone();
        let result = repo
            .run_implicit_batch_tx(Actor::System, "", move |tx| {
                Box::pin(async move {
                    let interner = owned_table.interner().get().await?;
                    let refs = crate::types::common::new_map();
                    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                    owned_table
                        .execute_delete_tx(&owned_op, &ctx, tx, None)
                        .await
                })
            })
            .await?;
        Ok(admin_result(mpack!({
            "dropped_user": @(QueryValue::Str(op.drop_user.clone())),
            "existed": @(QueryValue::Bool(result.affected > 0)),
        })))
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
        let role_qv = to_qv(&role);
        let table = self
            .shamir
            .system_store()
            .roles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("roles"),
            key: mpack!({"name": @(QueryValue::Str(op.create_role.clone()))}),
            value: role_qv,
        };
        // W3d-2: route through the implicit-tx file-WAL path.
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_role": @(QueryValue::Str(op.create_role.clone())),
        })))
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

        // if_exists early-exit: role not found → no-op.
        if op.if_exists {
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
            let lookup = crate::query::read::ReadQuery::new("roles").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String(op.drop_role.clone()),
                },
            );
            let existing = table
                .read(&lookup, &ctx)
                .await
                .map_err(|e| err(e.to_string()))?;
            if existing.records.is_empty() {
                return Ok(admin_result(mpack!({
                    "dropped_role": @(QueryValue::Str(op.drop_role.clone())),
                    "existed": false,
                })));
            }
        }

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
        let del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("roles"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(op.drop_role.clone()),
            },
            select: None,
        };
        // F5a: implicit-tx file-WAL delete path (see handle_drop_user).
        let repo = self
            .shamir
            .system_store()
            .system_repo()
            .map_err(|e| err(e.to_string()))?;
        let owned_op = del_op.clone();
        let owned_table = table.clone();
        let result = repo
            .run_implicit_batch_tx(Actor::System, "", move |tx| {
                Box::pin(async move {
                    let interner = owned_table.interner().get().await?;
                    let refs = crate::types::common::new_map();
                    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                    owned_table
                        .execute_delete_tx(&owned_op, &ctx, tx, None)
                        .await
                })
            })
            .await?;
        Ok(admin_result(mpack!({
            "dropped_role": @(QueryValue::Str(op.drop_role.clone())),
            "existed": @(QueryValue::Bool(result.affected > 0)),
        })))
    }

    pub(super) async fn handle_rename_role(
        &self,
        op: &crate::query::auth::RenameRoleOp,
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

        let from = op.rename_role.clone();
        let to = op.to.clone();

        // Renaming to itself is a no-op success (the role already exists
        // under that name, so there is nothing to re-key).
        if from == to {
            return Ok(admin_result(mpack!({
                "renamed_role": @(QueryValue::Str(from.clone())),
                "to": @(QueryValue::Str(to.clone())),
            })));
        }

        // Role management is global-admin only (Manage on the root).
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(err_access)?;

        let roles_table = self
            .shamir
            .system_store()
            .roles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let roles_interner = roles_table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let roles_ctx = crate::query::filter::FilterContext::new(roles_interner, &refs);

        // Guard source-exists: role `from` must be present.
        let source_lookup =
            crate::query::read::ReadQuery::new("roles").filter(crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(from.clone()),
            });
        let source_result = roles_table
            .read(&source_lookup, &roles_ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        if source_result.records.is_empty() {
            return Err(err_code("not_found", format!("Role '{}' not found", from)));
        }

        // Guard dest-free: role `to` must not already exist.
        let dest_lookup =
            crate::query::read::ReadQuery::new("roles").filter(crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(to.clone()),
            });
        let dest_result = roles_table
            .read(&dest_lookup, &roles_ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        if !dest_result.records.is_empty() {
            return Err(err_code(
                "already_exists",
                format!("Role '{}' already exists", to),
            ));
        }

        // Re-key the role record: clone the stored value, update `name`
        // to the new key, write it under `to`, then delete the old `from`
        // record.
        let mut role_qv = source_result.records[0].as_value().into_owned();
        if let QueryValue::Map(ref mut m) = role_qv {
            m.insert("name".to_string(), QueryValue::Str(to.clone()));
        }
        let role_set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("roles"),
            key: mpack!({"name": @(QueryValue::Str(to.clone()))}),
            value: role_qv,
        };
        self.shamir
            .system_store()
            .set_via_implicit_tx(&roles_table, &role_set_op)
            .await
            .map_err(|e| err(e.to_string()))?;

        let role_del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("roles"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(from.clone()),
            },
            select: None,
        };
        let repo = self
            .shamir
            .system_store()
            .system_repo()
            .map_err(|e| err(e.to_string()))?;
        let owned_op = role_del_op.clone();
        let owned_table = roles_table.clone();
        repo.run_implicit_batch_tx(Actor::System, "", move |tx| {
            Box::pin(async move {
                let interner = owned_table.interner().get().await?;
                let refs = crate::types::common::new_map();
                let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                owned_table
                    .execute_delete_tx(&owned_op, &ctx, tx, None)
                    .await
            })
        })
        .await?;
        roles_table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;

        // Rekey references in users: read every user, and for each whose
        // `roles` list contains `from`, replace it with `to` and write the
        // record back. Mirrors the per-user mutation in `handle_grant_role`.
        let users_table = self
            .shamir
            .system_store()
            .users_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let users_interner = users_table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let users_refs = crate::types::common::new_map();
        let users_ctx = crate::query::filter::FilterContext::new(users_interner, &users_refs);
        let all_users_query = crate::query::read::ReadQuery::new("users");
        let users_result = users_table
            .read(&all_users_query, &users_ctx)
            .await
            .map_err(|e| err(e.to_string()))?;

        let from_marker = QueryValue::Str(from.clone());
        let to_marker = QueryValue::Str(to.clone());
        for rec in &users_result.records {
            let needs_rekey = rec
                .get_value_owned("roles")
                .and_then(|v| {
                    if let QueryValue::List(l) = v {
                        Some(l.iter().any(|r| r == &from_marker))
                    } else {
                        None
                    }
                })
                .unwrap_or(false);
            if !needs_rekey {
                continue;
            }

            let user_name = rec
                .get_value_owned("name")
                .and_then(|v| {
                    if let QueryValue::Str(s) = v {
                        Some(s)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| err("user record missing 'name'".to_string()))?;

            // Per-user lock as in grant/revoke.
            let user_lock = self
                .shamir
                .admin_user_locks()
                .entry(user_name.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone();
            let _user_guard = user_lock.lock().await;

            let mut user_qv = rec.as_value().into_owned();
            if let QueryValue::Map(ref mut m) = user_qv {
                if let Some(QueryValue::List(ref mut list)) = m.get_mut("roles") {
                    for entry in list.iter_mut() {
                        if entry == &from_marker {
                            *entry = to_marker.clone();
                        }
                    }
                }
            }
            let user_set_op = crate::query::write::SetOp {
                set: crate::query::TableRef::new("users"),
                key: mpack!({"name": @(QueryValue::Str(user_name.clone()))}),
                value: user_qv,
            };
            self.shamir
                .system_store()
                .set_via_implicit_tx(&users_table, &user_set_op)
                .await
                .map_err(|e| err(e.to_string()))?;
        }
        users_table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;

        Ok(admin_result(mpack!({
            "renamed_role": @(QueryValue::Str(from.clone())),
            "to": @(QueryValue::Str(to.clone())),
        })))
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

        // Read user, add role, write back using QueryValue-native access.
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
        // Mutate the user record's `roles` list using QueryValue map operations.
        let mut user_qv = result.records[0].as_value().into_owned();
        if let QueryValue::Map(ref mut m) = user_qv {
            let roles = m
                .entry("roles".to_string())
                .or_insert(QueryValue::List(Vec::new()));
            if let QueryValue::List(ref mut list) = roles {
                let new_role = QueryValue::Str(op.grant_role.clone());
                if !list.contains(&new_role) {
                    list.push(new_role);
                }
            }
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: mpack!({"name": @(QueryValue::Str(op.user.clone()))}),
            value: user_qv,
        };
        // W3d-2: route through the implicit-tx file-WAL path.
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "granted_role": @(QueryValue::Str(op.grant_role.clone())),
            "user": @(QueryValue::Str(op.user.clone())),
        })))
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
        // Remove the role from the user record's `roles` list using QueryValue
        // map operations.
        let mut user_qv = result.records[0].as_value().into_owned();
        if let QueryValue::Map(ref mut m) = user_qv {
            if let Some(QueryValue::List(ref mut list)) = m.get_mut("roles") {
                let revoke_role = QueryValue::Str(op.revoke_role.clone());
                list.retain(|r| r != &revoke_role);
            }
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("users"),
            key: mpack!({"name": @(QueryValue::Str(op.user.clone()))}),
            value: user_qv,
        };
        // W3d-2: route through the implicit-tx file-WAL path.
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "revoked_role": @(QueryValue::Str(op.revoke_role.clone())),
            "user": @(QueryValue::Str(op.user.clone())),
        })))
    }
}
