//! Gate observation primitive: a settle-or-pending verdict against a
//! `RepoTxGate` under an optional budget. Shared by the instance
//! visibility wait, the tx acknowledgement tail and the temporal read
//! planner.

use std::time::Duration;

use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState};
use shamir_storage::error::{DbError, DbResult};
use shamir_tx::visibility_metrics::{AckResult, PendingKind, PendingReason};
use shamir_tx::RepoTxGate;

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
    gate: Option<&RepoTxGate>,
    visibility: VisibilityState,
    kind: PendingKind,
) {
    if let VisibilityState::Pending { reason, .. } = visibility {
        if let Some(gate) = gate {
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
