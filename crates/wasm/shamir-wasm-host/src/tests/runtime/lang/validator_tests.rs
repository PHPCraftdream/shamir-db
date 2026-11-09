//! M5 host execution tests for compiled `shamir-lang` `@entry validator`
//! modules.
//!
//! Part A drives a raw wasmtime harness so the design's manual byte vectors
//! are observable byte for byte (`WasmFunction::call` decodes into
//! `QueryValue` and cannot see raw bytes — the limitation `scalar_tests.rs`
//! and `module_tests.rs` document). Part B drives the real
//! `WasmFunction::call` harness for decoded-value coverage.
//!
//! Every raw-byte expectation carries a one-line `// derivation:` comment
//! (design manual vector, encoder layout, or lowering cap); compilation and
//! execution run in CI only, so nothing here is verified locally.
//!
//! Marker derivations (host surface, `crates/wasm/shamir-wasm-host/src/
//! wasm/bindings/raise.rs`): kind 2 maps to
//! `FunctionError::Compute("guest invalid argument: {message}")`.
//! Guest-side markers used here: projection failures `b"field"`/`b"arg"`
//! and the validation-error-list cap `b"bound"`
//! (`crates/wasm/shamir-lang/src/back/lower/types.rs`,
//! `VALIDATION_ERROR_CAP = 64`; raise at kind 2).
use super::raw::harness::{hex, raw_call, raw_module};
use crate::{
    AdmissionPolicy, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, TierMode, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_lang::{compile, CompileOptions};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Part A — raw-byte harness (design "Worked examples / Typed validator")
// ---------------------------------------------------------------------------

/// The design's worked-example validator source (contains `"` literals, so
/// a raw string with `#` delimiters is required).
const WORKED_EXAMPLE: &str = r#"@entry validator require_name(
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
}
"#;

/// Design manual vector (insertion, named envelope): record = {"name":"Ada"},
/// old_record = null.
const INSERTION_INPUT: &str = "82 a6 72 65 63 6f 72 64 81 a4 6e 61 6d 65 a3 41 64 61 \
aa 6f 6c 64 5f 72 65 63 6f 72 64 c0";

/// Design manual vector (empty-name reject): record = {"name":""},
/// old_record = null.
const EMPTY_NAME_INPUT: &str = "82 a6 72 65 63 6f 72 64 81 a4 6e 61 6d 65 a0 \
aa 6f 6c 64 5f 72 65 63 6f 72 64 c0";

/// Design manual vector (deletion): record = null,
/// old_record = {"name":"Ada"}.
const DELETION_INPUT: &str = "82 a6 72 65 63 6f 72 64 c0 \
aa 6f 6c 64 5f 72 65 63 6f 72 64 81 a4 6e 61 6d 65 a3 41 64 61";

/// design manual vector; produced by the record encoder: map(2) 'errors' [] 'stop' false
const ACCEPT_OUTPUT: &str = "82 a6 65 72 72 6f 72 73 90 a4 73 74 6f 70 c2";

/// design manual vector; record encoder: map(2), 'errors' fixarray(1),
/// error map(2) 'code' "name_empty" 'field' ["name"], 'stop' false
const EMPTY_NAME_OUTPUT: &str = "82 a6 65 72 72 6f 72 73 91 82 a4 63 6f 64 65 \
aa 6e 61 6d 65 5f 65 6d 70 74 79 a5 66 69 65 6c 64 91 a4 6e 61 6d 65 a4 73 74 6f 70 c2";

/// design manual vector; record encoder: map(2), 'errors' fixarray(1),
/// code-only error map(1) 'code' "record_required" (field omitted),
/// 'stop' false
const DELETION_OUTPUT: &str = "82 a6 65 72 72 6f 72 73 91 81 a4 63 6f 64 65 \
af 72 65 63 6f 72 64 5f 72 65 71 75 69 72 65 64 a4 73 74 6f 70 c2";

/// The insertion vector must produce the design's accept bytes EXACTLY.
#[tokio::test]
async fn raw_worked_example_insertion_accepts_byte_for_byte() {
    let module = compile(WORKED_EXAMPLE, &CompileOptions::default()).expect("fixture compiles");
    let mut raw = raw_module(&module.wasm).await;
    let output = raw_call(&mut raw, &hex(INSERTION_INPUT))
        .await
        .expect("raw call succeeds");
    // derivation: design manual vector; produced by the record encoder:
    // map(2) 'errors' [] 'stop' false
    assert_eq!(output, hex(ACCEPT_OUTPUT));
}

