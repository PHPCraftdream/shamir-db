//! Admin handlers: CreateValidator, DropValidator, RenameValidator, BindValidator, UnbindValidator, ListValidators.

use base64::Engine;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::{explicit_migration_target, ShamirAdminExecutor};
use super::helpers::admin_result;

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::CreateValidator(op) = batch_op else {
            unreachable!("handle_create_validator called with non-CreateValidator op");
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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::FunctionNamespace,
                Action::Create,
            )
            .await
            .map_err(err_access)?;
        let id = if let Some(ref source) = op.source {
            self.shamir
                .create_validator_from_source_as(
                    &op.create_validator,
                    source,
                    op.replace,
                    self.actor.clone(),
                )
                .await
                .map_err(|e| err(e.to_string()))?
        } else if let Some(ref wasm_b64) = op.wasm {
            let wasm_bytes = base64::engine::general_purpose::STANDARD
                .decode(wasm_b64)
                .map_err(|e| err(format!("invalid base64 wasm: {}", e)))?;
            self.shamir
                .create_validator_from_wasm_as(
                    &op.create_validator,
                    &wasm_bytes,
                    op.replace,
                    self.actor.clone(),
                )
                .await
                .map_err(|e| err(e.to_string()))?
        } else {
            return Err(err(
                "create_validator requires either 'source' or 'wasm'".to_string()
            ));
        };
        Ok(admin_result(mpack!({
            "created_validator": @(QueryValue::Str(op.create_validator.clone())),
            "id": @(QueryValue::Str(id.to_string())),
        })))
    }

    pub(super) async fn handle_drop_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::DropValidator(op) = batch_op else {
            unreachable!("handle_drop_validator called with non-DropValidator op");
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

        // Auth runs BEFORE the if_exists existence probe (#995): an
        // unauthorized caller must not be able to learn whether a validator
        // they have no Delete right on exists, by toggling if_exists and
        // observing the distinguishable outcomes (silent {"existed": false}
        // no-op vs access_denied). This is a pre-auth existence oracle
        // otherwise. Mirrors #989's fix for handle_drop_index/handle_rename_index.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::FunctionNamespace,
                Action::Delete,
            )
            .await
            .map_err(err_access)?;

        // if_exists early-exit: validator not registered → no-op.
        if op.if_exists
            && self
                .shamir
                .validators()
                .id_for_name(&op.drop_validator)
                .is_none()
        {
            return Ok(admin_result(mpack!({
                "dropped_validator": @(QueryValue::Str(op.drop_validator.clone())),
                "existed": false,
            })));
        }

        let existed = self
            .shamir
            .drop_validator_as(&op.drop_validator, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "dropped_validator": @(QueryValue::Str(op.drop_validator.clone())),
            "existed": @(QueryValue::Bool(existed)),
        })))
    }

    pub(super) async fn handle_rename_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameValidator(op) = batch_op else {
            unreachable!("handle_rename_validator called with non-RenameValidator op");
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

        self.shamir
            .authorize_access(&self.actor, &ResourcePath::FunctionNamespace, Action::Write)
            .await
            .map_err(err_access)?;
        self.shamir
            .rename_validator_as(&op.rename_validator, &op.to, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "renamed_validator": @(QueryValue::Str(op.rename_validator.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
        })))
    }

    /// Ordinary dispatch reserves the target after ACL. Migration apply calls
    /// the handlers below directly while holding its own migration gate.
    pub(super) async fn dispatch_validator_binding(
        &self,
        op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        match op {
            BatchOp::BindValidator(_) => self.bind_validator(op, true).await,
            BatchOp::UnbindValidator(_) => self.unbind_validator(op, true).await,
            _ => unreachable!("dispatch_validator_binding called with a non-binding op"),
        }
    }

    pub(super) async fn handle_bind_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        self.bind_validator(batch_op, false).await
    }

    async fn bind_validator(
        &self,
        batch_op: &BatchOp,
        reserve_target: bool,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::BindValidator(op) = batch_op else {
            unreachable!("handle_bind_validator called with non-BindValidator op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let path = ResourcePath::table(&op.db, &op.repo, &op.table);
        let _reservation = if reserve_target {
            self.authorize_admin_target_write(
                explicit_migration_target(batch_op),
                &path,
                Action::Write,
            )
            .await?
        } else {
            // The migration runner already owns the fence, but still needs ACL.
            self.shamir
                .authorize_access(&self.actor, &path, Action::Write)
                .await
                .map_err(|error| BatchError::query_coded("", "access_denied", error.to_string()))?;
            None
        };
        self.shamir
            .bind_validator_as(
                &op.db,
                &op.repo,
                &op.table,
                &op.bind_validator,
                op.ops.clone(),
                op.priority,
                self.actor.clone(),
            )
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "bound_validator": @(QueryValue::Str(op.bind_validator.clone())),
            "table": @(QueryValue::Str(op.table.clone())),
        })))
    }

    pub(super) async fn handle_unbind_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        self.unbind_validator(batch_op, false).await
    }

    async fn unbind_validator(
        &self,
        batch_op: &BatchOp,
        reserve_target: bool,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::UnbindValidator(op) = batch_op else {
            unreachable!("handle_unbind_validator called with non-UnbindValidator op");
        };

        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let path = ResourcePath::table(&op.db, &op.repo, &op.table);
        let _reservation = if reserve_target {
            self.authorize_admin_target_write(
                explicit_migration_target(batch_op),
                &path,
                Action::Write,
            )
            .await?
        } else {
            // The migration runner already owns the fence, but still needs ACL.
            self.shamir
                .authorize_access(&self.actor, &path, Action::Write)
                .await
                .map_err(|error| BatchError::query_coded("", "access_denied", error.to_string()))?;
            None
        };
        let removed = self
            .shamir
            .unbind_validator_as(
                &op.db,
                &op.repo,
                &op.table,
                &op.unbind_validator,
                self.actor.clone(),
            )
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "unbound_validator": @(QueryValue::Str(op.unbind_validator.clone())),
            "table": @(QueryValue::Str(op.table.clone())),
            "existed": @(QueryValue::Bool(removed)),
        })))
    }

    pub(super) async fn handle_list_validators(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::ListValidators(op) = batch_op else {
            unreachable!("handle_list_validators called with non-ListValidators op");
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

        // Auth: Read on the target Table.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::Table {
                    db: op.db.clone(),
                    store: op.repo.clone(),
                    table: op.list_validators.clone(),
                },
                Action::Read,
            )
            .await
            .map_err(err_access)?;
        let bindings = self
            .shamir
            .list_validator_bindings(&op.db, &op.repo, &op.list_validators)
            .await
            .map_err(|e| err(e.to_string()))?;
        let bindings_qv: Vec<QueryValue> = bindings
            .iter()
            .map(|b| {
                mpack!({
                    "validator_id": @(QueryValue::Str(b.validator_id.to_string())),
                    "priority": @(QueryValue::Int(b.priority as i64)),
                })
            })
            .collect();
        Ok(admin_result(mpack!({
            "validators": @(QueryValue::List(bindings_qv)),
            "table": @(QueryValue::Str(op.list_validators.clone())),
        })))
    }
}
