//! M4 host execution tests for compiled `shamir-lang` scalar modules,
//! driven through the real engine (`WasmFunction::call`) like
//! `module_tests.rs`.
//!
//! Honesty note: `WasmFunction::call` serializes `Params` through
//! `QueryValue` and decodes the guest output into a `QueryValue`, so raw
//! input/output BYTES are not observable through this harness (the same
//! limitation `module_tests.rs` documents). Every assertion here checks the
//! DECODED canonical value.
//!
//! Marker derivations (host surface, `crates/wasm/shamir-wasm-host/src/
//! wasm/bindings/raise.rs`):
//! - kind 1 -> `FunctionError::User(message)`
//! - kind 2 -> `FunctionError::Compute("guest invalid argument: {message}")`
//! - kind 3 -> `FunctionError::Compute("guest codec: {message}")`
//! - kind 4 -> `FunctionError::Compute("guest host: {message}")`
//!
//! Guest-side markers (compiler emission): wrapper envelope validation
//! `b"arg"`/`b"mixed"`/`b"arity"` (`back/emit/module.rs` WRAPPER_MESSAGES,
//! raised at kind 2 by `abi.rs::raise`); checked arithmetic `b"ovf"` /
//! `b"div0"` (`back/emit/scalar/arith.rs`); checked casts `b"range"`
//! (`arith.rs`); bounds `b"bound"` and `unwrap` `b"unwrap"`
//! (`back/emit/scalar/insts.rs`); codec rejections `b"nonfin"`/`b"dup"`
//! (`back/runtime/codec/decode/reject.rs`); re-entry `b"reentry"` at kind 4
//! (`abi.rs`) — the re-entry guard is unreachable through this harness
//! because every call runs on a fresh Store (see `module_tests.rs`).
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, TierMode, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_lang::{compile, CompileOptions};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

/// Compile a fixture and load it through the M1 harness (fresh engine per
/// call site, nothing cached; `compile()` already self-admits, so the load
/// policy is simply `Load`).
async fn compiled(source: &str) -> WasmFunction {
    let module = compile(source, &CompileOptions::default()).expect("fixture compiles");
    let engine = Arc::new(WasmEngine::new().expect("engine constructed"));
    let (function, _report) = WasmFunction::from_binary_admitted_blocking(
        engine,
        &module.wasm,
        WasmLimits::default(),
        TierMode::Auto,
        AdmissionPolicy::Load,
    )
    .await
    .expect("compiled module admitted and loaded");
    function
}

fn params_from(entries: &[(&str, QueryValue)]) -> Params {
    let map: TMap<String, QueryValue> = entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    Params::from_map(map)
}

async fn call_once(function: &WasmFunction, params: &Params) -> Result<QueryValue, FunctionError> {
    function.call(&FnCtx::new(), &FnBatch::new(), params).await
}

fn int(value: i64) -> QueryValue {
    QueryValue::Int(value)
}

fn f64_value(value: f64) -> QueryValue {
    QueryValue::F64(value)
}

fn str_value(value: &str) -> QueryValue {
    QueryValue::Str(value.to_string())
}

fn bin_value(bytes: &[u8]) -> QueryValue {
    QueryValue::Bin(bytes.to_vec())
}

fn list_value(items: &[QueryValue]) -> QueryValue {
    QueryValue::List(items.to_vec())
}

fn map_of(entries: &[(&str, QueryValue)]) -> QueryValue {
    let mut map = new_map();
    for (key, value) in entries {
        map.insert((*key).to_string(), value.clone());
    }
    QueryValue::Map(map)
}

/// kind 2 marker helper; marker format derived from `raise.rs`:
/// `FunctionError::Compute(format!("guest invalid argument: {message}"))`.
fn assert_invalid_argument(error: &FunctionError, marker: &str) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message.contains(&format!("guest invalid argument: {marker}"))),
        "expected invalid-argument marker {marker}, got {error}"
    );
}

/// kind 3 marker helper; marker format derived from `raise.rs`:
/// `FunctionError::Compute(format!("guest codec: {message}"))`.
fn assert_codec(error: &FunctionError, marker: &str) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message.contains(&format!("guest codec: {marker}"))),
        "expected codec marker {marker}, got {error}"
    );
}

