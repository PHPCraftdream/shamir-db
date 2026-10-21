//! Regression tests for [`NativeValidatorAdapter::call`]'s accept-path
//! round-trip skip (audit group 25, defect 4).
//!
//! `native_adapter.rs` used to run `validation_to_query_value` on EVERY
//! invocation, even when the closure accepted the record (no errors) — pure
//! waste, since `decode_validation_result` already treats `QueryValue::Null`
//! as "valid, no errors, stop=false". These tests lock down:
//! - the accept (no-error, no-stop) path now returns `QueryValue::Null`
//!   directly (proving the Map-encode round trip is genuinely skipped: a
//!   `Null` result could only come from the shortcut, never from
//!   `validation_to_query_value`, which always builds a `Map`);
//! - the reject path (errors present) is unchanged (still the `Map` form);
//! - an accept-with-`stop` result (no errors, but `stop = true`) must NOT
//!   take the `Null` shortcut, since `Null` decodes to `stop = false` and
//!   would silently drop the stop request.

use shamir_types::types::value::QueryValue;
use shamir_wasm_host::{FnBatch, FnCtx, Params, ShamirFunction};

use crate::validator::{decode_validation_result, NativeValidatorAdapter, Validation};

fn call_params() -> Params {
    let mut params = Params::new();
    params.set("record", QueryValue::Null);
    params.set("old_record", QueryValue::Null);
    params
}

#[tokio::test]
async fn accept_path_skips_round_trip_and_returns_null() {
    let adapter = NativeValidatorAdapter::new(|_record, _old, _ctx| Validation::accept());
    let ctx = FnCtx::new();
    let batch = FnBatch::new();

    let result = adapter
        .call(&ctx, &batch, &call_params())
        .await
        .expect("accept call should not error");

    // `validation_to_query_value` always produces a `Map` — getting `Null`
    // back is direct proof the encode step was skipped, not just that the
    // decoded outcome happens to be empty.
    assert_eq!(result, QueryValue::Null);

    // And decoding it still yields the correct (empty, non-stop) outcome —
    // the shortcut must be behaviourally identical to the old round trip.
    let outcome = decode_validation_result(&result).expect("Null decodes cleanly");
    assert!(outcome.errors.is_empty());
    assert!(!outcome.stop);
}

#[tokio::test]
async fn reject_path_still_encodes_full_map() {
    let adapter = NativeValidatorAdapter::new(|_record, _old, _ctx| {
        let mut v = Validation::accept();
        v.field_error(vec!["name".to_string()], "required");
        v
    });
    let ctx = FnCtx::new();
    let batch = FnBatch::new();

    let result = adapter
        .call(&ctx, &batch, &call_params())
        .await
        .expect("reject call should not error");

    // Errors present — must NOT take the Null shortcut.
    assert!(matches!(result, QueryValue::Map(_)));

    let outcome = decode_validation_result(&result).expect("map decodes cleanly");
    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "required");
    assert_eq!(
        outcome.errors[0].field.as_deref(),
        Some(&["name".to_string()][..])
    );
    assert!(!outcome.stop);
}

#[tokio::test]
async fn accept_with_stop_is_not_shortcut_to_null() {
    // No errors, but `stop = true` — the Null shortcut would silently lose
    // the stop request (Null always decodes to stop=false), so this MUST
    // still go through the full Map encode.
    let adapter = NativeValidatorAdapter::new(|_record, _old, _ctx| {
        let mut v = Validation::accept();
        v.stop();
        v
    });
    let ctx = FnCtx::new();
    let batch = FnBatch::new();

    let result = adapter
        .call(&ctx, &batch, &call_params())
        .await
        .expect("accept-with-stop call should not error");

    assert!(
        matches!(result, QueryValue::Map(_)),
        "accept-with-stop must not take the Null shortcut, got {result:?}"
    );

    let outcome = decode_validation_result(&result).expect("map decodes cleanly");
    assert!(outcome.errors.is_empty());
    assert!(outcome.stop);
}
