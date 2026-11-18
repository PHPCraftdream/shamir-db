//! Unit tests for the bounded same-volume rename-retry policy
//! ([`rename_with_retry`]), with the rename injected as a closure.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::boot::config::diagnostics::logical_import::stage::dir::rename_with_retry;

/// Windows-transient sharing-violation error, as the OS reports it.
fn sharing_violation() -> io::Error {
    io::Error::from_raw_os_error(32)
}

fn absent_target() -> &'static Path {
    Path::new("definitely-absent-target")
}

#[tokio::test]
async fn transient_error_retried_until_success() {
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in = calls.clone();
    let rename = move || {
        let n = calls_in.fetch_add(1, Ordering::SeqCst) + 1;
        if n < 4 {
            Err(sharing_violation())
        } else {
            Ok(())
        }
    };
    rename_with_retry(rename, absent_target(), &CancellationToken::new())
        .await
        .expect("transient errors must be retried until success");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn permanent_error_fails_immediately() {
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in = calls.clone();
    let rename = move || {
        calls_in.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::from_raw_os_error(2))
    };
    let result = rename_with_retry(rename, absent_target(), &CancellationToken::new()).await;
    let err = result.expect_err("a permanent error must not be retried");
    assert_eq!(err.raw_os_error(), Some(2));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one attempt");
}

#[tokio::test]
async fn exhausted_budget_fails_naming_attempts() {
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in = calls.clone();
    let rename = move || {
        calls_in.fetch_add(1, Ordering::SeqCst);
        Err(sharing_violation())
    };
    let result = rename_with_retry(rename, absent_target(), &CancellationToken::new()).await;
    assert!(result.is_err(), "exhausted retries must fail");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        super::super::dir::RENAME_ATTEMPTS,
        "the rename ran exactly RENAME_ATTEMPTS times"
    );
}

#[tokio::test]
async fn cancellation_between_attempts_stops_the_retry() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in = calls.clone();
    let rename = move || {
        calls_in.fetch_add(1, Ordering::SeqCst);
        Err(sharing_violation())
    };
    assert!(
        rename_with_retry(rename, absent_target(), &cancel)
            .await
            .is_err(),
        "a cancelled retry must fail, not sleep"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no attempt after cancel");
}

#[tokio::test]
async fn retry_never_runs_against_an_existing_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("target");
    std::fs::create_dir(&target).expect("create target");
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in = calls.clone();
    let rename = move || {
        calls_in.fetch_add(1, Ordering::SeqCst);
        Err(sharing_violation())
    };
    assert!(
        rename_with_retry(rename, &target, &CancellationToken::new())
            .await
            .is_err(),
        "an appeared target must fail the rename immediately"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no attempt on existing target"
    );
}