// ---------------------------------------------------------------------------
// 1. Worked example (design "Worked examples / Scalar")
// ---------------------------------------------------------------------------

/// Design worked example: `{"a":3,"b":5}` result `08` -> decoded Int(8).
#[tokio::test]
async fn worked_example_named_input_yields_i64_8() {
    let function = compiled(
        "fn add(a: i64, b: i64) -> i64 { return a + b; }\n\
         @entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }\n",
    )
    .await;
    let result = call_once(&function, &params_from(&[("a", int(3)), ("b", int(5))]))
        .await
        .expect("named call succeeds");
    // Design: result `08` (i64 8) after `add(3, 5)`.
    assert_eq!(result, int(8));
}

/// Design worked example: positional `{"0":3,"1":5,"args":[3,5]}` -> same
/// result `08`; numeric keys are authoritative, redundant `args` is ignored.
#[tokio::test]
async fn worked_example_positional_input_with_redundant_args_yields_i64_8() {
    let function = compiled(
        "fn add(a: i64, b: i64) -> i64 { return a + b; }\n\
         @entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }\n",
    )
    .await;
    let params = params_from(&[
        ("0", int(3)),
        ("1", int(5)),
        ("args", list_value(&[int(3), int(5)])),
    ]);
    let result = call_once(&function, &params)
        .await
        .expect("positional call succeeds");
    // Design: numeric keys authoritative, `args` ignored, result `08`.
    assert_eq!(result, int(8));
}

// ---------------------------------------------------------------------------
// 2. Checked arithmetic (kind 2, arith.rs emits b"ovf"/b"div0")
// ---------------------------------------------------------------------------

/// add/sub/mul overflow and MIN/-1 div/rem all raise kind 2 "ovf"
/// (arith.rs: checked add/sub/mul and MIN/-1 div/rem raise the overflow
/// message `b"ovf"`); happy path returns the exact i64.
#[tokio::test]
async fn arithmetic_checked_operations_raise_ovf_or_compute_exactly() {
    let add = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a + b; }\n").await;
    let sub = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a - b; }\n").await;
    let mul = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a * b; }\n").await;
    let div = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a / b; }\n").await;
    let rem = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a % b; }\n").await;

    // i64::MAX + 1 overflows (checked add -> ovf).
    let error = call_once(&add, &params_from(&[("a", int(i64::MAX)), ("b", int(1))]))
        .await
        .expect_err("add overflow must raise");
    assert_invalid_argument(&error, "ovf");
    // i64::MIN - 1 underflows (checked sub -> ovf).
    let error = call_once(&sub, &params_from(&[("a", int(i64::MIN)), ("b", int(1))]))
        .await
        .expect_err("sub underflow must raise");
    assert_invalid_argument(&error, "ovf");
    // mul overflow (checked mul -> ovf).
    let error = call_once(
        &mul,
        &params_from(&[("a", int(1 << 40)), ("b", int(1 << 40))]),
    )
    .await
    .expect_err("mul overflow must raise");
    assert_invalid_argument(&error, "ovf");
    // MIN / -1 overflows (checked div -> ovf).
    let error = call_once(&div, &params_from(&[("a", int(i64::MIN)), ("b", int(-1))]))
        .await
        .expect_err("MIN/-1 division must raise");
    assert_invalid_argument(&error, "ovf");
    // MIN % -1 overflows (checked rem -> ovf).
    let error = call_once(&rem, &params_from(&[("a", int(i64::MIN)), ("b", int(-1))]))
        .await
        .expect_err("MIN/-1 remainder must raise");
    assert_invalid_argument(&error, "ovf");

    // Happy paths: exact checked results (3+5=8, 7-2=5, 6*7=42, 7/2=3, 7%2=1).
    assert_eq!(
        call_once(&add, &params_from(&[("a", int(3)), ("b", int(5))]))
            .await
            .expect("add succeeds"),
        int(8)
    );
    assert_eq!(
        call_once(&sub, &params_from(&[("a", int(7)), ("b", int(2))]))
            .await
            .expect("sub succeeds"),
        int(5)
    );
    assert_eq!(
        call_once(&mul, &params_from(&[("a", int(6)), ("b", int(7))]))
            .await
            .expect("mul succeeds"),
        int(42)
    );
    assert_eq!(
        call_once(&div, &params_from(&[("a", int(7)), ("b", int(2))]))
            .await
            .expect("div succeeds"),
        int(3)
    );
    assert_eq!(
        call_once(&rem, &params_from(&[("a", int(7)), ("b", int(2))]))
            .await
            .expect("rem succeeds"),
        int(1)
    );
}

