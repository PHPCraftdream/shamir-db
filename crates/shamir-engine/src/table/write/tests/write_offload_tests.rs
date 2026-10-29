use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use shamir_funclib::registry::{FnEntry, ScalarError, ScalarRegistry};
use shamir_query_types::filter::{FilterValue, FnCall};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::super::write_helpers::eval_write_value;

fn assert_ticker_progress(ticker: &AtomicU64) {
    let before = ticker.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(500));
    let after = ticker.load(Ordering::Relaxed);
    assert!(
        after.saturating_sub(before) >= 3,
        "ticker stalled: {before} -> {after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn computed_write_argon_keeps_worker_live() {
    let ticker = Arc::new(AtomicU64::new(0));
    let ticks = Arc::clone(&ticker);
    let ticker_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
            ticks.fetch_add(1, Ordering::Relaxed);
        }
    });
    let result = tokio::spawn(async move {
        let mut registry = ScalarRegistry::new();
        registry.register(
            "crypto/argon2id",
            FnEntry::pure(
                move |_| {
                    assert_ticker_progress(&ticker);
                    Ok(QueryValue::Int(7))
                },
                0,
                Some(0),
            ),
        );
        let expr = FilterValue::FnCall {
            call: FnCall::complex("crypto/argon2id", vec![]),
        };
        eval_write_value(&expr, &new_map(), &registry)
    })
    .await;
    ticker_task.abort();
    let _ = ticker_task.await;
    assert_eq!(result.unwrap(), Ok(QueryValue::Int(7)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn computed_write_argon_error_propagates() {
    let mut registry = ScalarRegistry::new();
    registry.register(
        "crypto/argon2id",
        FnEntry::pure(|_| Err(ScalarError::new("probe_error")), 0, Some(0)),
    );
    let expr = FilterValue::FnCall {
        call: FnCall::complex("crypto/argon2id", vec![]),
    };
    assert_eq!(
        eval_write_value(&expr, &new_map(), &registry),
        Err("crypto/argon2id: probe_error".to_owned())
    );
}
