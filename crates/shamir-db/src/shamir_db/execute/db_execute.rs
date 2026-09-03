//! `impl ShamirDb { execute, execute_as }`.

use crate::access::Actor;
use crate::query::batch::{execute_batch, Authorized, BatchError, BatchRequest, BatchResponse};

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
    ///
    /// Authorization (DB visibility + every op's `required_access`,
    /// recursively through nested `Batch`/`ForEach` bodies — see
    /// `Authorized::authorize`'s doc) now happens INSIDE the type-level
    /// seam (#1199): `self` is passed as the [`crate::query::batch::AccessGate`],
    /// and `execute_batch` structurally cannot run without the resulting
    /// [`Authorized`] token.
    pub async fn execute_as(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        let auth = Authorized::authorize(request, actor, db_name, self).await?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;

        let resolver = DbTableResolver {
            db: db.clone(),
            validators: self.validators().clone(),
        };
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: auth.actor().clone(),
        };

        let invoker = ShamirFunctionInvoker {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        };
        let mut response = execute_batch(auth, &resolver, Some(&admin), Some(&invoker)).await?;

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