/// Division by zero raises kind 2 "div0" for i64 and i32
/// (arith.rs: div/rem rhs == 0 raises `b"div0"`).
#[tokio::test]
async fn arithmetic_division_by_zero_raises_div0_for_i64_and_i32() {
    let i64_div = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a / b; }\n").await;
    let i32_div = compiled("@entry scalar probe(a: i32, b: i32) -> i32 { return a / b; }\n").await;
    let error = call_once(&i64_div, &params_from(&[("a", int(1)), ("b", int(0))]))
        .await
        .expect_err("i64 div by zero must raise");
    assert_invalid_argument(&error, "div0");
    let error = call_once(&i32_div, &params_from(&[("a", int(1)), ("b", int(0))]))
        .await
        .expect_err("i32 div by zero must raise");
    assert_invalid_argument(&error, "div0");
}

// ---------------------------------------------------------------------------
// 3. NaN semantics (IEEE internally, non-finite output rejected kind 3)
// ---------------------------------------------------------------------------

/// Internal NaN obeys IEEE comparisons: `(a/a) != (a/a)` is true and
/// `a/a == a/a` is false for a=0.0 (0/0 produces NaN; NaN != NaN per IEEE).
#[tokio::test]
async fn nan_internal_comparisons_follow_ieee_rules() {
    let neq =
        compiled("@entry scalar probe(a: f64) -> bool { return (a / a) != (a / a); }\n").await;
    let eq = compiled("@entry scalar probe(a: f64) -> bool { return a / a == a / a; }\n").await;
    let params = params_from(&[("a", f64_value(0.0))]);
    assert_eq!(
        call_once(&neq, &params).await.expect("neq succeeds"),
        QueryValue::Bool(true),
        "NaN != NaN is true"
    );
    assert_eq!(
        call_once(&eq, &params).await.expect("eq succeeds"),
        QueryValue::Bool(false),
        "NaN == NaN is false"
    );
}

/// Non-finite OUTPUT is rejected kind 3 "nonfin": the result encoder rejects
/// non-finite f64 at typed output boundaries (design section 155; reject.rs
/// maps FAIL_NONFINITE to `b"nonfin"`).
#[tokio::test]
async fn nan_output_rejected_as_nonfin_codec_error() {
    let function = compiled("@entry scalar probe() -> f64 { return 0.0 / 0.0; }\n").await;
    let error = call_once(&function, &Params::new()).await;
    let error = match error {
        Err(error) => error,
        Ok(_) => panic!("NaN output must raise"),
    };
    assert_codec(&error, "nonfin");
}

// ---------------------------------------------------------------------------
// 4. Explicit casts (widening exact, narrowing checked kind 2 "range")
// ---------------------------------------------------------------------------

/// Widening casts are exact: to_i64(i32), to_f64_i32, to_f64_i64.
#[tokio::test]
async fn widening_casts_are_exact() {
    let i = compiled("@entry scalar probe(x: i32) -> i64 { return to_i64(x); }\n").await;
    let f32 = compiled("@entry scalar probe(x: i32) -> f64 { return to_f64_i32(x); }\n").await;
    let f64c = compiled("@entry scalar probe(x: i64) -> f64 { return to_f64_i64(x); }\n").await;
    // Exact values: 5 -> 5i64; 3 -> 3.0f64; -7 -> -7.0f64.
    assert_eq!(
        call_once(&i, &params_from(&[("x", int(5))]))
            .await
            .expect("to_i64"),
        int(5)
    );
    assert_eq!(
        call_once(&f32, &params_from(&[("x", int(3))]))
            .await
            .expect("to_f64_i32"),
        f64_value(3.0)
    );
    assert_eq!(
        call_once(&f64c, &params_from(&[("x", int(-7))]))
            .await
            .expect("to_f64_i64"),
        f64_value(-7.0)
    );
}

