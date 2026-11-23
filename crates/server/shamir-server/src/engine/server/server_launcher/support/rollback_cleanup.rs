//! Reporting for a failed launch's cleanup.
//!
//! A launch rollback preserves the boot error that aborted it: cleanup
//! failures are logged here at error level and never replace the original
//! cause. The cleanup report itself comes from the same shared audit
//! teardown the graceful path uses, so a stalled checkpoint shows up as an
//! audit timeout in the log instead of an unbounded wait.

use shamir_db::ShutdownReport;

/// Log every cleanup failure of a rollback, naming the stage that failed.
pub(super) fn log_rollback_cleanup(stage: &'static str, cleanup: &ShutdownReport) {
    for error in cleanup.errors() {
        tracing::error!(stage, error, "launch rollback cleanup error");
    }
    for (repository, error) in cleanup.repository_failures() {
        tracing::error!(stage, repository, error, "launch rollback cleanup error");
    }
}
