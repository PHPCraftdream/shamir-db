use std::time::Duration;

use shamir_query_types::batch::{VisibilityState, VisibilityWait};
use shamir_storage::error::DbResult;
use shamir_tx::visibility_metrics::{AckResult, VisibilityMetricsSnapshot};
use shamir_tx::RepoTxGate;

use crate::repo::gate_visibility::{observe_gate_with_budget, query_ack_result};

use super::super::RepoInstance;

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