/// to_i32(i64) in range narrows exactly; 10_000_000_000 is outside
/// [-2^31, 2^31) so the checked cast raises kind 2 "range"
/// (arith.rs: out-of-range checked casts raise `b"range"`).
#[tokio::test]
async fn narrowing_i64_to_i32_checks_range() {
    let function = compiled("@entry scalar probe(x: i64) -> i32 { return to_i32(x); }\n").await;
    // In range: -1 narrows exactly.
    assert_eq!(
        call_once(&function, &params_from(&[("x", int(-1))]))
            .await
            .expect("in-range to_i32 succeeds"),
        int(-1)
    );
    let error = call_once(&function, &params_from(&[("x", int(10_000_000_000))]))
        .await
        .expect_err("out-of-range to_i32 must raise");
    assert_invalid_argument(&error, "range");
}

/// to_i32_f64 rejects non-integral (1.5) and out-of-range (2147483648.0)
/// inputs with kind 2 "range" (design: reject nonintegral and values outside
/// [-2^31, 2^31); NaN input is unreachable through this harness because the
/// guest decoder already rejects non-finite floats with kind 3 "nonfin").
#[tokio::test]
async fn narrowing_f64_to_i32_checks_range_and_integral() {
    let function = compiled("@entry scalar probe(x: f64) -> i32 { return to_i32_f64(x); }\n").await;
    let error = call_once(&function, &params_from(&[("x", f64_value(2147483648.0))]))
        .await
        .expect_err("2^31 must raise");
    assert_invalid_argument(&error, "range");
    let error = call_once(&function, &params_from(&[("x", f64_value(1.5))]))
        .await
        .expect_err("non-integral must raise");
    assert_invalid_argument(&error, "range");
}

/// to_i64_f64 rejects 2^63 (outside [-2^63, 2^63)) with kind 2 "range".
#[tokio::test]
async fn narrowing_f64_to_i64_checks_range() {
    let function = compiled("@entry scalar probe(x: f64) -> i64 { return to_i64_f64(x); }\n").await;
    // In range: 42.0 narrows exactly.
    assert_eq!(
        call_once(&function, &params_from(&[("x", f64_value(42.0))]))
            .await
            .expect("in-range to_i64_f64 succeeds"),
        int(42)
    );
    let error = call_once(
        &function,
        &params_from(&[("x", f64_value(9223372036854775808.0))]),
    )
    .await
    .expect_err("2^63 must raise");
    assert_invalid_argument(&error, "range");
}

// ---------------------------------------------------------------------------
// 5. Optionals (missing key = null, unwrap checks, optional results)
// ---------------------------------------------------------------------------

const OPTIONAL_BRANCH_SOURCE: &str = "@entry scalar probe(a: i64?, b: i64) -> i64 {\n\
    if (is_null(a)) {\n      return b;\n    }\n    return unwrap(a) + b;\n  }\n";

/// An omitted optional named key is treated as null -> `b` branch (5).
#[tokio::test]
async fn optional_param_omitted_key_behaves_as_null() {
    let function = compiled(OPTIONAL_BRANCH_SOURCE).await;
    assert_eq!(
        call_once(&function, &params_from(&[("b", int(5))]))
            .await
            .expect("omitted optional succeeds"),
        int(5)
    );
}

/// An explicit null optional takes the null branch.
#[tokio::test]
async fn optional_param_explicit_null_takes_null_branch() {
    let function = compiled(OPTIONAL_BRANCH_SOURCE).await;
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("a", QueryValue::Null), ("b", int(5))])
        )
        .await
        .expect("explicit null succeeds"),
        int(5)
    );
}

/// A present optional takes the unwrap branch (3 + 5 = 8).
#[tokio::test]
async fn optional_param_present_value_takes_unwrap_branch() {
    let function = compiled(OPTIONAL_BRANCH_SOURCE).await;
    assert_eq!(
        call_once(&function, &params_from(&[("a", int(3)), ("b", int(5))]))
            .await
            .expect("present optional succeeds"),
        int(8)
    );
}

/// An optional result `i64?` decodes null to QueryValue::Null and 5 to Int(5).
#[tokio::test]
async fn optional_result_decodes_null_and_value() {
    let function = compiled(
        "@entry scalar probe(flag: bool) -> i64? {\n\
         if (flag) { return 5; }\n      return null;\n    }\n",
    )
    .await;
    assert_eq!(
        call_once(&function, &params_from(&[("flag", QueryValue::Bool(true))]))
            .await
            .expect("present result succeeds"),
        int(5)
    );
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("flag", QueryValue::Bool(false))])
        )
        .await
        .expect("null result succeeds"),
        QueryValue::Null
    );
}

