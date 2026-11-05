//! Unit tests for the `log` capture install: graceful degradation when a
//! foreign logger owns the process logger, and stable idempotent status.

use crate::log_capture::{install_log_ring, LogCaptureStatus};
use crate::ring::shared_ring;
use crate::tests::support::NoopLogger;
use crate::FlakeGuard;

/// The installed logger really feeds the ring: a `log` record emitted after
/// the guard exists is in the ring, with its level and target. Relies on the
/// runner's one-test-per-process isolation (no foreign logger installed).
#[test]
fn installed_logger_captures_log_records_into_the_ring() {
    let _guard = FlakeGuard::new("installed_logger_captures_log_records_into_the_ring");
    assert_eq!(install_log_ring(), LogCaptureStatus::Installed);
    log::warn!(target: "diag_probe", "probe-record-{}", 7);
    let found = shared_ring()
        .snapshot()
        .into_iter()
        .any(|r| r.level == "WARN" && r.target == "diag_probe" && r.message == "probe-record-7");
    assert!(found, "the log record never reached the ring");
}

/// Degraded mode (design): when another logger already owns the process
/// logger, installation degrades and keeps reporting degraded instead of
/// failing or touching the foreign logger. Relies on the runner's
/// one-test-per-process isolation (same assumption as shamir-index's
/// log_sink, which aborts on a pre-installed logger for the same reason).
#[test]
fn install_degrades_gracefully_when_a_foreign_logger_is_installed() {
    assert!(
        log::set_boxed_logger(Box::new(NoopLogger)).is_ok(),
        "a logger was already installed before this test ran — the degraded-mode test requires one-test-per-process isolation",
    );
    assert_eq!(install_log_ring(), LogCaptureStatus::Degraded);
    assert_eq!(install_log_ring(), LogCaptureStatus::Degraded);
}

/// Repeated installs report one stable status: whichever logger owns the
/// process logger, both calls agree.
#[test]
fn install_status_is_stable_across_calls() {
    let first = install_log_ring();
    let again = install_log_ring();
    assert_eq!(first, again);
}
