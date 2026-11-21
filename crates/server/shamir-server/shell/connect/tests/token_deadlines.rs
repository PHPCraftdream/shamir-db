//! Deterministic paused-time checks of the token connect phase budgets and
//! the private `authenticate` horizon helper.

use std::time::Duration;

use shamir_client::ClientError;
use tokio::time::{timeout, Instant};

use super::{wss_connect_error, ConnectBudget};
use crate::shell::output::ShellError;

/// DIAL ORACLE: DNS + TCP get exactly `start + 10 s`.
#[test]
fn dial_deadline_is_ten_seconds_after_start() {
    let now = Instant::now();
    let budget = ConnectBudget::start(now);
    assert_eq!(budget.dial_deadline(), now + Duration::from_secs(10));
}

/// AUTH ORACLE: a dial finishing at 9.5 s leaves exactly 20.5 s of the
/// single absolute `start + 30 s` horizon — a late dial consumes the TLS/
/// auth budget, the total connect cap never widens. Independent literal
/// oracles: `20_500` and `30_000`, not re-derivations from the constants.
#[test]
fn late_dial_consumes_the_auth_budget() {
    let now = Instant::now();
    let budget = ConnectBudget::start(now);
    let dial_finished = now + Duration::from_millis(9500);
    assert_eq!(
        budget
            .auth_deadline()
            .saturating_duration_since(dial_finished),
        Duration::from_millis(20_500)
    );
    // The horizon stays absolute: unchanged no matter when it is read.
    assert_eq!(budget.auth_deadline(), now + Duration::from_secs(30));
}

/// TLS running past the dial budget is NOT cut at 10 s — its deadline is
/// the 30 s horizon, strictly after the dial's.
#[test]
fn late_tls_is_not_cut_at_the_dial_deadline() {
    let now = Instant::now();
    let budget = ConnectBudget::start(now);
    let dial_deadline = budget.dial_deadline();
    assert!(dial_deadline < budget.auth_deadline());
    assert_eq!(
        budget
            .auth_deadline()
            .saturating_duration_since(dial_deadline),
        Duration::from_secs(20)
    );
}

/// WSS upgrade answered HTTP 408 BEFORE any channel exists: connection
/// failure, not the REST post-dispatch Timeout.
#[test]
fn wss_upgrade_408_is_connection() {
    let error = ClientError::Http {
        status: 408,
        code: None,
        retry_after: None,
    };
    assert_eq!(wss_connect_error(error), ShellError::Connection);
}

/// Deterministic paused-time probe of [`ConnectBudget::authenticate`]: the
/// simulated TLS handshake ends at 11 s (past the 10 s dial budget, so the
/// auth phase is NOT cut at 10 s), the marker proves it, then a never-ready
/// authentication is cut exactly at 30 s with the typed connection failure
/// — under a generous outer hang bound.
#[tokio::test(start_paused = true)]
async fn authenticate_is_cut_exactly_at_the_thirty_second_horizon() {
    let start = Instant::now();
    let budget = ConnectBudget::start(start);
    let mut tls_reached = false;
    let outcome = timeout(
        Duration::from_secs(120),
        budget.authenticate(async {
            // Simulated TLS handshake.
            tokio::time::sleep(Duration::from_secs(11)).await;
            tls_reached = true;
            // Timer-wheel deadlines round up to 1 ms.
            let at = Instant::now() - start;
            assert!(at >= Duration::from_secs(11), "at {at:?}");
            assert!(at <= Duration::from_millis(11_050), "at {at:?}");
            // Authentication never answers.
            std::future::pending::<Result<(), ClientError>>().await
        }),
    )
    .await
    .expect("outer hang bound");
    assert!(tls_reached, "the simulated TLS handshake must complete");
    assert!(
        matches!(outcome, Err(ShellError::Connection)),
        "expected typed connection failure, got {outcome:?}"
    );
    // Timer-wheel deadlines round up to 1 ms, so allow one tick of drift.
    let elapsed = Instant::now() - start;
    assert!(elapsed >= Duration::from_secs(30), "elapsed {elapsed:?}");
    assert!(
        elapsed <= Duration::from_millis(30_050),
        "elapsed {elapsed:?}"
    );
}