/// unwrap(null) raises kind 2 "unwrap" (insts.rs: null unwrap raises
/// `b"unwrap"` at kind 2).
#[tokio::test]
async fn unwrap_of_null_raises_unwrap_marker() {
    let function = compiled("@entry scalar probe(a: i64?) -> i64 { return unwrap(a); }\n").await;
    let error = call_once(&function, &params_from(&[("a", QueryValue::Null)]))
        .await
        .expect_err("unwrap(null) must raise");
    assert_invalid_argument(&error, "unwrap");
}

// ---------------------------------------------------------------------------
// 6. Strings and bytes
// ---------------------------------------------------------------------------

/// String equality and ordering follow UTF-8 byte order; len counts bytes.
#[tokio::test]
async fn strings_equality_ordering_and_len() {
    let eq =
        compiled("@entry scalar probe(s: string, t: string) -> bool { return s == t; }\n").await;
    let lt =
        compiled("@entry scalar probe(s: string, t: string) -> bool { return s < t; }\n").await;
    let len = compiled("@entry scalar probe(s: string) -> i32 { return len(s); }\n").await;
    assert_eq!(
        call_once(
            &eq,
            &params_from(&[("s", str_value("ab")), ("t", str_value("ab"))])
        )
        .await
        .expect("eq succeeds"),
        QueryValue::Bool(true)
    );
    assert_eq!(
        call_once(
            &eq,
            &params_from(&[("s", str_value("ab")), ("t", str_value("ac"))])
        )
        .await
        .expect("neq succeeds"),
        QueryValue::Bool(false)
    );
    // UTF-8 byte order: "abc" < "abd" ("c" = 0x63 < "d" = 0x64).
    assert_eq!(
        call_once(
            &lt,
            &params_from(&[("s", str_value("abc")), ("t", str_value("abd"))])
        )
        .await
        .expect("lt succeeds"),
        QueryValue::Bool(true)
    );
    // é is two UTF-8 bytes (0xC3 0xA9); "é" > "z" since 0xC3 > 0x7A.
    assert_eq!(
        call_once(
            &lt,
            &params_from(&[("s", str_value("é")), ("t", str_value("z"))])
        )
        .await
        .expect("byte-order lt succeeds"),
        QueryValue::Bool(false)
    );
    assert_eq!(
        call_once(&len, &params_from(&[("s", str_value("héllo"))]))
            .await
            .expect("len succeeds"),
        int(6)
    );
}

/// bytes equality compares contents: identical b"..." literals are equal,
/// differing contents are not.
#[tokio::test]
async fn bytes_equality_compares_contents() {
    let function =
        compiled("@entry scalar probe(x: bytes, y: bytes) -> bool { return x == y; }\n").await;
    assert_eq!(
        call_once(
            &function,
            &params_from(&[
                ("x", bin_value(&[0x01, 0x02])),
                ("y", bin_value(&[0x01, 0x02]))
            ])
        )
        .await
        .expect("equal bytes succeeds"),
        QueryValue::Bool(true)
    );
    assert_eq!(
        call_once(
            &function,
            &params_from(&[
                ("x", bin_value(&[0x01, 0x02])),
                ("y", bin_value(&[0x01, 0x03]))
            ])
        )
        .await
        .expect("unequal bytes succeeds"),
        QueryValue::Bool(false)
    );
}

// ---------------------------------------------------------------------------
// 7. Lists
// ---------------------------------------------------------------------------

/// List literal + indexing returns the element (xs[1] == 20).
#[tokio::test]
async fn list_literal_indexing_returns_element() {
    let function = compiled(
        "@entry scalar probe(xs: [i64]) -> i64 {\n\
         let copy: [i64] = [1, 2, 3];\n      return xs[1] + copy[2];\n    }\n",
    )
    .await;
    // xs[1] = 20 plus copy[2] = 3 -> 23.
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("xs", list_value(&[int(10), int(20), int(30)]))])
        )
        .await
        .expect("indexing succeeds"),
        int(23)
    );
}

