//! Admin handlers: CreateFunction, DropFunction, RenameFunction, CreateFunctionFolder.

use base64::Engine;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, validate_name_component};

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::CreateFunction(op) = batch_op else {
            unreachable!("handle_create_function called with non-CreateFunction op");
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
        if let Some(ref source) = op.source {
            self.shamir
                .create_function_from_source_as(
                    &op.create_function,
                    source,
                    op.replace,
                    self.actor.clone(),
                )
                .await
                .map_err(|e| err(e.to_string()))?;
        } else if let Some(ref wasm_b64) = op.wasm {
            let wasm_bytes = base64::engine::general_purpose::STANDARD
                .decode(wasm_b64)
                .map_err(|e| err(format!("invalid base64 wasm: {}", e)))?;
            self.shamir
                .create_function_from_wasm_as(
                    &op.create_function,
                    &wasm_bytes,
                    op.replace,
                    self.actor.clone(),
                )
                .await
                .map_err(|e| err(e.to_string()))?;
        } else {
            return Err(err(
                "create_function requires either 'source' or 'wasm'".to_string()
            ));
        }
        Ok(admin_result(mpack!({
            "created_function": @(QueryValue::Str(op.create_function.clone())),
        })))
    }

    pub(super) async fn handle_drop_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::DropFunction(op) = batch_op else {
            unreachable!("handle_drop_function called with non-DropFunction op");
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

        // if_exists early-exit: function not registered → no-op.
        if op.if_exists && !self.shamir.functions().contains(&op.drop_function) {
            return Ok(admin_result(mpack!({
                "dropped_function": @(QueryValue::Str(op.drop_function.clone())),
                "existed": false,
            })));
        }

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::Function {
                    name: op.drop_function.clone(),
                },
                Action::Delete,
            )
            .await
            .map_err(err_access)?;

        // Phase D.3 — bound-validator drop guard.
        //
        // Refuse to drop a function that is bound as a validator on any table.
        // Functions and validators share the FunctionNamespace; a function
        // whose name collides with a bound validator cannot be dropped while
        // the binding is live (dropping would leave the binding referencing a
        // ghost).  The check queries the validator registry: if a validator
        // with the same name exists and `is_bound`, reject.
        if let Some(vid) = self.shamir.validators().id_for_name(&op.drop_function) {
            if self.shamir.validators().is_bound(&vid) {
                let bound_tables = self.shamir.validators().bound_tables(&vid);
                return Err(err_code(
                    "drop_refused_bound",
                    format!(
                        "cannot drop function '{}': still bound as a validator on tables: {}",
                        op.drop_function,
                        bound_tables.join(", ")
                    ),
                ));
            }
        }

        let existed = self
            .shamir
            .drop_function_as(&op.drop_function, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "dropped_function": @(QueryValue::Str(op.drop_function.clone())),
            "existed": @(QueryValue::Bool(existed)),
        })))
    }

    pub(super) async fn handle_rename_function(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameFunction(op) = batch_op else {
            unreachable!("handle_rename_function called with non-RenameFunction op");
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
                &ResourcePath::Function {
                    name: op.rename_function.clone(),
                },
                Action::Write,
            )
            .await
            .map_err(err_access)?;
        self.shamir
            .rename_function_as(&op.rename_function, &op.to, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "renamed_function": @(QueryValue::Str(op.rename_function.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
        })))
    }

    pub(super) async fn handle_create_function_folder(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::CreateFunctionFolder(op) = batch_op else {
            unreachable!("handle_create_function_folder called with non-CreateFunctionFolder op");
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

        // Validate path segments.
        if op.create_function_folder.is_empty() {
            return Err(err("function folder path must not be empty".to_string()));
        }
        for segment in &op.create_function_folder {
            validate_name_component(segment, "folder segment")?;
        }

        // Auth: Create on the parent folder or FunctionNamespace
        // (if only one segment).
        let parent_path = if op.create_function_folder.len() == 1 {
            ResourcePath::FunctionNamespace
        } else {
            ResourcePath::FunctionFolder {
                path: op.create_function_folder[..op.create_function_folder.len() - 1].to_vec(),
            }
        };
        self.shamir
            .authorize_access(&self.actor, &parent_path, Action::Create)
            .await
            .map_err(err_access)?;

        // mkdir -p: create all prefix folders that don't yet exist.
        let created = self
            .shamir
            .create_function_folder_as(&op.create_function_folder, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;

        Ok(admin_result(mpack!({
            "created_function_folder": @(QueryValue::List(
                op.create_function_folder.iter().map(|s| QueryValue::Str(s.clone())).collect(),
            )),
            "created": @(QueryValue::List(
                created.into_iter().map(QueryValue::Str).collect(),
            )),
        })))
    }

    pub(super) async fn handle_rename_function_folder(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameFunctionFolder(op) = batch_op else {
            unreachable!("handle_rename_function_folder called with non-RenameFunctionFolder op");
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

        // Validate both paths: non-empty and each segment well-formed.
        if op.rename_function_folder.is_empty() {
            return Err(err(
                "rename_function_folder source path must not be empty".to_string()
            ));
        }
        if op.to.is_empty() {
            return Err(err(
                "rename_function_folder destination path must not be empty".to_string(),
            ));
        }
        for segment in &op.rename_function_folder {
            validate_name_component(segment, "source folder segment")?;
        }
        for segment in &op.to {
            validate_name_component(segment, "destination folder segment")?;
        }

        // Auth: Write on the source folder (it is itself a resource).
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::FunctionFolder {
                    path: op.rename_function_folder.clone(),
                },
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // Auth: Create on the destination parent (mirror create_function_folder).
        let parent_path = if op.to.len() == 1 {
            ResourcePath::FunctionNamespace
        } else {
            ResourcePath::FunctionFolder {
                path: op.to[..op.to.len() - 1].to_vec(),
            }
        };
        self.shamir
            .authorize_access(&self.actor, &parent_path, Action::Create)
            .await
            .map_err(err_access)?;

        self.shamir
            .rename_function_folder_as(&op.rename_function_folder, &op.to, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;

        Ok(admin_result(mpack!({
            "renamed_function_folder": @(QueryValue::List(
                op.rename_function_folder.iter().map(|s| QueryValue::Str(s.clone())).collect(),
            )),
            "to": @(QueryValue::List(
                op.to.iter().map(|s| QueryValue::Str(s.clone())).collect(),
            )),
        })))
    }
}
