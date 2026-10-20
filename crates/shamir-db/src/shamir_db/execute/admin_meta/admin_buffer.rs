//! Admin handlers: GetBufferConfig, SetBufferConfig, AlterBufferConfig.

use crate::access::{Action, ResourcePath};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, apply_patch, dto_from_storage, storage_from_dto, to_qv};

impl ShamirAdminExecutor {
    pub(super) async fn handle_get_buffer_config(
        &self,
        op: &crate::query::admin::GetBufferConfigOp,
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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.get_buffer_config.clone(),
                ),
                Action::Read,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.get_buffer_config)
            .await
            .map_err(|e| err(e.to_string()))?;
        let cfg = table
            .get_buffer_config()
            .await
            .map_err(|e| err(e.to_string()))?;
        let payload = match cfg {
            Some(c) => mpack!({
                "table": @(QueryValue::Str(op.get_buffer_config.clone())),
                "repo": @(QueryValue::Str(op.repo.clone())),
                "config": @(to_qv(&dto_from_storage(&c))),
            }),
            None => mpack!({
                "table": @(QueryValue::Str(op.get_buffer_config.clone())),
                "repo": @(QueryValue::Str(op.repo.clone())),
                "config": null,
            }),
        };
        Ok(admin_result(payload))
    }

    pub(super) async fn handle_set_buffer_config(
        &self,
        op: &crate::query::admin::SetBufferConfigOp,
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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.set_buffer_config.clone(),
                ),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.set_buffer_config)
            .await
            .map_err(|e| err(e.to_string()))?;
        let storage_cfg = storage_from_dto(&op.config);
        table
            .set_buffer_config(&storage_cfg)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "set_buffer_config": @(QueryValue::Str(op.set_buffer_config.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "config": @(to_qv(&dto_from_storage(&storage_cfg))),
        })))
    }

    pub(super) async fn handle_alter_buffer_config(
        &self,
        op: &crate::query::admin::AlterBufferConfigOp,
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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.alter_buffer_config.clone(),
                ),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.alter_buffer_config)
            .await
            .map_err(|e| err(e.to_string()))?;
        let patch = op.patch.clone();
        let updated = table
            .alter_buffer_config(|c| apply_patch(c, &patch))
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "alter_buffer_config": @(QueryValue::Str(op.alter_buffer_config.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "config": @(to_qv(&dto_from_storage(&updated))),
        })))
    }
}
