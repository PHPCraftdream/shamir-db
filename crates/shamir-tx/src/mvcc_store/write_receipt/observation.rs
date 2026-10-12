use std::time::Duration;

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
pub(crate) async fn observe(
    gate: &RepoTxGate,
    version: u64,
    wait: VisibilityWait,
) -> VisibilityState {
    if version == 0 || gate.visible_floor() >= version {
        return VisibilityState::Visible;
    }
    if !gate.requires_reopen() {
        match wait {
            VisibilityWait::Strict => {
                gate.await_visible(version).await;
            }
            VisibilityWait::Bounded { timeout_ms: 0 } => {}
            VisibilityWait::Bounded { timeout_ms } => {
                let start = tokio::time::Instant::now();
                // An unrepresentable post-known budget is exhausted, never
                // an ordinary write error erasing the committed position.
                let deadline = start
                    .checked_add(Duration::from_millis(timeout_ms))
                    .unwrap_or(start);
                let _ = tokio::time::timeout_at(deadline, gate.await_visible(version)).await;
            }
        }
    }
    let visible_through = gate.visible_floor();
    if visible_through >= version {
        return VisibilityState::Visible;
    }
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
}
