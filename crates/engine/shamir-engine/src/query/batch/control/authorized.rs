//! Type-level authorization seam — [`Authorized`] + [`AccessGate`] (#1199).
//!
//! **Problem this closes.** Before this module existed, `execute_batch`
//! and `execute_in_open_tx` took a raw `&BatchRequest` + `Actor` pair —
//! plain data, with nothing stopping a caller from invoking them WITHOUT
//! ever having run an access-control check. The real check
//! (`ShamirDb::authorize_access` against the live ACL tree) lived entirely
//! in `shamir-db`'s `execute_as`/`tx_execute_as` facades, as a convention
//! ("call `authorize_access` first") rather than a structural requirement.
//! Nothing in the engine's own types enforced that convention — a new
//! server route, a WASM host bridge, or an internal job could call
//! `execute_batch` directly and silently run as a full-power bypass. See
//! `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-engine/security-crypto.md`
//! finding #3.
//!
//! **Why a trait-injected token, not a direct dependency on `shamir-db`.**
//! The ACL tree itself (users/groups/`ResourceMeta`) is owned by
//! `ShamirDb`, which lives in `shamir-db` — a crate ABOVE `shamir-engine`
//! in the dependency graph (`shamir-db` depends on `shamir-engine`, never
//! the reverse). The engine cannot call `authorize_access` directly
//! without an illegal reverse dependency. [`AccessGate`] is the same
//! inversion-of-control seam already used for `crate::query::batch::1` /
//! `crate::query::batch::1` / `crate::query::batch::1`
//! (`executor_traits.rs`): a thin trait defined here, implemented by
//! `ShamirDb` in `shamir-db` by delegating to `authorize_access`.
//!
//! **What the token actually guarantees.** [`Authorized`]'s fields are
//! private and the type has no `Clone`/`Default`/serde impl — the ONLY
//! public way to produce one (outside `#[cfg(test)]`) is
//! [`Authorized::authorize`], which runs the full check against an
//! injected [`AccessGate`] before minting the value. This defends against
//! the "forgot to call the wrapper" class of bug: a caller with no
//! `Authorized` value has nothing to pass to `execute_batch`/
//! `execute_in_open_tx`, so the omission is a compile error rather than a
//! silent unauthorized execution. It does NOT defend against a caller who
//! deliberately writes a rubber-stamp `AccessGate` impl that always
//! returns `Ok` — that remains a visible, auditable, explicit choice in
//! the code (e.g. bench/test harnesses do exactly this), not a silent
//! omission.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::query::batch::exec::batch_validate::tables_created_in_batch;
use crate::query::batch::ExecScope;
use async_trait::async_trait;
use shamir_query_types::batch::{
    BatchError, BatchLimits, BatchOp, BatchRequest, BatchTreeVisitor, ExecPolicy, RequestMetadata,
};
use shamir_tx::CancelHandle;
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};

/// Capability to check whether `actor` may perform `action` on `path`.
///
/// Implemented by [`ShamirDb`](../../../../../shamir-db/src/shamir_db/shamir_db/access/gate.rs)
/// (`shamir-db`), delegating to `authorize_access` against the live ACL tree.
/// Custom implementations are trusted native-embedding policy: a permissive
/// gate authorizes the request. See [`Authorized`] and the module doc.
#[async_trait]
pub trait AccessGate: Send + Sync {
    /// Check one `(actor, path, action)` triple. `Err` denies.
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError>;

    /// Catalogue-only existence probe for a table READ. `None` means the
    /// repository is absent (preserve the legacy parent error); custom gates
    /// may keep the default and rely on the executor's normal validation.
    async fn table_read_exists(
        &self,
        _db_name: &str,
        _repo_name: &str,
        _table_name: &str,
    ) -> Result<Option<bool>, AccessError> {
        Ok(None)
    }
}

/// A [`BatchRequest`] that has already cleared authorization.
///
/// See the module doc for the full rationale. `execute_batch` and
/// `execute_in_open_tx` take this type BY VALUE (it is not `Clone`) in
/// place of a raw `&BatchRequest` + `Actor` — one mint authorizes exactly
/// one execution.
#[derive(Debug)]
pub struct Authorized<'a> {
    request: &'a BatchRequest,
    actor: Actor,
    db_name: &'a str,
    policy: ExecPolicy,
    deadline_cap: Option<(Instant, Duration)>,
    metadata: RequestMetadata,
    cancel: Option<Arc<CancelHandle>>,
}

