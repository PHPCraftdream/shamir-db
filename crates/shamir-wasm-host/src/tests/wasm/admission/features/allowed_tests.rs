use crate::tests::wasm::admission::support::{
    admit, build_and_run, limits_with, module_with_call_body,
};
use crate::{Tier, TierMode};

/// The feature executes inside `shamir_call`: the module is admitted, builds
/// and runs under `Auto`, and builds and runs on the optimized tier under `Now`.
async fn assert_feature_admitted_and_runs(extra_items: &str, call_body: &str) {
    let bytes = module_with_call_body(extra_items, call_body);
    let report = admit(&bytes, limits_with(|_| {}));
    assert!(report.is_ok(), "admission failed: {report:?}");
    // The Auto tier may be Fast or Optimized; only the call must succeed.
    build_and_run(&bytes, TierMode::Auto).await;
    assert_eq!(build_and_run(&bytes, TierMode::Now).await, Tier::Optimized);
}

#[tokio::test]
async fn sign_extension_admitted_and_runs() {
    assert_feature_admitted_and_runs("", "(drop (i32.extend8_s (i32.const 255)))").await;
}

#[tokio::test]
async fn saturating_float_to_int_admitted_and_runs() {
    assert_feature_admitted_and_runs("", "(drop (i32.trunc_sat_f64_s (f64.const 1e30)))").await;
}

#[tokio::test]
async fn bulk_memory_copy_and_fill_admitted_and_runs() {
    let body = "(memory.fill (i32.const 4096) (i32.const 7) (i32.const 16)) \
                (memory.copy (i32.const 4112) (i32.const 4096) (i32.const 16))";
    assert_feature_admitted_and_runs("", body).await;
}

#[tokio::test]
async fn bulk_memory_init_from_passive_data_admitted_and_runs() {
    let body = "(memory.init $d (i32.const 4096) (i32.const 0) (i32.const 4))";
    assert_feature_admitted_and_runs(r#"(data $d "abcd")"#, body).await;
}

#[tokio::test]
async fn multi_value_admitted_and_runs() {
    let items = "(func $two (result i32 i32) (i32.const 1) (i32.const 2))";
    assert_feature_admitted_and_runs(items, "call $two drop drop").await;
}

#[tokio::test]
async fn mutable_globals_admitted_and_run() {
    let items = "(global $g (mut i32) (i32.const 0))";
    let body = "(global.set $g (i32.const 9)) (drop (global.get $g))";
    assert_feature_admitted_and_runs(items, body).await;
}

#[tokio::test]
async fn funcref_call_indirect_admitted_and_runs() {
    let items = "(type $t (func)) (func $f (type $t)) (table 1 funcref) \
                 (elem (i32.const 0) func $f)";
    let body = "(call_indirect (type $t) (i32.const 0))";
    assert_feature_admitted_and_runs(items, body).await;
}

#[tokio::test]
async fn floating_point_admitted_and_runs() {
    let body = "(drop (f64.add (f64.const 1.5) (f64.mul (f64.const 2.0) (f64.const 3.0))))";
    assert_feature_admitted_and_runs("", body).await;
}
