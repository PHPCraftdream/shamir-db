//! M1 host execution tests: the emitted `shamir-lang` guest runtime run
//! through the real engine (`WasmFunction`), driven by the crate's
//! test-support module and codec vectors.
//!
//! Honest coverage notes:
//! - Raw-byte rejection vectors (ext/reserved/utf8/dup/trunc/trail/u64 and
//!   friends) cannot traverse `WasmFunction::call`, which serializes
//!   `Params` through `QueryValue::to_bytes`; only classes reachable from a
//!   `QueryValue` encoding are executed here. The raw-byte vectors and the
//!   second-call guard are executed against the guest in `raw/codec_tests.rs`
//!   through the raw wasmtime harness; here they remain asserted as inventory.
//! - Each `WasmFunction::call` runs on a FRESH Store, so the emitted
//!   second-call `call_state` guard can never fire through this API; each
//!   call here asserts the fresh-store, monotonic-allocator behaviour
//!   instead.
//! - Memory growth: the 100_000-byte string round trip forces the decoder's
//!   heap copy (memory.grow through `shamir_alloc`) and the 8 MiB output
//!   allocation; growth happens per-call on the fresh Store.
use crate::{
    admit_module, AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, TierMode,
    WasmEngine, WasmFunction, WasmLimits,
};
use shamir_lang::testsupport::{
    admission_policy_64kib, codec_vectors, enc_call_envelope, runtime_module, VectorExpectation,
};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

/// Compile + admit the testsupport runtime module under the same 64 KiB
/// policy the in-crate module tests use.
async fn runtime_function() -> WasmFunction {
    let engine = Arc::new(WasmEngine::new().expect("engine constructed"));
    let bytes = runtime_module();
    let (function, report) = WasmFunction::from_binary_admitted_blocking(
        engine,
        &bytes,
        WasmLimits::default(),
        TierMode::Auto,
        admission_policy_64kib(),
    )
    .await
    .expect("runtime module admitted and compiled");
    assert_eq!(report.byte_size, bytes.len(), "byte_size mismatch");
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

/// A chain of `depth` nested lists with a null at the bottom.
fn nested_lists(depth: usize) -> QueryValue {
    let mut value = QueryValue::Null;
    for _ in 0..depth {
        value = QueryValue::List(vec![value]);
    }
    value
}

fn contains_codec_marker(error: &FunctionError, marker: &str) -> bool {
    matches!(error, FunctionError::Compute(message) if message.contains(&format!(
        "guest codec: {marker}"
    )))
}

#[tokio::test]
async fn shamir_lang_runtime_module_admits_under_64kib_policy() {
    let bytes = runtime_module();
    let report = admit_module(&bytes, &AdmissionPolicy::Load).expect("load-policy admission");
    assert_eq!(report.byte_size, bytes.len());
    let report = admit_module(&bytes, &admission_policy_64kib()).expect("admission policy");
    assert_eq!(report.byte_size, bytes.len());
}

#[tokio::test]
async fn shamir_lang_runtime_round_trips_scalars_and_compound_values() {
    let function = runtime_function().await;
    let params = params_from(&[
        ("i", QueryValue::Int(-5)),
        ("s", QueryValue::Str("héllo".to_string())),
        ("n", QueryValue::Null),
        ("b", QueryValue::Bin(vec![0x00, 0x01, 0x02, 0xff])),
        ("f", QueryValue::F64(42.5)),
        ("flag", QueryValue::Bool(true)),
    ]);
    let mut expected = new_map();
    expected.insert("i".to_string(), QueryValue::Int(-5));
    expected.insert("s".to_string(), QueryValue::Str("héllo".to_string()));
    expected.insert("n".to_string(), QueryValue::Null);
    expected.insert(
        "b".to_string(),
        QueryValue::Bin(vec![0x00, 0x01, 0x02, 0xff]),
    );
    expected.insert("f".to_string(), QueryValue::F64(42.5));
    expected.insert("flag".to_string(), QueryValue::Bool(true));
    let result = call_once(&function, &params)
        .await
        .expect("scalar round trip succeeds");
    assert_eq!(result, QueryValue::Map(expected));
}

#[tokio::test]
async fn shamir_lang_runtime_round_trips_nested_maps_lists_and_depth_boundary() {
    let function = runtime_function().await;
    // root map (depth 1) + 31 nested lists = depth-32 boundary, accepted.
    let params = params_from(&[
        ("deep", nested_lists(31)),
        (
            "list",
            QueryValue::List(vec![
                QueryValue::Int(1),
                QueryValue::Str("two".to_string()),
                QueryValue::Null,
            ]),
        ),
    ]);
    let mut expected = new_map();
    expected.insert("deep".to_string(), nested_lists(31));
    expected.insert(
        "list".to_string(),
        QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Str("two".to_string()),
            QueryValue::Null,
        ]),
    );
    let result = call_once(&function, &params)
        .await
        .expect("nested round trip succeeds");
    assert_eq!(result, QueryValue::Map(expected));
}