/// An index equal to the length raises kind 2 "bound" (insts.rs: bounds check
/// raises `b"bound"`); `index == length` is the off-by-one boundary.
#[tokio::test]
async fn list_index_equal_to_length_raises_bound_marker() {
    let function = compiled(
        "@entry scalar probe(xs: [i64]) -> i64 { return xs[1]; }
",
    )
    .await;
    let error = call_once(&function, &params_from(&[("xs", list_value(&[int(1)]))]))
        .await
        .expect_err("index == length must raise");
    assert_invalid_argument(&error, "bound");
}

/// Index zero of an empty list is out of bounds too.
#[tokio::test]
async fn empty_list_index_zero_raises_bound_marker() {
    let function = compiled(
        "@entry scalar probe(xs: [i64]) -> i64 { return xs[0]; }
",
    )
    .await;
    let error = call_once(&function, &params_from(&[("xs", list_value(&[]))]))
        .await
        .expect_err("empty list index 0 must raise");
    assert_invalid_argument(&error, "bound");
}

/// The last valid index (length - 1) still reads.
#[tokio::test]
async fn list_last_valid_index_reads() {
    let function = compiled(
        "@entry scalar probe(xs: [i64]) -> i64 { return xs[1]; }
",
    )
    .await;
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("xs", list_value(&[int(7), int(9)]))])
        )
        .await
        .expect("last index succeeds"),
        int(9)
    );
}

/// for-in sums the elements; len returns the list length as i32.
#[tokio::test]
async fn list_for_in_sum_and_len() {
    let sum = compiled(
        "@entry scalar probe(xs: [i64]) -> i64 {\n\
         let total: i64 = 0;\n      for (x in xs) { total = total + x; }\n\
         return total;\n    }\n",
    )
    .await;
    let len = compiled("@entry scalar probe(xs: [i64]) -> i32 { return len(xs); }\n").await;
    let items = [int(1), int(2), int(3), int(4)];
    // 1 + 2 + 3 + 4 = 10.
    assert_eq!(
        call_once(&sum, &params_from(&[("xs", list_value(&items))]))
            .await
            .expect("for-in sum succeeds"),
        int(10)
    );
    assert_eq!(
        call_once(&len, &params_from(&[("xs", list_value(&items))]))
            .await
            .expect("list len succeeds"),
        int(4)
    );
}

/// A list parameter round trips to a list result (identity projection).
#[tokio::test]
async fn list_param_and_result_round_trip() {
    let function = compiled("@entry scalar probe(xs: [i64]) -> [i64] { return xs; }\n").await;
    let items = list_value(&[int(-1), int(0), int(922337203685477580)]);
    assert_eq!(
        call_once(&function, &params_from(&[("xs", items.clone())]))
            .await
            .expect("list round trip succeeds"),
        items
    );
}

// ---------------------------------------------------------------------------
// 8. Records
// ---------------------------------------------------------------------------

/// A record param projects through QueryValue::Map fields (r.a + r.b).
#[tokio::test]
async fn record_param_fields_project_to_i64_sum() {
    let function =
        compiled("@entry scalar probe(r: { a: i64, b: i64 }) -> i64 { return r.a + r.b; }\n").await;
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("r", map_of(&[("a", int(3)), ("b", int(4))]))])
        )
        .await
        .expect("record param succeeds"),
        int(7)
    );
}

/// A record RESULT built in the guest decodes to QueryValue::Map equality
/// ({x: 3, y: 6} for a=3).
#[tokio::test]
async fn record_result_decodes_to_expected_map() {
    let function = compiled(
        "@entry scalar probe(a: i64) -> { x: i64, y: i64 } {\n\
         let r: { x: i64, y: i64 } = { x: a, y: a * 2 };\n      return r;\n    }\n",
    )
    .await;
    assert_eq!(
        call_once(&function, &params_from(&[("a", int(3))]))
            .await
            .expect("record result succeeds"),
        map_of(&[("x", int(3)), ("y", int(6))])
    );
}

/// Nested field access reaches inner record fields (r.inner.v).
#[tokio::test]
async fn nested_record_field_access() {
    let function =
        compiled("@entry scalar probe(r: { inner: { v: i64 } }) -> i64 { return r.inner.v; }\n")
            .await;
    let inner = map_of(&[("v", int(7))]);
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("r", map_of(&[("inner", inner)]))])
        )
        .await
        .expect("nested access succeeds"),
        int(7)
    );
}

