//! `impl ShamirDb { execute, execute_as }`.

use shamir_query_types::batch::tree_visitor::BatchTreeVisitor;
use shamir_query_types::batch::{BatchOp, ExecPolicy};

use crate::access::Actor;
use crate::query::batch::{execute_batch, Authorized, BatchError, BatchRequest, BatchResponse};
use crate::shamir_db::ports::RepoAdminObserver;

use super::super::shamir_db::ShamirDb;
use super::admin_dispatch::ShamirAdminExecutor;
use super::ambient_interner::attach_interner_delta;
use super::db_tx::tx_read_quarantine_refusal_for_request;
use super::function_invoker::ShamirFunctionInvoker;
use super::quarantine_cache::QuarantineSnapshot;
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
        let ambient_actor = auth.actor().clone();
        let mut authorized_interner_repos = Vec::new();
        for repo_name in request.interner_epochs.keys() {
            // Boxed: keeps this rarely-taken await out of the batch future's inline state.
            if Box::pin(self.authorize_access(
                &ambient_actor,
                &crate::access::ResourcePath::store(db_name, repo_name),
                crate::access::Action::Read,
            ))
            .await
            .is_ok()
            {
                authorized_interner_repos.push(repo_name.clone());
            }
        }

        // S5c-X PR8b: ordinary READS of a quarantined (`resync_required`)
        // repo fail closed. Fast path is one `Relaxed` load (empty cache ⇒
        // no ops are touched, no allocation). Writes and admin ops are NOT
        // gated — status/DESCRIBE stay readable by design.
        //
        // The gate is a single call to the shared SYNCHRONOUS helper
        // (`tx_read_quarantine_refusal_for_request` in `db_tx.rs` — no
        // `.await` inside). Keeping the whole-query-tree walk OUT of this
        // async fn body is what matters for stack safety: any code inlined
        // here becomes fields of this fn's generated `Future` state machine,
        // and under nested `Batch`/`ForEach` recursion that future is held
        // on the stack once per recursion level (Round-12 Windows overflow).
        // The sync helper's locals live in its own plain stack frame and the
        // returned `Option<BatchError>` is dropped before `execute_batch`.
        if let Some(err) = tx_read_quarantine_refusal_for_request(self, db_name, request) {
            return Err(err);
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

        // Interner access has already been pre-authorized; deltas attach only
        // after execution, so a denied repository cannot cause batch side effects.
        if !request.interner_epochs.is_empty() {
            Box::pin(attach_interner_delta(
                &mut response,
                &authorized_interner_repos,
                request,
                &db,
            ))
            .await;
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
#[inline(never)]
pub(crate) fn quarantined_read_hit(
    db_name: &str,
    request: &BatchRequest,
    snapshot: &QuarantineSnapshot,
) -> Option<(String, String, String)> {
    for visit in BatchTreeVisitor::new(&request.queries) {
        let BatchOp::Read(query) = &visit.entry.op else {
            continue;
        };
        let repo = &query.from.repo;
        if let Some(reason) = snapshot
            .entries
            .get(db_name)
            .and_then(|repos| repos.get(repo))
        {
            return Some((visit.alias.to_string(), repo.clone(), reason.to_string()));
        }
    }
    None
}
