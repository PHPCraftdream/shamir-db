use std::time::Duration;

use crate::visibility_metrics::{ack_result_of, AckMode, AckResult, PendingKind};
use crate::RepoTxGate;

// One closely coupled observation protocol: wait policy, sampled state and its
// pending reason are the inputs/outputs of the single observer below.
// The storage/tx layers cannot depend on query DTOs. Engine adapters convert
// these observations losslessly to the existing query contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityWait {
    Strict,
    Bounded { timeout_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityPendingReason {
    Timeout,
    ReopenRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityState {
    Visible,
    Pending {
        visible_through: u64,
        blocking_version: u64,
        reason: VisibilityPendingReason,
    },
}

impl RepoTxGate {
    /// Non-tx acknowledgement only: the version is already committed.
    /// cancel-safe: yes — cancellation abandons only the read-only observer.
    pub(crate) async fn await_non_tx_ack(
        &self,
        version: u64,
        wait: VisibilityWait,
    ) -> VisibilityState {
        observe(self, version, wait).await
    }
}

/// cancel-safe: yes — only await_visible's RAII registration is dropped.
/// Known writes require no allocation validation; W wins over reopen/expiry.
/// Records the VIS-4c direct-write metrics: the fast path pays one relaxed
/// counter increment; the duration histogram is observed only when this
/// acknowledgement actually waited.
pub(crate) async fn observe(
    gate: &RepoTxGate,
    version: u64,
    wait: VisibilityWait,
) -> VisibilityState {
    let mode = match wait {
        VisibilityWait::Strict => AckMode::Strict,
        VisibilityWait::Bounded { .. } => AckMode::Bounded,
    };
    if version == 0 || gate.visible_floor() >= version {
        gate.visibility()
            .record_ack(mode, AckResult::Visible, false, Duration::ZERO);
        return VisibilityState::Visible;
    }
    let started = tokio::time::Instant::now();
    let mut waited = false;
    if !gate.requires_reopen() {
        match wait {
            VisibilityWait::Strict => {
                gate.await_visible(version).await;
                waited = true;
            }
            VisibilityWait::Bounded { timeout_ms: 0 } => {}
            VisibilityWait::Bounded { timeout_ms } => {
                // An unrepresentable post-known budget is exhausted, never
                // an ordinary write error erasing the committed position.
                let deadline = started
                    .checked_add(Duration::from_millis(timeout_ms))
                    .unwrap_or(started);
                let _ = tokio::time::timeout_at(deadline, gate.await_visible(version)).await;
                waited = true;
            }
        }
    }
    let visible_through = gate.visible_floor();
    let state = if visible_through >= version {
        VisibilityState::Visible
    } else {
        VisibilityState::Pending {
            visible_through,
            // visible_through < version proves the addition fits.
            blocking_version: visible_through + 1,
            reason: if gate.requires_reopen() {
                VisibilityPendingReason::ReopenRequired
            } else {
                VisibilityPendingReason::Timeout
            },
        }
    };
    let result = ack_result_of(state);
    gate.visibility()
        .record_ack(mode, result, waited, started.elapsed());
    if let VisibilityState::Pending {
        visible_through,
        blocking_version,
        reason,
    } = state
    {
        gate.visibility()
            .record_pending(reason, PendingKind::DirectWrite);
        if gate.visibility().reserve_pending_warning() {
            log::warn!(
                "direct write acknowledgement still pending at wait expiry: target={version}, visible_through={visible_through}, blocking_version={blocking_version}, elapsed_ms={}",
                started.elapsed().as_millis(),
            );
        }
    }
    state
}
