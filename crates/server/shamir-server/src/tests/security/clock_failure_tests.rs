//! Clock-failure tests (RV-CONNECT-387EAA05, P2-10 second tranche): a clock
//! that cannot be read is skipped (scheduler tick) or mapped to a typed error
//! (user-admin port adapter) instead of panicking via the panicking
//! `UnixNanos::now` wrapper or fabricating a timestamp.

use crate::boot::scheduler::tick_clock_or_skip;
use crate::sync::ports::{map_clock_to_port, now_ns_for_port};
use shamir_connect::common::error::EnvironmentError;
use shamir_connect::common::time::UnixNanos;

/// A clock failure makes the tick return `None` (skip) instead of
/// panicking or substituting a fabricated timestamp. On the pre-fix code
/// the tick used `UnixNanos::now()` and would panic outside `safe_run`.
#[test]
fn rv_connect_387eaa05_scheduler_tick_skips_on_clock_failure() {
    let failing = Err(EnvironmentError::ClockBeforeEpoch);
    assert!(
        tick_clock_or_skip("test_task", failing).is_none(),
        "clock failure must skip the tick"
    );
}

/// A healthy clock read still yields the timestamp, so normal ticks are
/// unaffected by the fallible wrapper.
#[test]
fn rv_connect_387eaa05_scheduler_tick_passes_healthy_clock_through() {
    let clock = UnixNanos::try_now().map(UnixNanos::as_u64);
    let now_ns = tick_clock_or_skip("test_task", clock);
    assert!(now_ns.is_some(), "healthy clock must not skip the tick");
}

/// The fallible clock read succeeds under a healthy clock (smoke pin so the
/// mapping below is provably not a hard-coded failure).
#[test]
fn rv_connect_387eaa05_port_clock_read_succeeds_on_healthy_clock() {
    assert!(now_ns_for_port().is_ok());
}

/// A clock failure maps to `PortError::other` — the same mapping the port
/// methods apply via `now_ns_for_port()?`. On the pre-fix code the port
/// called the panicking `UnixNanos::now()` and never returned this error.
#[test]
fn rv_connect_387eaa05_port_clock_failure_maps_to_port_error_other() {
    let failing: Result<u64, EnvironmentError> = Err(EnvironmentError::ClockBeforeEpoch);
    let mapped = map_clock_to_port(failing);
    assert!(mapped.is_err(), "clock failure must not yield a timestamp");
    assert!(
        !mapped.expect_err("checked").to_string().is_empty(),
        "PortError::other must carry the failure message"
    );
}
