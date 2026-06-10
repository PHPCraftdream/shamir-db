//! Admin handlers: CreateValidator, DropValidator, RenameValidator, BindValidator, UnbindValidator, ListValidators.

use base64::Engine;
use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
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
        Ok(admin_result(json!({
            "created_validator": op.create_validator,
            "id": id.to_string(),
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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::FunctionNamespace,
                Action::Delete,
            )
            .await
            .map_err(err_access)?;
        let existed = self
            .shamir
            .drop_validator_as(&op.drop_validator, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"dropped_validator": op.drop_validator, "existed": existed}),
        ))
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
        Ok(admin_result(
            json!({"renamed_validator": op.rename_validator, "to": op.to}),
        ))
    }

    pub(super) async fn handle_bind_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::BindValidator(op) = batch_op else {
            unreachable!("handle_bind_validator called with non-BindValidator op");
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

        // Auth: Write on the target Table (binding changes the
        // table's write behaviour).
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::Table {
                    db: op.db.clone(),
                    store: op.repo.clone(),
                    table: op.table.clone(),
                },
                Action::Write,
            )
            .await
            .map_err(err_access)?;
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
        Ok(admin_result(json!({
            "bound_validator": op.bind_validator,
            "table": op.table,
        })))
    }

    pub(super) async fn handle_unbind_validator(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::UnbindValidator(op) = batch_op else {
            unreachable!("handle_unbind_validator called with non-UnbindValidator op");
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

        // Auth: Write on the target Table.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::Table {
                    db: op.db.clone(),
                    store: op.repo.clone(),
                    table: op.table.clone(),
                },
                Action::Write,
            )
            .await
            .map_err(err_access)?;
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
        Ok(admin_result(json!({
            "unbound_validator": op.unbind_validator,
            "table": op.table,
            "existed": removed,
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
        let bindings_json: Vec<serde_json::Value> = bindings
            .iter()
            .map(|b| {
                json!({
                    "validator_id": b.validator_id.to_string(),
                    "priority": b.priority,
                })
            })
            .collect();
        Ok(admin_result(json!({
            "validators": bindings_json,
            "table": op.list_validators,
        })))
    }
}