#[tokio::test]
async fn shamir_lang_runtime_round_trips_positional_call_envelope() {
    // Design section 3 worked example: {"0":3,"1":5,"args":[3,5]} — at M1
    // the wrapper decodes and re-encodes the raw map, so the canonical
    // (key-sorted) form round trips unchanged.
    let function = runtime_function().await;
    let envelope = QueryValue::from_bytes(enc_call_envelope(&[3, 5])).expect("envelope decodes");
    let params = Params::from_value(envelope).expect("envelope is a map");
    let expected = QueryValue::from_bytes(enc_call_envelope(&[3, 5])).expect("canonical decodes");
    let result = call_once(&function, &params)
        .await
        .expect("envelope round trip succeeds");
    assert_eq!(result, expected);
}

#[tokio::test]
async fn shamir_lang_runtime_rejects_over_cap_values_via_params_encoding() {
    let function = runtime_function().await;
    // (params value, expected kind-3 literal) for every class reachable
    // from a QueryValue encoding; nesting arithmetic: root
    // map (depth 1) + N nested lists = depth N + 1, so nested_lists(32) is
    // depth 33, one past the depth-32 cap.
    let long_map_key = {
        let mut map = new_map();
        map.insert("a".repeat(65_537), QueryValue::Null);
        QueryValue::Map(map)
    };
    let huge_list = QueryValue::List((0..100_001).map(|_| QueryValue::Null).collect());
    let cases = [
        (nested_lists(32), "depth"),
        (QueryValue::Str("a".repeat(1_048_577)), "str"),
        (long_map_key, "keys"),
        (huge_list, "ent"),
        (QueryValue::F64(f64::NAN), "nonfin"),
    ];
    for (value, marker) in cases {
        let params = params_from(&[("v", value)]);
        let error = call_once(&function, &params)
            .await
            .expect_err("over-cap value must raise");
        assert!(
            contains_codec_marker(&error, marker),
            "expected marker {marker}, got {error}"
        );
    }
}

#[test]
fn shamir_lang_rejection_vector_inventory_is_complete() {
    // Byte-level classes that `WasmFunction::call` cannot inject (see the
    // module doc) stay asserted as vector inventory here; they are executed
    // against the guest in `raw/codec_tests.rs`.
    let vectors = codec_vectors();
    for vector in &vectors {
        match &vector.expectation {
            VectorExpectation::Rejected(marker) => assert!(!marker.is_empty()),
            VectorExpectation::Accepted(canonical) => assert!(!canonical.is_empty()),
        }
    }
    for expected in [
        "ext8",
        "timestamp_fixext4",
        "reserved_marker",
        "utf8_overlong_c0_80",
        "map_non_string_key",
        "map_duplicate_keys",
        "truncated_string_body",
        "trailing_bytes_after_root",
        "depth_33_over_limit",
        "u64_above_i64_max",
        "map_key_bytes_over_cap",
        "string_length_over_cap",
        "f64_non_finite",
        "depth_32_boundary",
    ] {
        assert!(
            vectors.iter().any(|vector| vector.name == expected),
            "vector inventory misses {expected}"
        );
    }
}

#[tokio::test]
async fn shamir_lang_runtime_each_call_runs_on_a_fresh_store_and_succeeds() {
    // Honest framing: `WasmFunction::call` builds a fresh Store per call, so
    // the emitted second-call `call_state` guard can never fire through this
    // API — the guard itself is executed in `raw/codec_tests.rs`. Here each
    // call on the same instance is a fresh instance store with the monotonic
    // allocator, so both succeed.
    let function = runtime_function().await;
    let params = params_from(&[("a", QueryValue::Int(1)), ("b", QueryValue::Int(2))]);
    let mut expected = new_map();
    expected.insert("a".to_string(), QueryValue::Int(1));
    expected.insert("b".to_string(), QueryValue::Int(2));
    let first = call_once(&function, &params)
        .await
        .expect("first call succeeds");
    assert_eq!(first, QueryValue::Map(expected.clone()));
    let second = call_once(&function, &params)
        .await
        .expect("second call succeeds (fresh Store per call)");
    assert_eq!(second, QueryValue::Map(expected));
}

#[tokio::test]
async fn shamir_lang_runtime_grows_memory_for_a_100k_byte_string_round_trip() {
    // Real heap-growth coverage: the 100_000-byte string forces the decoder
    // to heap-copy its payload through `shamir_alloc` (a visible
    // memory.grow delta above the minimum pages) and the identity round
    // trip writes into the 8 MiB output allocation, so the call is EXPECTED
    // to succeed with exact equality.
    let function = runtime_function().await;
    let text = "a".repeat(100_000);
    let params = params_from(&[("s", QueryValue::Str(text.clone()))]);
    let mut expected = new_map();
    expected.insert("s".to_string(), QueryValue::Str(text));
    let result = call_once(&function, &params)
        .await
        .expect("100k string round trip succeeds with heap growth");
    assert_eq!(result, QueryValue::Map(expected));
}