/// The ONE table-refusal constructor of this layer.
///
/// A TABLE-scoped READ that cannot be served answers exactly this, whether
/// the table is ABSENT from the catalogue or EXISTS but is closed to the
/// actor: same `unknown_table` code, same fixed text, empty alias. Nothing
/// here names the table, its repository, the requesting alias or the ACL
/// decision that produced the refusal, so the refusal is not an existence
/// oracle. Every refusal site in `crate::query::batch`
/// ([`Authorized::authorize`], `validate_tables`, the runtime
/// re-resolution and the subscription source check) calls THIS function, so
/// the texts cannot drift apart again.
pub(in crate::query::batch) fn table_unavailable() -> BatchError {
    BatchError::query_coded("", "unknown_table", "table is unavailable")
}

impl<'a> Authorized<'a> {
    /// Run authorization and mint a token on success.
    ///
    /// Checks, in order:
    /// 1. The actor may [`Action::Read`] the database itself
    ///    (`ResourcePath::database(db_name)`) — the same DB-visibility
    ///    gate `execute_as`/`tx_execute_as` ran before this seam existed.
    /// 2. Every `(Action, ResourcePath)` pair derived from the WHOLE query tree —
    ///    including nested `Batch`/`ForEach` bodies at any depth — against
    ///    the SAME `gate`. Pairs are deduplicated (a batch touching the
    ///    same table N times pays the gate cost once), mirroring the
    ///    inline ACL cache `tx_execute_as` used to keep by hand.
    /// 3. Every `Subscribe` source (also at any depth) for
    ///    [`Action::Read`] on its table — subscription sources carry no
    ///    entry in the metadata's pair list, so without this they would be
    ///    granted without a check. A denial answers the fixed table refusal
    ///    (see below), so it matches what an absent source answers.
    ///
    /// Returns `BatchError::QueryError` on the first denial:
    /// - the database check keeps `code: Some("access_denied")` — it runs
    ///   before any table is resolved and therefore discloses nothing about
    ///   one;
    /// - a denied `(Action::Read, ResourcePath::Table { .. })` answers
    ///   `table_unavailable` instead — the SAME `unknown_table` code and
    ///   the SAME fixed `"table is unavailable"` text every read path
    ///   answers for an absent table. This is the no-existence-oracle rule:
    ///   the table ACL is open by default for a name with no catalogue
    ///   record, so a table-Read denial is only reachable for a table that
    ///   EXISTS, and replying `access_denied` (or any diagnostic echoing
    ///   the name) there would let an unauthorized caller tell a closed
    ///   table from an absent one;
    /// - every other table action (writes/DDL) keeps `access_denied`: that
    ///   rule masks existence on READS only.
    pub async fn authorize(
        request: &'a BatchRequest,
        actor: Actor,
        db_name: &'a str,
        gate: &dyn AccessGate,
    ) -> Result<Self, BatchError> {
        gate.check(&actor, &ResourcePath::database(db_name), Action::Read)
            .await
            .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

        let metadata = RequestMetadata::collect(&request.queries, db_name);
        let (created_tables, created_repos) = tables_created_in_batch(&request.queries);
        // Resolve table availability and ACL in the ordered authorization metadata.
        for (action, path) in &metadata.required_access {
            let ResourcePath::Table { db, store, table } = path else {
                gate.check(&actor, path, *action)
                    .await
                    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
                continue;
            };
            let table_read = *action == Action::Read && db == db_name;
            if table_read
                && (created_tables.contains(&format!("{store}/{table}"))
                    || created_repos.contains(store))
            {
                continue;
            }
            if table_read
                && gate
                    .table_read_exists(db, store, table)
                    .await
                    .ok()
                    .flatten()
                    == Some(false)
            {
                return Err(table_unavailable());
            }
            gate.check(&actor, path, *action).await.map_err(|e| {
                if table_read {
                    table_unavailable()
                } else {
                    BatchError::query_coded("", "access_denied", e.to_string())
                }
            })?;
        }

        // Subscription sources carry no `required_access` entry
        // (`BatchOp::required_access` answers `None` for `Subscribe`), so a
        // source closed to the actor would otherwise be handed a
        // subscription grant. Classify every source here, before any
        // manager is resolved, with the SAME fixed refusal an absent source
        // answers in the runner — a closed source and an absent one must
        // stay indistinguishable.
        for visit in BatchTreeVisitor::new(&request.queries) {
            let BatchOp::Subscribe(op) = &visit.entry.op else {
                continue;
            };
            for source in &op.subscribe {
                let path = ResourcePath::Table {
                    db: db_name.to_string(),
                    store: source.table.repo.clone(),
                    table: source.table.table.clone(),
                };
                let exists = gate
                    .table_read_exists(db_name, &source.table.repo, &source.table.table)
                    .await
                    .ok()
                    .flatten();
                if exists != Some(true) {
                    return Err(table_unavailable());
                }
                if gate.check(&actor, &path, Action::Read).await.is_err() {
                    return Err(table_unavailable());
                }
            }
        }

        Ok(Self {
            request,
            actor,
            db_name,
            policy: ExecPolicy::EMBEDDED,
            deadline_cap: None,
            cancel: None,
            metadata,
        })
    }

