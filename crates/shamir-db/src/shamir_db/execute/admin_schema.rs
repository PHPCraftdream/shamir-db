//! Admin handlers: SetTableSchema, AddSchemaRule, RemoveSchemaRule, GetTableSchema.
//!
//! All mutating schema ops are gated by `Action::Write` on the table
//! resource (same as ALTER TABLE). `GetTableSchema` is gated by
//! `Action::Read` (introspection).
//!
//! # Phase A skeleton (TODO — handler implementation)
//!
//! All four handlers are **authz-only stubs**: they enforce the correct
//! permission gate (`Action::Write` / `Action::Read`) but return a
//! hardcoded `{ok: true}` without persisting the schema, interning
//! paths, bumping `schema_version`, or compiling/binding a
//! `SchemaValidator`. Specifically:
//!
//! - `SetTableSchema`: accepts `expected_version` but never checks it
//!   (optimistic concurrency is not enforced).
//! - No handler writes `schema` or `schema_validator_id` to catalogue
//!   records, so `boot_compile_schemas` will never restore a schema
//!   after restart.
//! - DDL ops silently succeed with zero effect on the engine state.
//!
//! **Follow-up task:** implement catalogue persistence, version
//! checking, interning, and validator compilation in Phase A server
//! execution (tracked by task #196 sub-deliverable: DDL handler
//! implementation).

use crate::access::{Action, ResourcePath};
use crate::query::admin::{
    AddSchemaRuleOp, GetTableSchemaOp, RemoveSchemaRuleOp, SetTableSchemaOp,
};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

impl ShamirAdminExecutor {
    pub(super) async fn handle_set_table_schema(
        &self,
        op: &SetTableSchemaOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Authz: Action::Write on the table (same pattern as ALTER).
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.set_table_schema.clone(),
                ),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // TODO(Phase A — server execution): read current catalogue record,
        // check expected_version (optimistic concurrency), intern paths,
        // persist schema + bump schema_version, compile & bind validator.
        Ok(admin_result(mpack!({
            "set_table_schema": @(QueryValue::Str(op.set_table_schema.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "ok": true,
        })))
    }

    pub(super) async fn handle_add_schema_rule(
        &self,
        op: &AddSchemaRuleOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Authz: Action::Write on the table.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.add_schema_rule.clone(),
                ),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // TODO(Phase A — server execution): upsert rule by path in catalogue,
        // bump schema_version, recompile validator.
        Ok(admin_result(mpack!({
            "add_schema_rule": @(QueryValue::Str(op.add_schema_rule.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "ok": true,
        })))
    }

    pub(super) async fn handle_remove_schema_rule(
        &self,
        op: &RemoveSchemaRuleOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Authz: Action::Write on the table.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.remove_schema_rule.clone(),
                ),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        // TODO(Phase A — server execution): remove rule by path in catalogue,
        // bump schema_version, recompile validator.
        Ok(admin_result(mpack!({
            "remove_schema_rule": @(QueryValue::Str(op.remove_schema_rule.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "ok": true,
        })))
    }

    pub(super) async fn handle_get_table_schema(
        &self,
        op: &GetTableSchemaOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Authz: Action::Read on the table (introspection).
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.get_table_schema.clone(),
                ),
                Action::Read,
            )
            .await
            .map_err(err_access)?;

        // TODO(Phase A — server execution): read schema from catalogue,
        // de-intern paths, return schema + schema_version.
        Ok(admin_result(mpack!({
            "get_table_schema": @(QueryValue::Str(op.get_table_schema.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "schema": @(QueryValue::List(Vec::new())),
            "schema_version": 0i64,
        })))
    }
}