// ---------------------------------------------------------------------------
// 9. Calls and loops
// ---------------------------------------------------------------------------

/// Nested helper calls compose: add(mul(3, 4), 5) = 17.
#[tokio::test]
async fn nested_helper_calls_compose() {
    let function = compiled(
        "fn add(a: i64, b: i64) -> i64 { return a + b; }\n\
         fn mul(a: i64, b: i64) -> i64 { return a * b; }\n\
         @entry scalar probe(a: i64, b: i64, c: i64) -> i64 { return add(mul(a, b), c); }\n",
    )
    .await;
    // mul(3,4)=12, add(12,5)=17.
    assert_eq!(
        call_once(
            &function,
            &params_from(&[("a", int(3)), ("b", int(4)), ("c", int(5))])
        )
        .await
        .expect("nested calls succeed"),
        int(17)
    );
}

/// while with break and continue: n=7 skips i==2 and breaks at i==5, so the
/// total is 1+3+4 = 8.
#[tokio::test]
async fn while_loop_break_and_continue_compute_value() {
    let function = compiled(
        "@entry scalar probe(n: i64) -> i64 {\n\
         let total: i64 = 0;\n      let i: i64 = 0;\n\
         while (i < n) {\n        i = i + 1;\n\
         if (i == 2) { continue; }\n        if (i == 5) { break; }\n\
         total = total + i;\n      }\n      return total;\n    }\n",
    )
    .await;
    assert_eq!(
        call_once(&function, &params_from(&[("n", int(7))]))
            .await
            .expect("while loop succeeds"),
        int(8)
    );
}

/// A half-open range for sums 0..5 = 10 (range 0..5 iterates 0,1,2,3,4).
#[tokio::test]
async fn range_for_sum() {
    let function = compiled(
        "@entry scalar probe() -> i64 {\n\
         let total: i64 = 0;\n      for (i in 0..5) { total = total + i; }\n\
         return total;\n    }\n",
    )
    .await;
    assert_eq!(
        call_once(&function, &Params::new())
            .await
            .expect("range for succeeds"),
        int(10)
    );
}

// ---------------------------------------------------------------------------
// 10. Throw (kind 1)
// ---------------------------------------------------------------------------

/// `throw "boom";` raises kind 1; raise.rs maps kind 1 to
/// `FunctionError::User(message)`, so only containment of "boom" is asserted.
#[tokio::test]
async fn throw_raises_kind_1_user_error_containing_message() {
    let function = compiled(
        "@entry scalar probe(flag: bool) -> i64 {\n\
         if (flag) { throw \"boom\"; }\n      return 1;\n    }\n",
    )
    .await;
    let result = call_once(
        &function,
        &params_from(&[("flag", QueryValue::Bool(false))]),
    )
    .await
    .expect("no-throw path succeeds");
    assert_eq!(result, int(1));
    let error = call_once(&function, &params_from(&[("flag", QueryValue::Bool(true))]))
        .await
        .expect_err("throw must raise");
    assert!(
        matches!(error, FunctionError::User(ref message) if message.contains("boom")),
        "expected kind-1 user error containing boom, got {error}"
    );
}

// ---------------------------------------------------------------------------
// 11. Short-circuit evaluation
// ---------------------------------------------------------------------------

/// `&&` only skips the right side when the LEFT is false: (a=0, b=0) makes
/// `a == 0` true, so `1 / b` EXECUTES and raises the kind-2 div0 error.
#[tokio::test]
async fn logical_and_true_left_executes_right_operand() {
    let function =
        compiled("@entry scalar probe(a: i64, b: i64) -> bool { return a == 0 && 1 / b == 1; }\n")
            .await;
    let error = call_once(&function, &params_from(&[("a", int(0)), ("b", int(0))]))
        .await
        .expect_err("executed right operand must raise div0");
    assert_invalid_argument(&error, "div0");
}

/// (a=1, b=0): `a == 0` is false, so the right operand is SKIPPED and `&&`
/// yields false without touching `1 / b` (no div0 error).
#[tokio::test]
async fn logical_and_false_left_short_circuits() {
    let function =
        compiled("@entry scalar probe(a: i64, b: i64) -> bool { return a == 0 && 1 / b == 1; }\n")
            .await;
    assert_eq!(
        call_once(&function, &params_from(&[("a", int(1)), ("b", int(0))]))
            .await
            .expect("false left operand short-circuits"),
        QueryValue::Bool(false)
    );
}

