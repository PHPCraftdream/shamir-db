//! M5 integration tests: the real `WasmRecordValidator` adapter driven by
//! `shamir-lang`-compiled validator sources through the real host engine.
//!
//! Chain under test: `shamir_lang::compile` → `WasmEngine::new` →
//! `WasmFunction::from_binary_admitted_blocking` → `WasmRecordValidator`
//! → `decode_validation_result` → [`Validation`].
//!
//! One-line derivations per assertion cite the compiler/adapter/decoder
//! behavior that produces the expected value.
use shamir_lang::{compile, CompileOptions};
use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

use crate::validator::{
    OwnedFields, RecordValidator, Validation, ValidationError, ValidatorCtx, WasmRecordValidator,
};
use shamir_wasm_host::{
    AdmissionPolicy, ShamirFunction, TierMode, WasmEngine, WasmFunction, WasmLimits,
};

/// The design's worked example: null record → record-level error, empty
/// name → field-bound error, otherwise accept.
const REQUIRE_NAME: &str = r#"@entry validator require_name(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  if (is_null(record)) {
    return record_error("record_required");
  }
  let current: { name: string } = unwrap(record);
  if (current.name == "") {
    return reject("name", "name_empty");
  }
  return accept();
}"#;

/// Stop source: `stop(with_error(...))` appends a field-bound error and
/// sets the stop flag on the guest's `Validation`.
const STOPPER: &str = r#"@entry validator stopper(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  return stop(with_error(accept(), "name", "stopped"));
}"#;

/// Compiles a validator source and loads it through the real host engine,
/// wrapped as the adapter's function object.
///
/// Loading pattern mirrors
/// `crates/wasm/shamir-wasm-host/src/tests/runtime/lang/scalar_tests.rs`
/// (`compile()` already self-admits, so the load policy is `Load`).
async fn compiled_validator(source: &str) -> Arc<dyn ShamirFunction> {
    let module = compile(source, &CompileOptions::default()).expect("validator source compiles");
    let engine = Arc::new(WasmEngine::new().expect("engine constructed"));
    let (function, _report) = WasmFunction::from_binary_admitted_blocking(
        engine,
        &module.wasm,
        WasmLimits::default(),
        TierMode::Auto,
        AdmissionPolicy::Load,
    )
    .await
    .expect("compiled module loads");
    let function: Arc<dyn ShamirFunction> = Arc::new(function);
    function
}

/// Build a `Validation`-shaped record map (`QueryValue::Map`) from
/// `("field", QueryValue)` pairs, mirroring the `new_map()` fixture pattern.
fn record_map(entries: &[(&str, QueryValue)]) -> QueryValue {
    let mut map: TMap<String, QueryValue> = new_map();
    for (key, value) in entries {
        map.insert((*key).to_string(), value.clone());
    }
    QueryValue::Map(map)
}

#[tokio::test]
async fn adapter_insert_accepts_a_named_record() {
    let validator = WasmRecordValidator::new(compiled_validator(REQUIRE_NAME).await);
    let interner = Interner::new();
    let actor = Actor::System;
    let ctx = ValidatorCtx::new(&actor, &interner);

    // Extra undeclared `id` field proves the adapter projects the whole map
    // and the guest ignores undeclared keys (schema-typed record projection).
    let qv = record_map(&[
        ("name", QueryValue::Str("ada".to_string())),
        ("id", QueryValue::Int(7)),
    ]);
    let fields = OwnedFields { qv: &qv };

    // Derivation: INSERT has old_record absent → adapter passes
    // `QueryValue::Null` for `old_record` → `accept()` → empty errors,
    // stop=false (`Validation` derives PartialEq, codecs/encode.rs:22).
    let got = validator.validate(Some(&fields), None, &ctx).await;
    assert_eq!(got, Validation::default());
}

#[tokio::test]
async fn adapter_update_rejects_an_empty_name() {
    let validator = WasmRecordValidator::new(compiled_validator(REQUIRE_NAME).await);
    let interner = Interner::new();
    let actor = Actor::System;
    let ctx = ValidatorCtx::new(&actor, &interner);

    let qv_new = record_map(&[("name", QueryValue::Str("".to_string()))]);
    let qv_old = record_map(&[("name", QueryValue::Str("bob".to_string()))]);
    let fields_new = OwnedFields { qv: &qv_new };
    let fields_old = OwnedFields { qv: &qv_old };

    // Derivation: the worked example returns reject("name", "name_empty")
    // for the empty name; the compiler emits error maps key-ordered
    // code-then-field and the decoder decodes `field` as the one-segment
    // path `["name"]` (wire form `List<Str>`, validation_error.rs:15).
    let got = validator
        .validate(Some(&fields_new), Some(&fields_old), &ctx)
        .await;
    assert_eq!(
        got,
        Validation {
            errors: vec![ValidationError {
                field: Some(vec!["name".to_string()]),
                code: "name_empty".to_string(),
            }],
            stop: false,
        }
    );
}

#[tokio::test]
async fn adapter_delete_reports_a_record_level_error() {
    let validator = WasmRecordValidator::new(compiled_validator(REQUIRE_NAME).await);
    let interner = Interner::new();
    let actor = Actor::System;
    let ctx = ValidatorCtx::new(&actor, &interner);

    let qv = record_map(&[("name", QueryValue::Str("ada".to_string()))]);
    let fields = OwnedFields { qv: &qv };

    // Derivation: DELETE passes record=null → `record_error` branch emits a
    // code-only error, so the decoded `field` is `None` (no field path on
    // record-level errors, validation_error.rs:14).
    let got = validator.validate(None, Some(&fields), &ctx).await;
    assert_eq!(
        got,
        Validation {
            errors: vec![ValidationError {
                field: None,
                code: "record_required".to_string(),
            }],
            stop: false,
        }
    );
}

#[tokio::test]
async fn adapter_stop_propagates_through_the_decoder() {
    let validator = WasmRecordValidator::new(compiled_validator(STOPPER).await);
    let interner = Interner::new();
    let actor = Actor::System;
    let ctx = ValidatorCtx::new(&actor, &interner);

    let qv = record_map(&[("name", QueryValue::Str("ada".to_string()))]);
    let fields = OwnedFields { qv: &qv };

    // Derivation: stop(with_error(accept(), "name", "stopped")) preserves the
    // appended field-bound error and sets stop=true; the decoder maps the
    // guest's "stop" bool to ValidationOutcome.stop (adapter
    // wasm_record_validator.rs:86-89).
    let got = validator.validate(Some(&fields), None, &ctx).await;
    assert_eq!(
        got,
        Validation {
            errors: vec![ValidationError {
                field: Some(vec!["name".to_string()]),
                code: "stopped".to_string(),
            }],
            stop: true,
        }
    );
}
