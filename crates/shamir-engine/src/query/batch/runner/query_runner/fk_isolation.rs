use shamir_query_types::batch::BatchError;

use crate::query::batch::executor_traits::TableResolver;
use crate::query::TableRef;

/// B06 one-shot pause after graph acquisition, before an FK role decision.
/// Each Notify has one waiter; notify_one retains an early permit.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct FkRoleDecisionHook {
    pub(crate) reached: tokio::sync::Notify,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl FkRoleDecisionHook {
    pub(crate) fn new() -> Self {
        Self {
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
            armed: std::sync::atomic::AtomicBool::new(true),
        }
    }

    async fn fire(&self) {
        if self
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            self.reached.notify_one();
            self.resume.notified().await;
        }
    }
}

// Installation assumes nextest's per-test process isolation, as do the
// existing cache seams. Uninstalled hooks never suspend the caller.
#[cfg(test)]
pub(crate) static TEST_PARENT_PRE_ROLE_DECISION_HOOK: std::sync::OnceLock<
    std::sync::Arc<FkRoleDecisionHook>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) static TEST_CHILD_PRE_ROLE_DECISION_HOOK: std::sync::OnceLock<
    std::sync::Arc<FkRoleDecisionHook>,
> = std::sync::OnceLock::new();

/// F-28 Step 5 (S3-C) — child-side hook: if `table_ref`'s table is flagged
/// (via `RepoInstance::fk_reverse_cache`) as an FK child (it references some
/// OTHER table via a foreign key), require this tx's commit footprint to be
/// published for it, regardless of the tx's own isolation level.
///
/// This is the mechanism that makes a concurrent Serializable FK-parent
/// delete's phantom check (Phase 2-bis) have something to conflict against
/// even when THIS writer is plain Snapshot isolation (the overwhelming
/// common case for an autocommit insert/update) — see
/// `build_footprint_from_tx`'s widened gate in `shamir-tx`'s
/// `repo_tx_gate.rs`. Called at insert/update/set STAGING time, before the
/// write is staged, from both the explicit-tx and implicit-tx arms below —
/// any Snapshot-isolation writer touching an FK-child table needs its
/// footprint published, regardless of which path staged it.
///
/// **F-40: fail-CLOSED on discovery failure.** This hook is a correctness
/// precondition for the FK-race-closure guarantee (without a footprint
/// token, a concurrent Serializable FK-parent-delete's Phase 2-bis check has
/// nothing to conflict against — exactly the silent-corruption outcome S3-C
/// exists to prevent). So when FK-role discovery ITSELF fails — a
/// `resolve_repo` error, or a `get_or_build_snapshot` cache-build error —
/// the hook does NOT fall back to the permissive "assume not an FK child"
/// behavior; it widens the footprint UNCONDITIONALLY by calling
/// `TxContext::require_footprint_for` anyway. This is safe because a
/// `resolve_repo` failure here almost certainly means the SAME resolve will
/// also fail moments later for the actual write this hook precedes (the
/// table was just resolved by the caller; a transient failure tends to
/// recur on the very next FK-discovery call the write path needs), so the
/// operation is very likely to fail downstream regardless — the extra
/// footprint token is a no-practical-cost safety margin, not a source of
/// new spurious aborts on otherwise-healthy repos. A cache-build failure is
/// rarer still and would likewise recur on the next real FK-discovery call.
///
/// The first touch builds the repo graph. Entries and roles come from one
/// populated snapshot, unaffected by invalidation after acquisition.
pub(crate) async fn require_footprint_if_fk_child(
    resolver: &dyn TableResolver,
    table_ref: &TableRef,
    table_token: u64,
    tx: &mut shamir_tx::TxContext,
) {
    let repo = match resolver.resolve_repo(&table_ref.repo).await {
        Ok(repo) => repo,
        Err(e) => {
            // F-40: fail CLOSED — discovery failed, so we cannot prove the
            // table is NOT an FK child. Widen the footprint unconditionally
            // rather than silently skipping the requirement (the old
            // permissive behavior). See this function's doc comment for why
            // this is safe and the right direction for a correctness-gating
            // hook.
            log::warn!(
                "F-40 fail-closed: require_footprint_if_fk_child: resolve_repo({}) failed ({e}) \
                 — widening footprint for table_token={table_token} anyway (cannot prove \
                 non-FK-child on discovery failure)",
                table_ref.repo
            );
            tx.require_footprint_for(table_token);
            return;
        }
    };
    let cache = repo.fk_reverse_cache();
    let repo_name = table_ref.repo.clone();
    let snapshot = match cache
        .get_or_build_snapshot(|| {
            // Fn is re-callable after a generation CAS loss.
            let repo_name = repo_name.clone();
            async move { crate::repo::build_reverse_fk_entries(resolver, &repo_name).await }
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(e) => {
            // F-40: fail CLOSED — same rationale as the resolve_repo error path
            // above. A cache-build failure means we cannot prove the table is
            // NOT an FK child, so widen the footprint unconditionally rather
            // than silently skipping the requirement.
            log::warn!(
                "F-40 fail-closed: require_footprint_if_fk_child: reverse-FK scan failed for repo \
                 '{}' ({e}) — widening footprint for table_token={table_token} anyway (cannot prove \
                 non-FK-child on cache-build failure)",
                table_ref.repo
            );
            tx.require_footprint_for(table_token);
            return;
        }
    };
    #[cfg(test)]
    if let Some(hook) = TEST_CHILD_PRE_ROLE_DECISION_HOOK.get() {
        hook.fire().await;
    }
    if snapshot.is_fk_child(&table_ref.table) {
        tx.require_footprint_for(table_token);
    }
}

/// F-28 Step 5 (S3-C) — parent-side hook: decide the isolation level an
/// IMPLICIT batch tx should open at for a delete/update against
/// `table_ref`'s table.
///
/// Returns [`IsolationLevel::Serializable`](shamir_tx::IsolationLevel::Serializable)
/// iff `table_ref`'s table is flagged (via `RepoInstance::fk_reverse_cache`)
/// as an FK parent with at least one non-`NoAction` action for the
/// operation kind `op` selects — `FkParentOpKind::Delete` consults
/// `FkSnapshot::is_parent_with_delete_action` (the `on_delete`
/// flag), `FkParentOpKind::Update` consults
/// `FkSnapshot::is_parent_with_update_action` (the `on_update`
/// flag). I.e. a RESTRICT/CASCADE/SET NULL fan-out is about to run against
/// it on that path. Upgrading to Serializable is what makes the fan-out's
/// child-table scans (`fk_restrict.rs`/`fk_actions.rs`/`fk_on_update.rs`)
/// record a real `TableScan` SSI predicate, closing the cross-transaction
/// TOCTOU race between "scan found no conflicting child row" and this tx's
/// eventual commit.
///
/// **F-40: fail-CLOSED on discovery failure.** When FK-role discovery ITSELF
/// fails — a `resolve_repo` error, or a `get_or_build_snapshot` cache-build
/// error — the hook returns `Serializable` (the MORE protective isolation),
/// not `Snapshot`. The old permissive fallback (return `Snapshot`, byte-
/// identical to a non-FK-parent table) was the wrong direction for a
/// correctness-gating mechanism: a discovery failure means we cannot prove
/// the table is NOT an FK parent with a non-`NoAction` action, so assume it
/// is and apply the stronger protection. This is safe for the same reason
/// `require_footprint_if_fk_child`'s fail-closed branch is safe (see that
/// function's doc): a `resolve_repo` failure here almost certainly recurs
/// for the actual delete/update this hook precedes, so the operation very
/// likely fails downstream regardless — the Serializable upgrade is a
/// no-practical-cost safety margin, not a source of new spurious aborts on
/// otherwise-healthy repos.
///
/// F-35: `op` was added because the cache used to carry only a single
/// `action` field (always `on_delete`), so the UPDATE arm — which needs the
/// `on_update` flag — silently never upgraded for an `on_delete = NoAction,
/// on_update = Restrict/Cascade/SetNull` FK. The two arms now ask the
/// cache the question matching their operation kind.
pub(crate) enum FkParentOpKind {
    Delete,
    Update,
}

pub(crate) async fn implicit_tx_isolation_for_fk_parent(
    resolver: &dyn TableResolver,
    table_ref: &TableRef,
    op: FkParentOpKind,
) -> shamir_tx::IsolationLevel {
    let repo = match resolver.resolve_repo(&table_ref.repo).await {
        Ok(repo) => repo,
        Err(_e) => {
            // F-40: fail CLOSED — discovery failed, so we cannot prove the
            // table is NOT an FK parent with a non-`NoAction` action. Return
            // the MORE protective isolation rather than the old permissive
            // `Snapshot` fallback. See this function's doc comment.
            log::warn!(
                "F-40 fail-closed: implicit_tx_isolation_for_fk_parent: resolve_repo({}) failed \
                 ({_e}) — returning Serializable anyway (cannot prove non-FK-parent on discovery \
                 failure)",
                table_ref.repo
            );
            return shamir_tx::IsolationLevel::Serializable;
        }
    };
    let cache = repo.fk_reverse_cache();
    let repo_name = table_ref.repo.clone();
    let snapshot = match cache
        .get_or_build_snapshot(|| {
            // Fn is re-callable after a generation CAS loss.
            let repo_name = repo_name.clone();
            async move { crate::repo::build_reverse_fk_entries(resolver, &repo_name).await }
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(_e) => {
            // F-40: fail CLOSED — same rationale as the resolve_repo error path
            // above. A cache-build failure means we cannot prove the table is
            // NOT an FK parent, so return the MORE protective isolation rather
            // than the old permissive `Snapshot` fallback.
            log::warn!(
                "F-40 fail-closed: implicit_tx_isolation_for_fk_parent: reverse-FK scan failed for \
                 repo '{}' — returning Serializable anyway (cannot prove non-FK-parent on \
                 cache-build failure)",
                table_ref.repo
            );
            return shamir_tx::IsolationLevel::Serializable;
        }
    };
    #[cfg(test)]
    if let Some(hook) = TEST_PARENT_PRE_ROLE_DECISION_HOOK.get() {
        hook.fire().await;
    }
    let is_fk_parent = match op {
        FkParentOpKind::Delete => snapshot.is_parent_with_delete_action(&table_ref.table),
        FkParentOpKind::Update => snapshot.is_parent_with_update_action(&table_ref.table),
    };
    if is_fk_parent {
        shamir_tx::IsolationLevel::Serializable
    } else {
        shamir_tx::IsolationLevel::Snapshot
    }
}

/// F-28 Step 5 (S3-C) — bounded retry budget for an implicit FK-relevant
/// delete/update that was opportunistically upgraded to Serializable.
///
/// Upgrading to Serializable means the implicit tx CAN now abort with
/// `CommitError::PhantomConflict`/`SsiConflict` (surfaced as the coded
/// `"tx_conflict"` — see `RepoInstance::commit_implicit_batch_tx`) in cases
/// that used to always succeed under plain Snapshot (Snapshot never
/// aborts). A genuine, persistent write-write race SHOULD abort and reach
/// the caller — but an already-resolved race (the conflicting concurrent
/// writer committed first and is simply gone by the next attempt) should
/// not surface as a client-visible error for what is, from the caller's
/// perspective, a perfectly ordinary single delete/update. Re-running the
/// WHOLE attempt (re-plan the FK fan-out against the fresh snapshot, then
/// re-commit) resolves that case transparently.
///
/// This codebase has no pre-existing generic "retry on tx_conflict" wrapper
/// (searched `query_runner.rs` or a `db_execute.rs`-equivalent) to match, so
/// this is intentionally minimal and tightly scoped: a small fixed bound
/// (never unbounded), applied ONLY to the two call sites that perform this
/// specific opportunistic isolation upgrade (implicit DELETE/UPDATE against
/// an FK-flagged parent table). After the bound is exhausted the LAST
/// error is returned unchanged (never swallowed) — the caller sees the same
/// coded `tx_conflict` a non-retried commit would have produced.
const FK_SERIALIZABLE_RETRY_ATTEMPTS: u32 = 3;

/// Run `attempt` up to [`FK_SERIALIZABLE_RETRY_ATTEMPTS`] times, retrying
/// only on a coded `"tx_conflict"` `BatchError` (the SSI/phantom/wound-wait
/// abort family — see `RepoInstance::commit_implicit_batch_tx`'s mapping).
/// Any other error, or the final attempt's error once the budget is
/// exhausted, is returned as-is — never swallowed.
///
/// `attempt` is re-invoked from scratch on each retry (a fresh closure call),
/// so the caller's body must be safe to re-run wholesale: re-resolve/re-plan
/// against a fresh implicit tx, not resume a half-finished one. Both call
/// sites below satisfy this — each `attempt` begins a brand-new implicit tx
/// internally.
pub(super) async fn retry_on_tx_conflict<F, Fut, T>(mut attempt: F) -> Result<T, BatchError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, BatchError>>,
{
    let mut last_err = None;
    for _ in 0..FK_SERIALIZABLE_RETRY_ATTEMPTS {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) if e.code() == Some("tx_conflict") => {
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    // Budget exhausted — surface the last (coded) conflict unchanged.
    Err(last_err.expect("loop runs at least once, so an exhausted budget always has an error"))
}
