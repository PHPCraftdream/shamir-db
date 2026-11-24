//! Reporting for a failed launch's cleanup.
//!
//! A launch rollback preserves the boot error that aborted it: cleanup
//! failures are logged here at error level and never replace the original
//! cause. The cleanup report itself comes from the same shared audit
//! teardown the graceful path uses, so a stalled checkpoint shows up as an
//! audit timeout in the log instead of an unbounded wait.

use std::time::Duration;

use shamir_db::ShutdownReport;

use crate::server::server_handle::MetaSnapshotTask;

/// Upper bound for joining the meta-snapshot task in a rollback. The task still
/// has to run one final snapshot on the blocking pool after the root token is
/// cancelled, so a saturated or stalled pool would keep the failed launch from
/// ever returning its boot error. Same value as the graceful shutdown's join.
const META_SNAPSHOT_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Join the meta-snapshot task of a failed launch, bounded; an expiry is logged
/// like every other cleanup failure and the task is left to finish on its own
/// (it may still hold the `server_meta` store lock until then).
pub(super) async fn join_meta_snapshot(stage: &'static str, task: Option<MetaSnapshotTask>) {
    let Some(task) = task else {
        return;
    };
    if tokio::time::timeout(META_SNAPSHOT_JOIN_TIMEOUT, task.handle)
        .await
        .is_err()
    {
        tracing::error!(
            stage,
            error = "meta snapshot task did not stop within 5s: it may still hold the \
                     server_meta store lock, so an immediate retry on the same data \
                     directory can fail",
            "launch rollback cleanup error"
        );
    }
}

/// Log every cleanup failure of a rollback, naming the stage that failed.
pub(super) fn log_rollback_cleanup(stage: &'static str, cleanup: &ShutdownReport) {
    for error in cleanup.errors() {
        tracing::error!(stage, error, "launch rollback cleanup error");
    }
    for (repository, error) in cleanup.repository_failures() {
        tracing::error!(stage, repository, error, "launch rollback cleanup error");
    }
}
