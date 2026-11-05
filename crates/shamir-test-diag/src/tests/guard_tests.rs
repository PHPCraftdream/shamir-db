//! Unit tests for [`FlakeGuard`]: failure-only printing, the watchdog's
//! exactly-once dump, and its silence after the guard is dropped.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use crate::ring::shared_ring;
use crate::tests::support::{captured, clear_capture, install_capture};
use crate::FlakeGuard;

/// Green path (design requirement): a passing test prints NOTHING, even
/// though the guard recorded marks and notes and the ring holds records.
#[test]
fn green_path_prints_nothing() {
    let capture = install_capture();
    let guard = FlakeGuard::new("green_path_prints_nothing");
    guard.mark("step one");
    guard.note("port", "0");
    shared_ring().push("INFO", "green_path", "a record");
    drop(guard);
    assert!(
        captured(&capture).is_empty(),
        "a green run must print nothing: {}",
        captured(&capture)
    );
    clear_capture();
}

/// Panic path: dropping the guard while panicking prints the header, the
/// full timeline, and the ring contents.
#[test]
fn panic_path_prints_timeline_and_ring() {
    let capture = install_capture();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let guard = FlakeGuard::new("panic_path_prints_timeline_and_ring");
        guard.mark("before the failure");
        shared_ring().push("WARN", "panic_path", "ring line");
        guard.mark("after the record");
        panic!("simulated test failure");
    }));
    assert!(
        result.is_err(),
        "the simulated panic must surface as an error"
    );
    let text = captured(&capture);
    assert!(
        text.contains("=== flake-diag: panic_path_prints_timeline_and_ring ==="),
        "report header missing: {text}"
    );
    assert!(
        text.contains("--- timeline ---"),
        "timeline section missing: {text}"
    );
    assert!(
        text.contains("before the failure"),
        "timeline mark missing: {text}"
    );
    assert!(
        text.contains("after the record"),
        "timeline mark missing: {text}"
    );
    assert!(text.contains("ring line"), "ring contents missing: {text}");
    clear_capture();
}

/// Watchdog: dumps the report exactly once while the guard is alive past the
/// deadline, then never again ("dumps once and stops").
#[test]
fn watchdog_dumps_once_while_guard_is_alive() {
    let capture = install_capture();
    let guard = FlakeGuard::with_watchdog(
        "watchdog_dumps_once_while_guard_is_alive",
        Duration::from_millis(120),
    );
    std::thread::sleep(Duration::from_millis(500));
    let first = captured(&capture);
    assert_eq!(
        first.matches("watchdog: FIRED").count(),
        1,
        "exactly one watchdog dump expected: {first}"
    );
    std::thread::sleep(Duration::from_millis(400));
    let later = captured(&capture);
    assert_eq!(
        later.matches("watchdog: FIRED").count(),
        1,
        "the watchdog must stop after one dump: {later}"
    );
    assert!(
        later.contains("elapsed:"),
        "the watchdog dump must carry the report header: {later}"
    );
    drop(guard);
    clear_capture();
}

/// A guard dropped BEFORE the watchdog deadline produces no dump at all.
#[test]
fn watchdog_is_silent_once_the_guard_dropped() {
    let capture = install_capture();
    let guard = FlakeGuard::with_watchdog(
        "watchdog_is_silent_once_the_guard_dropped",
        Duration::from_millis(120),
    );
    drop(guard);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        captured(&capture).is_empty(),
        "a dropped guard must not be reported: {}",
        captured(&capture)
    );
    clear_capture();
}
