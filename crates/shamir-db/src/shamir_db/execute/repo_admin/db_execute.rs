//! `impl ShamirDb { execute, execute_as }`.

use shamir_query_types::batch::tree_visitor::BatchTreeVisitor;
use shamir_query_types::batch::{BatchOp, ExecPolicy};

use crate::access::Actor;
use crate::query::batch::{execute_batch, Authorized, BatchError, BatchRequest, BatchResponse};
use crate::shamir_db::ports::RepoAdminObserver;
use shamir_query_types::batch::ResyncRequiredDetail;

use super::super::shamir_db::ShamirDb;
use super::admin_dispatch::ShamirAdminExecutor;
use super::ambient_interner::attach_interner_delta;
use super::function_invoker::ShamirFunctionInvoker;
use super::quarantine_cache::{skip_read_quarantine_for_test, ResyncQuarantineCache};
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
        self.execute_with_policy(actor, db_name, request, ExecPolicy::EMBEDDED)
            .await
    }

    /// Execute with an authenticated actor and a server-owned resource policy.
    pub async fn execute_with_policy(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
        policy: ExecPolicy,
    ) -> Result<BatchResponse, BatchError> {
        self.execute_with_policy_and_repo_observer(actor, db_name, request, policy, None)
            .await
    }

    /// Execute under a server-owned policy and observe successful repo durability changes.
    /// Each successful ALTER calls the observer synchronously after persistence and live swap.
    pub async fn execute_with_policy_and_repo_observer(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
        policy: ExecPolicy,
        repo_admin_observer: Option<std::sync::Arc<dyn RepoAdminObserver>>,
    ) -> Result<BatchResponse, BatchError> {
        let auth = Authorized::authorize(request, actor, db_name, self)
            .await?
            .with_policy(policy);

        // S5c-X PR8b: ordinary READS of a quarantined (`resync_required`)
        // repo fail closed. Fast path is one `Relaxed` load (empty cache ⇒
        // no ops are touched, no allocation). Writes and admin ops are NOT
        // gated — status/DESCRIBE stay readable by design.
        let skip_seam = skip_read_quarantine_for_test();
        if !skip_seam && !self.resync_quarantine.is_empty() {
            if let Some((alias, repo, reason)) =
                quarantined_read_hit(db_name, request, &self.resync_quarantine)
            {
                return Err(BatchError::ResyncRequired {
                    alias,
                    detail: Box::new(ResyncRequiredDetail {
                        db: db_name.to_string(),
                        repo,
                        reason,
                    }),
                });
            }
        }

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
            repo_admin_observer,
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

/// Walk the whole query tree (nested `Batch`/`ForEach` bodies included —
/// mirroring the authorization walk) and return the first READ op whose
/// target repo is quarantined, as `(alias, repo, reason)`. Only data reads
/// (`BatchOp::Read` — the `Action::Read`-classified variant) are checked;
/// writes and admin/status/DESCRIBE ops pass through. Shared by the batch
/// (above) and the interactive-tx gate in `db_tx.rs`.
pub(crate) fn quarantined_read_hit(
    db_name: &str,
    request: &BatchRequest,
    cache: &ResyncQuarantineCache,
) -> Option<(String, String, String)> {
    for visit in BatchTreeVisitor::new(&request.queries) {
        let BatchOp::Read(query) = &visit.entry.op else {
            continue;
        };
        let repo = &query.from.repo;
        if let Some(reason) = cache.reason_for(db_name, repo) {
            return Some((visit.alias.to_string(), repo.clone(), reason));
        }
    }
    None
}
