use std::future::Future;

use shamir_db::engine::BoundedJournalRead;

/// Internal RPITIT + Send seam: engine hooks are crate-test-only. No dyn API.
pub(super) trait JournalSource: Sync {
    /// Observation seam for deterministic tests; production has no idle work.
    fn on_idle(&self) {}

    fn read(
        &self,
        repo: &str,
        from: u64,
    ) -> impl Future<Output = Result<BoundedJournalRead, String>> + Send;

    /// cancel-safe: yes — completion observation only, no payload consumption.
    fn progress(&self, repo: &str, target: u64) -> impl Future<Output = Result<(), String>> + Send;
}
