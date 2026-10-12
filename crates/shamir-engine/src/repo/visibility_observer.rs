use std::time::Duration;

use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use shamir_storage::error::{DbError, DbResult};
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
        observe_gate_visibility(&gate, version, wait).await
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
