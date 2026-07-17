//! `impl ShamirDb { tx_begin, tx_begin_as, tx_execute, tx_execute_as, tx_commit, tx_commit_as }`.
//!
//! Phase B — interactive (multi-call) transactions.
//!
//! These facade methods expose the engine's interactive-tx glue
//! (`open_interactive_tx` / `execute_in_open_tx` / `commit_interactive_tx`)
//! to the server, which owns the live-tx registry (it depends on `shamir-tx`
//! directly). The facade resolves the db/repo and builds the same
//! resolver + admin pair `execute` uses, then drives one lifecycle step. The
//! `TxContext` / `SnapshotGuard` flow back to the server registry via the
//! engine re-export (`crate::engine::tx::*`) — the same concrete `shamir-tx`
//! types the server names. See `docs/dev-artifacts/roadmap/PHASE_B_INTERACTIVE_TX.md` §5.

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{
    collect_required_access, commit_interactive_tx, execute_in_open_tx, open_interactive_tx,
    BatchError, BatchRequest, BatchResponse, TransactionInfo,
};

use rustc_hash::FxHashMap;

use super::super::shamir_db::ShamirDb;
use super::admin_dispatch::ShamirAdminExecutor;
use super::function_invoker::ShamirFunctionInvoker;
use super::table_resolver::DbTableResolver;

impl ShamirDb {
    /// BEGIN: open an interactive tx against `db_name`/`repo_name`. Returns
    /// the live `TxContext` + its `SnapshotGuard` for the caller (the server
    /// registry) to park between round-trips.
    pub async fn tx_begin(
        &self,
        db_name: &str,
        repo_name: &str,
        isolation: &str,
    ) -> Result<
        (
            crate::engine::tx::TxContext,
            crate::engine::tx::SnapshotGuard,
        ),
        BatchError,
    > {
        self.tx_begin_as(Actor::System, db_name, repo_name, isolation)
            .await
    }

    /// BEGIN with an explicit [`Actor`].
    pub async fn tx_begin_as(
        &self,
        actor: Actor,
        db_name: &str,
        repo_name: &str,
        isolation: &str,
    ) -> Result<
        (
            crate::engine::tx::TxContext,
            crate::engine::tx::SnapshotGuard,
        ),
        BatchError,
    > {
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
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
                code: None,
            })?;
        let iso = match isolation {
            "serializable" => crate::engine::tx::IsolationLevel::Serializable,
            _ => crate::engine::tx::IsolationLevel::Snapshot,
        };
        let (mut tx, guard) =
            open_interactive_tx(&repo, iso)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: String::new(),
                    message: format!("begin_tx: {}", e),
                    code: None,
                })?;
        tx.set_actor(actor);
        Ok((tx, guard))
    }

    /// EXECUTE: run one batch inside an already-open interactive tx, WITHOUT
    /// committing. The `BatchResponse` carries `transaction: None` (the tx is
    /// still open). The single-repo guard is enforced inside the engine glue;
    /// the caller additionally asserts the batch targets the handle's repo.
    pub async fn tx_execute(
        &self,
        db_name: &str,
        request: &BatchRequest,
        tx: &mut crate::engine::tx::TxContext,
    ) -> Result<BatchResponse, BatchError> {
        self.tx_execute_as(Actor::System, db_name, request, tx)
            .await
    }

    /// EXECUTE with an explicit [`Actor`].
    pub async fn tx_execute_as(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
        tx: &mut crate::engine::tx::TxContext,
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

        // Per-op DML authorization (mirrors execute_as).
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
        //
        // ACL inline cache: within a single tx_execute_as call every
        // (path, action) pair resolves to the same answer — the actor,
        // the ACL tree, and the requested resource do not change between
        // ops. The first call pays the full async traversal; subsequent
        // calls for the same key hit a HashMap and cost ~50 ns. The cache
        // is stack-local and dropped at function exit (no cross-call sharing).
        // It stays correct over the recursively-collected list — it's keyed
        // on `(ResourcePath, Action)`, unrelated to how the list was gathered.
        let mut acl_cache: FxHashMap<(ResourcePath, Action), bool> = FxHashMap::default();
        for (action, path) in collect_required_access(&request.queries, db_name) {
            let key = (path.clone(), action);
            let allowed = if let Some(&cached) = acl_cache.get(&key) {
                cached
            } else {
                let ok = self.authorize_access(&actor, &path, action).await.is_ok();
                acl_cache.insert(key, ok);
                ok
            };
            if !allowed {
                return Err(BatchError::query_coded(
                    "",
                    "access_denied",
                    format!("access denied: {:?} on {:?}", action, path),
                ));
            }
        }

        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;
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
        execute_in_open_tx(
            request,
            &resolver,
            Some(&admin),
            Some(&invoker),
            &actor,
            db_name,
            tx,
        )
        .await
    }

    /// COMMIT: run the Phase-A commit pipeline on a parked interactive tx and
    /// map the outcome to a wire [`TransactionInfo`] — `committed` (with the
    /// inherited `materialized` flag) on success, `aborted` with a reason on
    /// a commit-time conflict/violation. Mirrors the mapping the single-batch
    /// `execute_transactional` performs.
    pub async fn tx_commit(
        &self,
        db_name: &str,
        repo_name: &str,
        tx: crate::engine::tx::TxContext,
    ) -> Result<TransactionInfo, BatchError> {
        self.tx_commit_as(Actor::System, db_name, repo_name, tx)
            .await
    }

    /// COMMIT with an explicit [`Actor`].
    pub async fn tx_commit_as(
        &self,
        actor: Actor,
        db_name: &str,
        repo_name: &str,
        tx: crate::engine::tx::TxContext,
    ) -> Result<TransactionInfo, BatchError> {
        self.authorize_access(
            &actor,
            &ResourcePath::Database {
                db: db_name.to_string(),
            },
            Action::Write,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
                code: None,
            })?;
        let tx_id = tx.tx_id.0;
        match commit_interactive_tx(&repo, tx).await {
            Ok(outcome) => Ok(TransactionInfo::committed(
                outcome.tx_id,
                outcome.snapshot_version,
                outcome.commit_version,
                outcome.materialized(),
            )),
            Err(commit_err) => {
                let reason = match commit_err {
                    crate::engine::tx::CommitError::SsiConflict { .. } => "tx_conflict".to_string(),
                    crate::engine::tx::CommitError::PhantomConflict { .. } => {
                        "tx_conflict".to_string()
                    }
                    crate::engine::tx::CommitError::Wounded { .. } => "tx_conflict".to_string(),
                    crate::engine::tx::CommitError::UniqueViolation { .. } => {
                        "unique_violation".to_string()
                    }
                    crate::engine::tx::CommitError::Storage(e) => format!("storage: {}", e),
                    crate::engine::tx::CommitError::Expired { elapsed, max } => {
                        format!("tx expired: elapsed {:?} > max {:?}", elapsed, max)
                    }
                };
                Ok(TransactionInfo::aborted(tx_id, reason))
            }
        }
    }
}