// ---------------------------------------------------------------------------
// 12. Named envelope validation (kind 2 wrapper markers)
// ---------------------------------------------------------------------------

/// Unknown named key raises kind 2 "arg" (abi.rs: unknown key raises the
/// wrapper `b"arg"` message at kind 2).
#[tokio::test]
async fn named_envelope_unknown_key_rejected() {
    let function = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a + b; }\n").await;
    let error = call_once(&function, &params_from(&[("a", int(3)), ("z", int(9))]))
        .await
        .expect_err("unknown key must raise");
    assert_invalid_argument(&error, "arg");
}

/// A partial named call (missing required key) raises kind 2 "arity"
/// (abi.rs: missing required key raises `b"arity"`).
#[tokio::test]
async fn named_envelope_missing_key_rejected_as_arity() {
    let function = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a + b; }\n").await;
    let error = call_once(&function, &params_from(&[("a", int(3))]))
        .await
        .expect_err("missing key must raise");
    assert_invalid_argument(&error, "arity");
}

/// Mixing positional and named keys raises kind 2 "mixed"
/// (abi.rs: mixed envelope raises `b"mixed"`).
#[tokio::test]
async fn named_envelope_mixed_keys_rejected_as_mixed() {
    let function = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a + b; }\n").await;
    let error = call_once(&function, &params_from(&[("0", int(3)), ("a", int(5))]))
        .await
        .expect_err("mixed envelope must raise");
    assert_invalid_argument(&error, "mixed");
}

/// A complete named envelope succeeds: 3 + 4 = 7.
#[tokio::test]
async fn named_envelope_complete_call_succeeds() {
    let function = compiled("@entry scalar probe(a: i64, b: i64) -> i64 { return a + b; }\n").await;
    assert_eq!(
        call_once(&function, &params_from(&[("a", int(3)), ("b", int(4))]))
            .await
            .expect("complete named call succeeds"),
        int(7)
    );
}

// ---------------------------------------------------------------------------
// 13. Differential against an independent Rust reference evaluator
// ---------------------------------------------------------------------------

/// Differential test: an inline xorshift64 PRNG (state update
/// `x ^= x << 13; x ^= x >> 7; x ^= x << 17;`, seed 0x2545F4914F6CDD1D)
/// generates 40 random (op, operand) pairs; operands are `next() % 2001 -
/// 1000` in [-1000, 1000]. The independent reference evaluates the same
/// operation with Rust `checked_add`/`checked_sub`/`checked_mul`: Ok(v)
/// must decode to QueryValue::Int(v); Err must raise the kind-2 overflow
/// marker ("guest invalid argument: ovf", derived from raise.rs kind 2 and
/// arith.rs `b"ovf"`). No compiler code is reused in the reference.
#[tokio::test]
async fn differential_checked_arithmetic_matches_rust_reference() {
    let ops = ["+", "-", "*"];
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for case in 0..40 {
        let op = ops[(next() % 3) as usize];
        // Map to a symmetric signed range via i64 remainder (biased, but
        // deterministic and bounded — fine for a PRNG-seeded test vector).
        let a = ((next() % 2001) as i64) - 1000;
        let b = ((next() % 2001) as i64) - 1000;
        let source = format!("@entry scalar probe(a: i64, b: i64) -> i64 {{ return a {op} b; }}\n");
        let function = compiled(&source).await;
        let result = call_once(&function, &params_from(&[("a", int(a)), ("b", int(b))])).await;
        let reference = match op {
            "+" => a.checked_add(b),
            "-" => a.checked_sub(b),
            _ => a.checked_mul(b),
        };
        match reference {
            Some(expected) => assert_eq!(
                result.unwrap_or_else(|error| panic!(
                    "case {case} ({a} {op} {b}) must succeed: {error:?}"
                )),
                int(expected),
                "case {case}: guest {a} {op} {b} diverged from reference"
            ),
            None => {
                let error = result
                    .err()
                    .unwrap_or_else(|| panic!("case {case}: {a} {op} {b} must raise overflow"));
                assert_invalid_argument(&error, "ovf");
            }
        }
    }
}
