//! `impl ShamirDb { execute, execute_as }`.

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{
    collect_required_access, execute_batch, BatchError, BatchRequest, BatchResponse,
};

use super::super::shamir_db::ShamirDb;
use super::admin_dispatch::ShamirAdminExecutor;
use super::ambient_interner::attach_interner_delta;
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
        //
        // `collect_required_access` recursively walks the WHOLE query tree,
        // including nested `Batch`/`ForEach` bodies at any depth — a flat,
        // one-level walk over `request.queries.values()` would see `None`
        // for `Batch`/`ForEach` (they have no `table_ref()`) and silently
        // skip authorizing whatever tables their nested body actually
        // touches, letting an actor bypass a forbidden table's ACL by
        // wrapping the op in a top-level `Batch`/`ForEach` (the #660-class
        // bug, but for authorization). See `collect_required_access`'s doc
        // comment (mirrors `distinct_repos`'s recursive-walk precedent).
        for (action, path) in collect_required_access(&request.queries, db_name) {
            self.authorize_access(&actor, &path, action)
                .await
                .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        }

        let resolver = DbTableResolver {
            db: db.clone(),
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
        let mut response = execute_batch(
            request,
            &resolver,
            Some(&admin),
            Some(&invoker),
            actor,
            db_name,
        )
        .await?;

        // Ambient interner epoch-delta sync (Stage 5-wire Part A): attach the
        // server's per-repo delta for each epoch the client advertised. `db`
        // is cloned above for the resolver; we reuse the original here.
        // Errors are non-fatal (batch already succeeded) — logged + swallowed.
        if !request.interner_epochs.is_empty() {
            if let Err(e) = attach_interner_delta(&mut response, request, &db).await {
                log::debug!("ambient interner delta attach skipped: {e}");
            }
        }

        Ok(response)
    }
}
