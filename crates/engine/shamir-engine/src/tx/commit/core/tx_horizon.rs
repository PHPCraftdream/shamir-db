use shamir_query_types::batch::VisibilityWait;
use std::sync::Arc;

use crate::tx::commit::pipeline::admission::gate::CommitAdmissionGate;

#[derive(Clone)]
pub(crate) struct TxHorizon {
    /// Lazy-initialized RepoTxGate. Created on first call to `tx_gate()`.
    tx_gate: Arc<tokio::sync::OnceCell<Arc<shamir_tx::RepoTxGate>>>,
    /// Atomic tx telemetry counters.
    tx_metrics: Arc<shamir_tx::TxMetrics>,
    /// Counts owned inline and detached commits for explicit shutdown waiting.
    commit_tail: Arc<crate::tx::CommitTailBarrier>,
    /// R2a 1c: commit admission gate (design `online-backup.md` section 2).
    /// One per repository, owned next to `commit_tail`; reachable from the
    /// commit pipeline (leader) and the follower apply path.
    commit_admission: Arc<CommitAdmissionGate>,
    /// D2 P1d-2b: lazily-spawned background drainer (history-write off the
    /// ack-path). Created + spawned once on first `drainer`
    /// call; shared across foreground clones. The drain TASK holds a
    /// background clone of this repo (`live = None`) plus a `Weak<()>` of
    /// the shell's `live` token, so it exits + releases when the last foreground
    /// clone drops — see [`Drainer::spawn`](crate::tx::drainer::Drainer::spawn).
    ///
    /// `std::sync::OnceLock` (not tokio's async `OnceCell`): the
    /// `drainer` accessor is SYNC (called from the commit
    /// tail to `wake()`), and init does no `.await` — it only constructs
    /// the drainer + `tokio::spawn`s the loop. Single-init is guaranteed by
    /// `OnceLock::get_or_init`: the closure runs EXACTLY once even under
    /// concurrent callers (the loser waits for the winner's value and
    /// returns the same `Arc<Drainer>`), so there is no "lost race" — every
    /// caller observes the one-and-only drainer.
    drainer: Arc<std::sync::OnceLock<Arc<crate::tx::drainer::Drainer>>>,
    commit_visibility_wait: shamir_query_types::batch::VisibilityWait,
}

impl TxHorizon {
    pub(crate) fn new(
        tx_gate: Arc<tokio::sync::OnceCell<Arc<shamir_tx::RepoTxGate>>>,
        tx_metrics: Arc<shamir_tx::TxMetrics>,
        commit_tail: Arc<crate::tx::CommitTailBarrier>,
        commit_admission: Arc<CommitAdmissionGate>,
        drainer: Arc<std::sync::OnceLock<Arc<crate::tx::drainer::Drainer>>>,
        commit_visibility_wait: VisibilityWait,
    ) -> Self {
        Self {
            tx_gate,
            tx_metrics,
            commit_tail,
            commit_admission,
            drainer,
            commit_visibility_wait,
        }
    }

    pub(crate) fn tx_gate_cell(&self) -> &Arc<tokio::sync::OnceCell<Arc<shamir_tx::RepoTxGate>>> {
        &self.tx_gate
    }

    pub(crate) fn initialized_tx_gate(&self) -> Option<&shamir_tx::RepoTxGate> {
        self.tx_gate_cell().get().map(|gate| gate.as_ref())
    }

    pub(crate) fn tx_metrics(&self) -> &Arc<shamir_tx::TxMetrics> {
        &self.tx_metrics
    }

    pub(crate) fn commit_tail(&self) -> &Arc<crate::tx::CommitTailBarrier> {
        &self.commit_tail
    }

    pub(crate) fn commit_admission(&self) -> &Arc<CommitAdmissionGate> {
        &self.commit_admission
    }

    pub(crate) fn drainer_cell(
        &self,
    ) -> &Arc<std::sync::OnceLock<Arc<crate::tx::drainer::Drainer>>> {
        &self.drainer
    }

    pub(crate) fn commit_visibility_wait(&self) -> VisibilityWait {
        self.commit_visibility_wait
    }

    pub(crate) fn set_commit_visibility_wait(&mut self, wait: VisibilityWait) {
        self.commit_visibility_wait = wait;
    }
}
