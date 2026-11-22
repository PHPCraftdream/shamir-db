//! Deterministic unit seams for two PR4 shell behaviours that the
//! integration suites can only observe end-to-end (design `admin-shell.md`
//! §5/§6): the deadline re-consulted AFTER the last blocking step, and the
//! activity paging cap.
//!
//! Both are deterministic by construction: `output::deadline_check` takes
//! the loop's start `Instant` and an already-projected outcome, so the
//! overdue case is produced by moving the start instant BACKWARDS by a
//! whole `REQUEST_TIMEOUT` (no sleep, no runtime); `activity::collect_pages`
//! takes its fetch closure, so the page cap is proven with a counting fake
//! that always returns ONE row plus a continuation.

use std::cell::Cell;
use std::time::Duration;

use shamir_query_types::admin::activity::{ActivityPage, ActivityRow};

use crate::shell::commands::activity::collect_pages;
use crate::shell::output::{deadline_check, ShellError};

/// Hard hang bound for the paging walk — a hang DETECTOR, not a latency
/// assertion.
const BOUND: Duration = Duration::from_secs(120);

/// One fake session row; the paging rule only counts rows, so every leaf is
/// a fixed, harmless value.
fn session_row(handle: u64) -> ActivityRow {
    ActivityRow::Session {
        session_handle: handle,
        user: String::from("pager"),
        transport: String::from("tcp"),
        created_at_ns: 1,
        idle_ns: 0,
        open_transactions: 0,
        open_cursors: 0,
    }
}

/// An elapsed budget refuses even a SUCCESSFUL projection: the deadline is
/// re-consulted after the last await, so a command that overran
/// `REQUEST_TIMEOUT` reports the typed timeout (exit 7) instead of a
/// late-but-successful answer.
#[test]
fn expired_budget_refuses_even_a_successful_projection() {
    let started = tokio::time::Instant::now() - crate::shell::connect::REQUEST_TIMEOUT;
    assert_eq!(
        deadline_check(started, Ok(1u8)),
        Err(ShellError::Timeout),
        "an elapsed budget must refuse a successful projection"
    );
}

/// A live budget returns the projection unchanged, on both arms: the
/// deadline is a bound, never a rewrite of the outcome.
#[test]
fn live_budget_returns_the_projection() {
    let started = tokio::time::Instant::now();
    assert_eq!(
        deadline_check(started, Ok(7u8)),
        Ok(7u8),
        "a live budget must return the projection"
    );
    let refusal: Result<u8, ShellError> = deadline_check(started, Err(ShellError::Server));
    assert_eq!(
        refusal,
        Err(ShellError::Server),
        "a live budget must preserve the projected refusal"
    );
}

/// Reaching the row cap WHILE a continuation is still live ends the walk in
/// that same iteration: the page is truncated and no third request is
/// issued. Mutant: dropping the `rows.len() >= cap` stop issues a third
/// request (which a server-side cap would answer with an empty page, hiding
/// the truncation).
#[test]
fn cap_reached_with_a_continuation_stops_without_another_request() {
    let requests = Cell::new(0usize);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("paging test runtime");
    let (rows, truncated) = runtime.block_on(async {
        let (rows, truncated) = tokio::time::timeout(
            BOUND,
            collect_pages(2, 10, |_continuation| {
                requests.set(requests.get() + 1);
                std::future::ready(Ok(ActivityPage::new(
                    vec![session_row(requests.get() as u64)],
                    Some(String::from("next-page")),
                )))
            }),
        )
        .await
        .expect("collect_pages hang bound")
        .expect("collect_pages succeeds");
        (rows.len(), truncated)
    });
    assert_eq!(rows, 2, "the walk must stop exactly at the row cap");
    assert!(
        truncated,
        "a cap stop with a live continuation is truncated"
    );
    assert_eq!(
        requests.get(),
        2,
        "the cap stop must not issue another request"
    );
}

/// An exhausted listing (one row, NO continuation) is complete, not
/// truncated.
#[test]
fn exhausted_listing_is_not_truncated() {
    let requests = Cell::new(0usize);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("paging test runtime");
    let (rows, truncated) = runtime.block_on(async {
        let (rows, truncated) = tokio::time::timeout(
            BOUND,
            collect_pages(4, 10, |_continuation| {
                requests.set(requests.get() + 1);
                std::future::ready(Ok(ActivityPage::new(vec![session_row(1)], None)))
            }),
        )
        .await
        .expect("collect_pages hang bound")
        .expect("collect_pages succeeds");
        (rows.len(), truncated)
    });
    assert_eq!(rows, 1, "the one-row page must be returned whole");
    assert!(!truncated, "an exhausted listing is not truncated");
    assert_eq!(requests.get(), 1, "an exhausted listing is one request");
}