/// The empty-name vector must produce the design's reject bytes EXACTLY.
#[tokio::test]
async fn raw_worked_example_empty_name_rejects_byte_for_byte() {
    let module = compile(WORKED_EXAMPLE, &CompileOptions::default()).expect("fixture compiles");
    let mut raw = raw_module(&module.wasm).await;
    let output = raw_call(&mut raw, &hex(EMPTY_NAME_INPUT))
        .await
        .expect("raw call succeeds");
    // derivation: design manual vector; error map key order 'code' then
    // 'field', field path ["name"], 'stop' false.
    assert_eq!(output, hex(EMPTY_NAME_OUTPUT));
}

/// The deletion vector must produce the design's record-required bytes
/// EXACTLY (record-level error OMITS the `field` key — error map is fixmap 1).
#[tokio::test]
async fn raw_worked_example_deletion_reports_record_required_byte_for_byte() {
    let module = compile(WORKED_EXAMPLE, &CompileOptions::default()).expect("fixture compiles");
    let mut raw = raw_module(&module.wasm).await;
    let output = raw_call(&mut raw, &hex(DELETION_INPUT))
        .await
        .expect("raw call succeeds");
    // derivation: design manual vector; record-level error is a one-entry
    // map (0x81) so `field` is absent.
    assert_eq!(output, hex(DELETION_OUTPUT));
}

// ---------------------------------------------------------------------------
// Part B — decoded-value coverage through the real WasmFunction::call harness
// ---------------------------------------------------------------------------

/// Compile a fixture and load it through the M1 harness (own local copy; each
/// test file in this crate is self-contained, mirroring `scalar_tests.rs`).
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

