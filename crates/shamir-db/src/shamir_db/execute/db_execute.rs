//! `impl ShamirDb { execute, execute_as }`.

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{execute_batch, BatchError, BatchOp, BatchRequest, BatchResponse};

use super::super::shamir_db::ShamirDb;
use super::admin_dispatch::ShamirAdminExecutor;
use super::function_invoker::ShamirFunctionInvoker;
use super::table_resolver::DbTableResolver;

impl ShamirDb {
    /// Execute a batch request against a specific database.
    pub async fn execute(
        &self,
        db_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        self.execute_as(Actor::System, db_name, request).await
    }

    /// Execute a batch request with an explicit [`Actor`] for access control.
    ///
    /// This is the principal-aware entry point called by the server with the
    /// authenticated session's actor. The convenience [`execute`] delegates
    /// here with `Actor::System` (admin bypass) for backward compatibility.
    pub async fn execute_as(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        self.authorize_access(
            &actor,
            &ResourcePath::Database {
                db: db_name.to_string(),
            },
            Action::Read,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;

        // Per-op authorization: each data op is checked against its TARGET
        // table (admin/DDL ops carry no table_ref and are authorized in
        // execute_admin). authorize_access traverses the db/store ancestors,
        // so the table path covers the whole chain. System bypasses.
        for entry in request.queries.values() {
            if let Some(tref) = entry.op.table_ref() {
                let action = match &entry.op {
                    BatchOp::Read(_) => Action::Read,
                    BatchOp::Insert(_) => Action::Create,
                    BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
                    BatchOp::Delete(_) => Action::Delete,
                    _ => Action::Write,
                };
                let path = ResourcePath::Table {
                    db: db_name.to_string(),
                    store: tref.repo.clone(),
                    table: tref.table.clone(),
                };
                self.authorize_access(&actor, &path, action)
                    .await
                    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
            }
        }

        let resolver = DbTableResolver {
            db,
            validators: self.validators().clone(),
        };
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        };

        let invoker = ShamirFunctionInvoker {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        };
        execute_batch(
            request,
            &resolver,
            Some(&admin),
            Some(&invoker),
            actor,
            db_name,
        )
        .await
    }
}
