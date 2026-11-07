//! Regression tests for argon2 scalar dispatch from filter resolution.

use crate::query::filter::{resolve_filter_query, FilterContext, FilterValue};
use shamir_funclib::registry::{v_str, FnEntry};
use shamir_funclib::scalar_resolver::{
    ScalarResolver, UserScalarLayer, ARGON2_INLINE_ON_RUNTIME, ARGON2_INLINE_WARNING_EMITTED,
};
use shamir_query_types::filter::FnCall;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn resolver() -> ScalarResolver {
    let layer = Arc::new(UserScalarLayer::new());
    layer.register(
        "crypto/argon2id",
        FnEntry::pure(
            |_| Ok(v_str(format!("{:?}", std::thread::current().id()))),
            0,
            Some(0),
        ),
    );
    layer.register(
        "thread_probe",
        FnEntry::pure(
            |_| Ok(v_str(format!("{:?}", std::thread::current().id()))),
            0,
            Some(0),
        ),
    );
    ScalarResolver::new(layer)
}

fn call(name: &str, scalars: ScalarResolver) -> Option<QueryValue> {
    let interner = Interner::new();
    let refs = new_map();
    let mut ctx = FilterContext::new(&interner, &refs);
    ctx.scalars = scalars;
    let value = FilterValue::FnCall {
        call: FnCall::complex(name, vec![]),
    };
    resolve_filter_query(&value, &InnerValue::Null, &ctx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn argon_dispatch_keeps_worker_live() {
    let ticker = Arc::new(AtomicU64::new(0));
    let ticks = Arc::clone(&ticker);
    let ticker_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
            ticks.fetch_add(1, Ordering::Relaxed);
        }
    });
    let layer = Arc::new(UserScalarLayer::new());
    layer.register(
        "crypto/argon2id",
        FnEntry::pure(
            move |_| {
                crate::assert_ticker_progress(&ticker);
                Ok(v_str("ok".into()))
            },
            0,
            Some(0),
        ),
    );
    let result =
        tokio::spawn(async move { call("crypto/argon2id", ScalarResolver::new(layer)) }).await;
    ticker_task.abort();
    let _ = ticker_task.await;
    assert_eq!(result.unwrap(), Some(v_str("ok".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn rv_funclib_8b0c96fb_argon_current_thread_documents_inline_limitation() {
    let before = ARGON2_INLINE_ON_RUNTIME.load(Ordering::Relaxed);
    let caller = format!("{:?}", std::thread::current().id());
    match call("crypto/argon2id", resolver()) {
        Some(QueryValue::Str(id)) => assert_eq!(id, caller),
        other => panic!("unexpected scalar result: {other:?}"),
    }
    assert!(ARGON2_INLINE_ON_RUNTIME.load(Ordering::Relaxed) > before);
    assert!(ARGON2_INLINE_WARNING_EMITTED.load(Ordering::Relaxed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_argon_builtin_or_custom_scalar_stays_inline() {
    let caller = format!("{:?}", std::thread::current().id());
    match call("thread_probe", resolver()) {
        Some(QueryValue::Str(id)) => assert_eq!(id, caller),
        other => panic!("unexpected scalar result: {other:?}"),
    }
}