fn str_value(value: &str) -> QueryValue {
    QueryValue::Str(value.to_string())
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

/// kind 2 marker helper (own copy); derived from `raise.rs`:
/// `FunctionError::Compute(format!("guest invalid argument: {message}"))`.
fn assert_invalid_argument(error: &FunctionError, marker: &str) {
    assert!(
        matches!(error, FunctionError::Compute(message)
            if message.contains(&format!("guest invalid argument: {marker}"))),
        "expected invalid-argument marker {marker}, got {error}"
    );
}

/// The canonical accept result: `{"errors": [], "stop": false}` with key
/// order `errors` then `stop`.
fn accept_result() -> QueryValue {
    // derivation: validator.rs module docs — canonical result is
    // { "errors": [], "stop": bool } in that key order.
    let mut map = new_map();
    map.insert("errors".to_string(), QueryValue::List(Vec::new()));
    map.insert("stop".to_string(), QueryValue::Bool(false));
    QueryValue::Map(map)
}

/// A field error: `{"code": code, "field": [segments]}` in that key order.
fn field_error(code: &str, field: &[&str]) -> QueryValue {
    // derivation: types.rs `field_error_descriptor` — fields "code", "field".
    let mut map = new_map();
    map.insert("code".to_string(), str_value(code));
    map.insert(
        "field".to_string(),
        QueryValue::List(field.iter().map(|segment| str_value(segment)).collect()),
    );
    QueryValue::Map(map)
}

/// A code-only (record-level) error: `{"code": code}` with NO `field` key.
fn code_error(code: &str) -> QueryValue {
    // derivation: types.rs code-error descriptor — the single field "code".
    let mut map = new_map();
    map.insert("code".to_string(), str_value(code));
    QueryValue::Map(map)
}

/// `{"errors": errors, "stop": stop}` in canonical key order.
fn validation_result(errors: Vec<QueryValue>, stop: bool) -> QueryValue {
    let mut map = new_map();
    map.insert("errors".to_string(), QueryValue::List(errors));
    map.insert("stop".to_string(), QueryValue::Bool(stop));
    QueryValue::Map(map)
}

/// Insertion, update, and deletion envelopes against the worked example:
/// insertion and update accept; deletion reports the code-only
/// `record_required` error with no `field` key.
#[tokio::test]
async fn insertion_update_deletion_envelopes() {
    let function = compiled(WORKED_EXAMPLE).await;

    // Insertion: the undeclared adapter field `id` is IGNORED by projection
    // (validator.rs module docs: undeclared fields are ignored), so accept.
    let insertion = params_from(&[
        (
            "record",
            map_of(&[("name", str_value("Ada")), ("id", int(7))]),
        ),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &insertion)
            .await
            .expect("insertion succeeds"),
        accept_result()
    );

    // Update: both records present with declared `name` fields -> accept.
    let update = params_from(&[
        ("record", map_of(&[("name", str_value("Ada"))])),
        ("old_record", map_of(&[("name", str_value("Bob"))])),
    ]);
    assert_eq!(
        call_once(&function, &update)
            .await
            .expect("update succeeds"),
        accept_result()
    );

    // Deletion: record null -> code-only `record_required` error, stop false.
    let deletion = params_from(&[
        ("record", QueryValue::Null),
        ("old_record", map_of(&[("name", str_value("Ada"))])),
    ]);
    let result = call_once(&function, &deletion)
        .await
        .expect("deletion call succeeds");
    assert_eq!(
        result,
        validation_result(vec![code_error("record_required")], false)
    );
    // Explicitly confirm the record-level error carries no `field` key
    // (derivation: code-error descriptor has only the "code" field).
    let QueryValue::Map(ref decoded) = result else {
        panic!("validation result must decode to a map");
    };
    let Some(QueryValue::List(errors)) = decoded.get("errors") else {
        panic!("validation result must carry an `errors` list");
    };
    let Some(QueryValue::Map(error)) = errors.first() else {
        panic!("errors must contain at least one error map");
    };
    assert!(
        !error.contains_key("field"),
        "record-level error must omit `field`, got {error:?}"
    );
}

/// Positional numeric-key envelopes select the parameter slots directly:
/// `"0"` -> record, `"1"` -> old_record. The redundant `args` key variant is
/// also accepted because numeric keys are authoritative (scalar wrapper
/// envelope scan shared by the validator wrapper per validator.rs).
#[tokio::test]
async fn positional_envelope_selects_slots() {
    let function = compiled(WORKED_EXAMPLE).await;
    let numeric = params_from(&[
        ("0", map_of(&[("name", str_value("Ada"))])),
        ("1", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &numeric)
            .await
            .expect("positional call succeeds"),
        accept_result()
    );
    let with_args = params_from(&[
        ("0", map_of(&[("name", str_value("Ada"))])),
        ("1", QueryValue::Null),
        (
            "args",
            list_value(&[map_of(&[("name", str_value("Ada"))]), QueryValue::Null]),
        ),
    ]);
    assert_eq!(
        call_once(&function, &with_args)
            .await
            .expect("redundant-args call succeeds"),
        accept_result()
    );
}

/// A named envelope with `old_record` omitted reads the missing optional key
/// as null (scalar wrapper optional-parameter null fill), so the insertion
/// path accepts.
#[tokio::test]
async fn named_envelope_omitted_optional_key_reads_null() {
    let function = compiled(WORKED_EXAMPLE).await;
    let params = params_from(&[("record", map_of(&[("name", str_value("Ada"))]))]);
    assert_eq!(
        call_once(&function, &params)
            .await
            .expect("omitted key succeeds"),
        accept_result()
    );
}

/// Every validator result constructor decodes to its canonical shape, in
/// insertion order where multiple errors accumulate.
#[tokio::test]
async fn every_constructor_and_combinations() {
    // accept() -> no errors, stop false.
    let accept_fn = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return accept(); }\n",
    )
    .await;
    assert_eq!(
        call_once(&accept_fn, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("accept succeeds"),
        accept_result()
    );

    // reject("name", "name_empty") -> one field error on ["name"].
    let reject_fn = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return reject(\"name\", \"name_empty\"); }\n",
    )
    .await;
    // derivation: reject builds a field error with the single-segment path.
    assert_eq!(
        call_once(&reject_fn, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("reject succeeds"),
        validation_result(vec![field_error("name_empty", &["name"])], false)
    );

    // reject_path(["a", "b"], "nested_code") -> two-segment field path.
    let reject_path_fn = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return reject_path([\"a\", \"b\"], \"nested_code\"); }\n",
    )
    .await;
    assert_eq!(
        call_once(
            &reject_path_fn,
            &params_from(&[("record", QueryValue::Null)])
        )
        .await
        .expect("reject_path succeeds"),
        validation_result(vec![field_error("nested_code", &["a", "b"])], false)
    );

    // record_error("record_required") -> code-only error (no field key).
    let record_error_fn = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return record_error(\"record_required\"); }\n",
    )
    .await;
    assert_eq!(
        call_once(
            &record_error_fn,
            &params_from(&[("record", QueryValue::Null)])
        )
        .await
        .expect("record_error succeeds"),
        validation_result(vec![code_error("record_required")], false)
    );

    // with_error(accept(), "a", "c1") -> one field error appended.
    let with_one = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return with_error(accept(), \"a\", \"c1\"); }\n",
    )
    .await;
    assert_eq!(
        call_once(&with_one, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("with_error succeeds"),
        validation_result(vec![field_error("c1", &["a"])], false)
    );

    // Nested with_error appends IN ORDER: a/c1 first, then b/c2
    // (derivation: with_error appends to the end of the errors list).
    let with_two = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return with_error(with_error(accept(), \"a\", \"c1\"), \"b\", \"c2\"); }\n",
    )
    .await;
    assert_eq!(
        call_once(&with_two, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("nested with_error succeeds"),
        validation_result(
            vec![field_error("c1", &["a"]), field_error("c2", &["b"])],
            false
        )
    );

    // with_record_error over one field error: three errors, the LAST one
    // code-only (derivation: record_error appends a code-only record).
    let mixed = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return with_record_error(with_error(accept(), \"a\", \"c1\"), \"c3\"); }\n",
    )
    .await;
    assert_eq!(
        call_once(&mixed, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("with_record_error succeeds"),
        validation_result(vec![field_error("c1", &["a"]), code_error("c3")], false)
    );

    // stop(accept()) -> stop flips to true, errors stay empty.
    let stop_fn = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return stop(accept()); }\n",
    )
    .await;
    assert_eq!(
        call_once(&stop_fn, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("stop succeeds"),
        validation_result(Vec::new(), true)
    );

    // stop(with_error(accept(), "a", "c")) -> errors preserved AND stop true.
    let stop_with = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return stop(with_error(accept(), \"a\", \"c\")); }\n",
    )
    .await;
    assert_eq!(
        call_once(&stop_with, &params_from(&[("record", QueryValue::Null)]))
            .await
            .expect("stop with_error succeeds"),
        validation_result(vec![field_error("c", &["a"])], true)
    );
}

/// A record missing a REQUIRED declared field fails at projection with the
/// kind-2 `field` marker (derivation: the projection helper raises the
/// wrapper "field" message for a missing required declared field).
#[tokio::test]
async fn missing_required_declared_field_is_kind_2() {
    let function = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return accept(); }\n",
    )
    .await;
    let params = params_from(&[
        ("record", map_of(&[("id", int(7))])),
        ("old_record", QueryValue::Null),
    ]);
    let error = call_once(&function, &params)
        .await
        .expect_err("missing required field must raise");
    assert_invalid_argument(&error, "field");
}

/// A declared field decoded with the wrong type (int where `string` is
/// declared) fails at projection with the kind-2 `arg` marker.
#[tokio::test]
async fn wrongly_typed_field_is_kind_2() {
    let function = compiled(
        "@entry validator v(record: { name: string }?, old_record: { name: string }?) \
         -> Validation { return accept(); }\n",
    )
    .await;
    let params = params_from(&[
        ("record", map_of(&[("name", int(5))])),
        ("old_record", QueryValue::Null),
    ]);
    let error = call_once(&function, &params)
        .await
        .expect_err("wrongly typed field must raise");
    // derivation: string expected, int decoded -> wrapper b"arg" message.
    assert_invalid_argument(&error, "arg");
}

/// The validation-error list cap: `VALIDATION_ERROR_CAP = 64`
/// (types.rs) — appending the 65th error trips the kind-2 `bound` marker
/// (derivation: append_error's CellElemPtr cap check raises b"bound" when
/// count >= 64), while 64 appends in a loop SUCCEED with exactly 64 errors
/// (cap check passes when count < 64).
#[tokio::test]
async fn error_cap_growth_raises_kind_2_bound() {
    // A loop keeps the code small: a 64-deep nested expression exceeds the
    // 64 KiB code budget because every constructor call is emitted inline.
    let build_source = |depth: usize| {
        format!(
            "@entry validator v(record: {{ name: string }}?,              old_record: {{ name: string }}?) -> Validation {{
             let acc = accept();
             let i = 0;
             while (i < {depth}) {{ acc = with_error(acc, \"f\", \"c\"); i = i + 1; }}
             return acc;
}}
"
        )
    };

    // 64 deep: at the cap boundary, all appends pass.
    let at_cap = compiled(&build_source(64)).await;
    let result = call_once(&at_cap, &params_from(&[("record", QueryValue::Null)]))
        .await
        .expect("64-deep chain stays under the cap");
    let QueryValue::Map(ref decoded) = result else {
        panic!("validation result must decode to a map");
    };
    let Some(QueryValue::List(errors)) = decoded.get("errors") else {
        panic!("validation result must carry an `errors` list");
    };
    // derivation: 64 with_error appends produce exactly 64 errors, stop false.
    assert_eq!(errors.len(), 64);
    assert_eq!(decoded.get("stop"), Some(&QueryValue::Bool(false)));

    // 65 deep: the 65th append exceeds the cap and raises kind 2 "bound".
    let over_cap = compiled(&build_source(65)).await;
    let error = call_once(&over_cap, &params_from(&[("record", QueryValue::Null)]))
        .await
        .expect_err("65th append must raise the cap error");
    assert_invalid_argument(&error, "bound");
}

/// Two `with_error` values branching from one zero-error base keep their own
/// error regions (copy-on-append), so returning `left` shows only "left"
/// even though `right` was appended after it.
#[tokio::test]
async fn branching_from_one_base_keeps_values_independent() {
    let function = compiled(
        r#"@entry validator v(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  let base = accept();
  let left = with_error(base, "f", "left");
  let right = with_error(base, "f", "right");
  if (is_null(record)) {
    return left;
  }
  return right;
}
"#,
    )
    .await;
    let null_params = params_from(&[
        ("record", QueryValue::Null),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &null_params)
            .await
            .expect("null-record call succeeds"),
        // derivation: both appends copy the base cells into fresh regions,
        // so the later `right` append cannot leak into `left`.
        validation_result(vec![field_error("left", &["f"])], false)
    );
    let named_params = params_from(&[
        ("record", map_of(&[("name", str_value("Ada"))])),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &named_params)
            .await
            .expect("named-record call succeeds"),
        // derivation: `right` appends after `left` on its own fresh region
        // (pre-fix shared-region lowering would have shown "right" here).
        validation_result(vec![field_error("right", &["f"])], false)
    );
}

/// A helper taking and returning `Validation` compiles (entry-only type
/// nodes skip opaque helper parameters) and appends its code-only error
/// after the entry's field error.
#[tokio::test]
async fn validation_helper_composes_errors() {
    let function = compiled(
        r#"fn add(v: Validation) -> Validation {
  return with_record_error(v, "extra");
}
@entry validator v(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  return add(with_error(accept(), "f", "c1"));
}
"#,
    )
    .await;
    let params = params_from(&[
        ("record", QueryValue::Null),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &params)
            .await
            .expect("helper call succeeds"),
        // derivation: with_error appends the field error first, then the
        // helper's with_record_error appends the code-only error in order.
        validation_result(vec![field_error("c1", &["f"]), code_error("extra")], false)
    );
}

/// A conditional `?:` argument to a result constructor lowers through a
/// spill local, so the chosen string becomes the field path depending on
/// the record's nullness.
#[tokio::test]
async fn conditional_argument_selects_field_name() {
    let function = compiled(
        r#"@entry validator v(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  return with_error(accept(), is_null(record) ? "absent" : "present", "code");
}
"#,
    )
    .await;
    let null_params = params_from(&[
        ("record", QueryValue::Null),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &null_params)
            .await
            .expect("null-record call succeeds"),
        // derivation: is_null(record) selects the "absent" arm via the
        // spilled conditional argument.
        validation_result(vec![field_error("code", &["absent"])], false)
    );
    let named_params = params_from(&[
        ("record", map_of(&[("name", str_value("Ada"))])),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &named_params)
            .await
            .expect("named-record call succeeds"),
        // derivation: constructor reads both arguments in its own block
        // after the spill, so the "present" arm reaches the field path.
        validation_result(vec![field_error("code", &["present"])], false)
    );
}

/// An omitted optional declared field null-fills at projection (the
/// projection stores the null TAG), so `is_null(current.nick)` is true;
/// a present nick takes the reject branch.
#[tokio::test]
async fn omitted_optional_record_field_reads_null() {
    let function = compiled(
        r#"@entry validator v(
  record: { name: string, nick: string? }?,
  old_record: { name: string, nick: string? }?
) -> Validation {
  let current: { name: string, nick: string? } = unwrap(record);
  if (is_null(current.nick)) {
    return record_error("nick_absent");
  }
  return reject("nick", "nick_present");
}
"#,
    )
    .await;
    let absent = params_from(&[
        ("record", map_of(&[("name", str_value("Ada"))])),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &absent)
            .await
            .expect("omitted-nick call succeeds"),
        // derivation: projection null-fills the omitted optional field with
        // a NULL-tagged cell, so the is_null branch returns the code error.
        validation_result(vec![code_error("nick_absent")], false)
    );
    let present = params_from(&[
        (
            "record",
            map_of(&[("name", str_value("Ada")), ("nick", str_value("N"))]),
        ),
        ("old_record", QueryValue::Null),
    ]);
    assert_eq!(
        call_once(&function, &present)
            .await
            .expect("present-nick call succeeds"),
        // derivation: a present nick reads non-null, taking the reject
        // branch with the one-segment ["nick"] path.
        validation_result(vec![field_error("nick_present", &["nick"])], false)
    );
}
