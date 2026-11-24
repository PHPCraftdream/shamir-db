//! A failed launch's cleanup must be REPORTED, never silently dropped: every
//! audit/repository failure the shared teardown recorded is logged at error
//! level by [`log_rollback_cleanup`], and the boot error still wins.
//!
//! Mutant killed: the function's body removed, or its `tracing::error!` level
//! lowered to debug — the ring captures debug too, so the level assertion is
//! what fails, not only the "no record at all" case.

use shamir_db::ShutdownReport;
use shamir_test_diag::{ring::shared_ring, try_install_global_tracing_ring};

use super::rollback_cleanup::log_rollback_cleanup;

#[test]
fn launch_rollback_cleanup_logs_errors_and_repository_failures() {
    let _ = try_install_global_tracing_ring();

    // Both failures the report can carry, with inputs unique to this test so
    // no other process-wide log line can satisfy the assertions.
    let mut cleanup = ShutdownReport::default();
    cleanup.add_error("test rollback error alpha");
    cleanup.record(
        "test-repo-beta",
        Err("test repository failure gamma".to_string()),
    );
    log_rollback_cleanup("test-rollback-stage", &cleanup);

    let records = shared_ring().snapshot();
    let stage_field = "stage=\"test-rollback-stage\"";
    let events: Vec<_> = records
        .iter()
        .filter(|record| {
            record.message.contains("launch rollback cleanup error")
                && record.message.contains(stage_field)
        })
        .collect();
    assert!(
        events.iter().any(|record| record.level == "ERROR"
            && record
                .message
                .contains("error=\"test rollback error alpha\"")),
        "every rollback cleanup error must be logged at ERROR for the exact input: {records:?}"
    );
    assert!(
        events.iter().any(|record| record.level == "ERROR"
            && record.message.contains("repository=\"test-repo-beta\"")
            && record
                .message
                .contains("error=\"test repository failure gamma\"")),
        "every rollback repository failure must be logged at ERROR for the exact input: \
         {records:?}"
    );
}
