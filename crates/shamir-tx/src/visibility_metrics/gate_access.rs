//! Gate-level accessor for the visibility telemetry.

use super::VisibilityMetrics;
use crate::repo_tx_gate::RepoTxGate;

impl RepoTxGate {
    /// Visibility-acknowledgement telemetry; it lives with the completion
    /// tracker that owns the waits it describes.
    pub fn visibility(&self) -> &VisibilityMetrics {
        self.completion().visibility()
    }
}
