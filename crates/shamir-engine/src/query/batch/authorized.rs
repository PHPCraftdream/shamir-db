//! Type-level authorization seam — [`Authorized`] + [`AccessGate`] (#1199).
//!
//! **Problem this closes.** Before this module existed, [`execute_batch`]
//! and [`execute_in_open_tx`] took a raw `&BatchRequest` + `Actor` pair —
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
//! inversion-of-control seam already used for [`super::AdminExecutor`] /
//! [`super::FunctionInvoker`] / [`super::TableResolver`]
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

use async_trait::async_trait;
use shamir_collections::TFxSet;
use shamir_query_types::batch::{collect_required_access, BatchError, BatchRequest};
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};

/// Capability to check whether `actor` may perform `action` on `path`.
///
/// Implemented by `ShamirDb` (`shamir-db`), delegating to
/// `authorize_access` against the live ACL tree. See the module doc for
/// why this indirection exists.
#[async_trait]
pub trait AccessGate: Send + Sync {
    /// Check one `(actor, path, action)` triple. `Err` denies.
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError>;
}

/// A [`BatchRequest`] that has already cleared authorization.
///
/// See the module doc for the full rationale. [`execute_batch`] and
/// [`execute_in_open_tx`] take this type BY VALUE (it is not `Clone`) in
/// place of a raw `&BatchRequest` + `Actor` — one mint authorizes exactly
/// one execution.
#[derive(Debug)]
pub struct Authorized<'a> {
    request: &'a BatchRequest,
    actor: Actor,
    db_name: &'a str,
}

impl<'a> Authorized<'a> {
    /// Run authorization and mint a token on success.
    ///
    /// Checks, in order:
    /// 1. The actor may [`Action::Read`] the database itself
    ///    (`ResourcePath::database(db_name)`) — the same DB-visibility
    ///    gate `execute_as`/`tx_execute_as` ran before this seam existed.
    /// 2. Every `(Action, ResourcePath)` pair
    ///    [`collect_required_access`] derives from the WHOLE query tree —
    ///    including nested `Batch`/`ForEach` bodies at any depth — against
    ///    the SAME `gate`. Pairs are deduplicated (a batch touching the
    ///    same table N times pays the gate cost once), mirroring the
    ///    inline ACL cache `tx_execute_as` used to keep by hand.
    ///
    /// Returns `BatchError::QueryError { code: Some("access_denied"), .. }`
    /// on the first denial — the same wire shape `execute_as`/
    /// `tx_execute_as` already produced.
    pub async fn authorize(
        request: &'a BatchRequest,
        actor: Actor,
        db_name: &'a str,
        gate: &dyn AccessGate,
    ) -> Result<Self, BatchError> {
        gate.check(&actor, &ResourcePath::database(db_name), Action::Read)
            .await
            .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

        let mut seen: TFxSet<(Action, ResourcePath)> = TFxSet::default();
        for (action, path) in collect_required_access(&request.queries, db_name) {
            if !seen.insert((action, path.clone())) {
                continue;
            }
            gate.check(&actor, &path, action)
                .await
                .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        }

        Ok(Self {
            request,
            actor,
            db_name,
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

    /// Consume the token, handing its parts to the execution entry points
    /// in this module. `pub(super)` — only `execute_batch`/
    /// `execute_in_open_tx` may unwrap a token; every other consumer must
    /// go through the accessors above.
    pub(super) fn into_parts(self) -> (&'a BatchRequest, Actor, &'a str) {
        (self.request, self.actor, self.db_name)
    }
}