    /// Test-only escape hatch: mint a token WITHOUT running any check.
    ///
    /// Exists so engine-internal unit tests that exercise execution
    /// mechanics (not authorization itself) don't each need a real
    /// [`AccessGate`] impl. Not reachable from a production build — no
    /// caller outside `#[cfg(test)]` (or the `test-util` feature, used by
    /// downstream crates' own test/bench harnesses) can name this
    /// function.
    #[cfg(any(test, feature = "test-util"))]
    pub fn unchecked(request: &'a BatchRequest, actor: Actor, db_name: &'a str) -> Self {
        Self {
            request,
            actor,
            db_name,
            policy: ExecPolicy::EMBEDDED,
            deadline_cap: None,
            cancel: None,
            metadata: RequestMetadata::collect(&request.queries, db_name),
        }
    }

    /// Attach a server-owned policy independently of the request DTO.
    pub fn with_policy(mut self, policy: ExecPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Attach optional cooperative cancellation independently of wire limits.
    pub fn with_cancel_handle(mut self, cancel: Option<Arc<CancelHandle>>) -> Self {
        self.cancel = cancel;
        self
    }

    /// Attach an exact caller-owned budget independently of wire limits.
    /// The clock starts here, not when the root scope is later constructed.
    /// Repeated caps can only tighten the deadline; zero expires immediately.
    pub fn with_deadline_cap(self, cap: Duration) -> Self {
        self.with_deadline_cap_at(Instant::now(), cap)
    }

    fn with_deadline_cap_at(mut self, now: Instant, cap: Duration) -> Self {
        if let Some(deadline) = now.checked_add(cap) {
            if self
                .deadline_cap
                .is_none_or(|(previous, _)| deadline < previous)
            {
                self.deadline_cap = Some((deadline, cap));
            }
        }
        self
    }

    /// Inject an attachment instant while sharing production cap construction.
    #[cfg(test)]
    pub(in crate::query::batch) fn with_deadline_cap_at_for_test(
        self,
        now: Instant,
        cap: Duration,
    ) -> Self {
        self.with_deadline_cap_at(now, cap)
    }

    /// Borrow the server-owned execution policy.
    pub fn policy(&self) -> &ExecPolicy {
        &self.policy
    }

    /// Shared root construction used by both execution entry points.
    pub(in crate::query::batch) fn root_scope(&self) -> ExecScope {
        let scope = ExecScope::root_with_result_size(
            &self
                .request
                .limits
                .clamped_to(&self.policy().caps)
                .clamped_to(&BatchLimits::CEILING),
            self.policy.enforce_result_size,
        )
        .with_read_visibility_timeout(self.policy.read_visibility_timeout_ms)
        .with_root_metadata(self.metadata.clone())
        .with_cancel_handle(self.cancel.clone());
        match self.deadline_cap {
            Some((deadline, cap)) => scope.with_deadline_cap(deadline, cap),
            None => scope,
        }
    }

    /// The authorized request.
    pub fn request(&self) -> &'a BatchRequest {
        self.request
    }

    /// The actor the request was authorized for.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// The database the request was authorized against.
    pub fn db_name(&self) -> &'a str {
        self.db_name
    }

    pub(in crate::query::batch) fn metadata(&self) -> &RequestMetadata {
        &self.metadata
    }

    /// Consume the token, handing its parts to the execution entry points
    /// in this module. `pub(in crate::query::batch)` — only `execute_batch`/
    /// `execute_in_open_tx` may unwrap a token; every other consumer must
    /// go through the accessors above.
    pub(in crate::query::batch) fn into_parts(self) -> (&'a BatchRequest, Actor, &'a str) {
        (self.request, self.actor, self.db_name)
    }
}
