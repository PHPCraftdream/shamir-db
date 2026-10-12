use std::time::Duration;

use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use shamir_storage::error::{DbError, DbResult};
use shamir_tx::visibility_metrics::{
    AckResult, PendingKind, PendingReason, VisibilityMetricsSnapshot,
};
use shamir_tx::RepoTxGate;

use super::RepoInstance;

impl RepoInstance {
    /// Observe settlement through a repository-scoped position, not a commit decision.
    /// cancel-safe: yes — gate initialization retains no observer; the only wait
    /// owns a timer and the gate's RAII registration. No task or snapshot is created.
    pub async fn observe_visibility(
        &self,
        version: u64,
        wait: VisibilityWait,
    ) -> DbResult<VisibilityState> {
        let gate = self.tx_gate().await?;
        // VIS-4c reconciliation bookkeeping lives HERE, not inside the shared
        // observation primitive: the same primitive also serves the
        // acknowledgement tail and the AsOf read wait.
        let started = tokio::time::Instant::now();
        let could_wait = gate.visible_floor() < version
            && !gate.requires_reopen()
            && !matches!(wait, VisibilityWait::Bounded { timeout_ms: 0 });
        let result = observe_gate_visibility(&gate, version, wait).await;
        let ack_result = match &result {
            Ok(state) => query_ack_result(*state),
            Err(_) => AckResult::Error,
        };
        let waited = could_wait && ack_result.observed_wait();
        gate.visibility()
            .record_reconciliation(ack_result, waited, started.elapsed());
        if let Ok(VisibilityState::Pending {
            visible_through,
            blocking_version,
            ..
        }) = &result
        {
            if gate.visibility().reserve_pending_warning() {
                log::warn!(
                    "WaitForVisibility reconciliation still pending at wait expiry: repository={}, target={version}, visible_through={visible_through}, blocking_version={blocking_version}, elapsed_ms={}",
                    self.name(),
                    started.elapsed().as_millis(),
                );
            }
        }
        result
    }
}

/// cancel-safe: yes — dropping either wait drops await_visible's registration;
/// neither wait mutates allocation, completion, or recovery state.
pub(crate) async fn observe_gate_visibility(
    gate: &RepoTxGate,
    version: u64,
    wait: VisibilityWait,
) -> DbResult<VisibilityState> {
    let budget = match wait {
        VisibilityWait::Strict => None,
        VisibilityWait::Bounded { timeout_ms } => Some(Duration::from_millis(timeout_ms)),
    };
    observe_gate_with_budget(gate, version, budget).await
}

/// cancel-safe: yes — the timer drops the gate's RAII waiter, no tasks survive.
pub(crate) async fn observe_gate_with_budget(
    gate: &RepoTxGate,
    version: u64,
    budget: Option<Duration>,
) -> DbResult<VisibilityState> {
    if gate.visible_floor() >= version {
        return Ok(VisibilityState::Visible);
    }
    let allocated = gate.version_allocation_high_water_mark();
    if version > allocated {
        return Err(DbError::VisibilityFutureVersion {
            requested: version,
            allocated,
        });
    }
    if !gate.requires_reopen() {
        match budget {
            None => {
                gate.await_visible(version).await;
            }
            Some(duration) if duration.is_zero() => {}
            Some(duration) => {
                // Reject an unrepresentable budget rather than making it infinite.
                let deadline = tokio::time::Instant::now().checked_add(duration).ok_or(
                    DbError::VisibilityWaitInvalid {
                        reason: "visibility wait duration is not representable".into(),
                    },
                )?;
                let _ = tokio::time::timeout_at(deadline, gate.await_visible(version)).await;
            }
        }
    }
    // Final sample: settlement wins over both expiry and reopen.
    let visible_through = gate.visible_floor();
    if visible_through >= version {
        return Ok(VisibilityState::Visible);
    }
    Ok(VisibilityState::Pending {
        visible_through,
        // visible_through < version <= u64::MAX proves this addition fits.
        blocking_version: visible_through + 1,
        reason: if gate.requires_reopen() {
            VisibilityPendingReason::ReopenRequired
        } else {
            VisibilityPendingReason::Timeout
        },
    })
}

/// VIS-4c: record the Pending-kind counter at a kind-known call site. The
/// shared acknowledgement tail serves both explicit and implicit commits,
/// so attribution must happen where the write family is known. The gate is
/// always initialized once a commit returned a known outcome; absence is
/// silently ignored (pure telemetry).
pub(crate) fn record_pending_kind(
    repo: &RepoInstance,
    visibility: VisibilityState,
    kind: PendingKind,
) {
    if let VisibilityState::Pending { reason, .. } = visibility {
        if let Some(gate) = repo.initialized_tx_gate() {
            let reason = match reason {
                VisibilityPendingReason::Timeout => PendingReason::Timeout,
                VisibilityPendingReason::ReopenRequired => PendingReason::ReopenRequired,
            };
            gate.visibility().record_pending(reason, kind);
        }
    }
}

/// Metric label of a query-contract visibility observation (the tx layer
/// keeps its own visibility types and its own `ack_result_of`).
pub(crate) fn query_ack_result(state: VisibilityState) -> AckResult {
    match state {
        VisibilityState::Visible => AckResult::Visible,
        VisibilityState::Pending {
            reason: VisibilityPendingReason::Timeout,
            ..
        } => AckResult::PendingTimeout,
        VisibilityState::Pending {
            reason: VisibilityPendingReason::ReopenRequired,
            ..
        } => AckResult::PendingReopen,
    }
}

impl RepoInstance {
    /// Public seam for the server's metrics poller: a plain-data snapshot of
    /// this repository's visibility telemetry, `None` before gate
    /// initialization (the exporter registers every series at zero
    /// regardless).
    pub fn visibility_metrics_snapshot(&self) -> Option<VisibilityMetricsSnapshot> {
        self.initialized_tx_gate()
            .map(|gate| gate.visibility().snapshot())
    }

    /// Sampled settled prefix W, `None` before gate initialization (the
    /// `changes_since` field is omitted rather than reporting a misleading
    /// zero).
    pub fn visible_through_sample(&self) -> Option<u64> {
        self.initialized_tx_gate().map(RepoTxGate::visible_floor)
    }
}
