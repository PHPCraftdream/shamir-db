//! Admin handlers: CreateFunction, DropFunction, RenameFunction, CreateFunctionFolder.

use base64::Engine;
use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;

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
        Ok(admin_result(
            json!({"created_function": op.create_function}),
        ))
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
        let existed = self
            .shamir
            .drop_function_as(&op.drop_function, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"dropped_function": op.drop_function, "existed": existed}),
        ))
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
        Ok(admin_result(
            json!({"renamed_function": op.rename_function, "to": op.to}),
        ))
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

        Ok(admin_result(json!({
            "created_function_folder": op.create_function_folder,
            "created": created,
        })))
    }
}
