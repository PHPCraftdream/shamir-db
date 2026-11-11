use crate::api::{
    CompileError, CompileOptions, Diagnostic, Position, Span, Stage, MAX_DIAGNOSTIC_MESSAGE_BYTES,
};
use crate::compile;
use wasmparser::{Parser, Payload, TypeRef, ValType};

fn assert_call_diag_codes(source: &str, options: &CompileOptions, code: &str) {
    let error = compile(source, options).expect_err("source must produce the requested diagnostic");
    assert!(
        matches!(error, CompileError::Diagnostics(ref diagnostics) if diagnostics.iter().any(|d| d.code == code))
    );
}

fn assert_diag_codes(source: &str, expected: &[&str]) {
    let options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    let error = compile(source, &options).expect_err("invalid procedure source rejects");
    let actual: Vec<&str> = match error {
        CompileError::Diagnostics(diagnostics) => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect(),
        other => panic!("expected source diagnostics, got {other:?}; source: {source}"),
    };
    assert_eq!(actual, expected, "source: {source}");
}

fn update_source(alias: &str, table: &str, filter: &str, set: &str, params: &str) -> String {
    format!(
        "@entry procedure p({params}) -> i64 {{ batch_update(batch(), {alias}, {table}, {filter}, {set}); return 0; }}"
    )
}

fn delete_source(alias: &str, table: &str, filter: &str, params: &str) -> String {
    format!(
        "@entry procedure p({params}) -> i64 {{ batch_delete(batch(), {alias}, {table}, {filter}); return 0; }}"
    )
}

fn assert_predicate_rejected(filter: &str, params: &str, code: &str) {
    assert_diag_codes(
        &update_source("\"u\"", "\"users\"", filter, "{ name: \"x\" }", params),
        &[code],
    );
    assert_diag_codes(
        &delete_source("\"d\"", "\"users\"", filter, params),
        &[code],
    );
}

#[test]
fn mutation_rejects_missing_arguments_with_t004() {
    assert_diag_codes(
        "@entry procedure p() -> i64 { batch_update(batch(), \"u\", \"users\"); return 0; }",
        &["T004"],
    );
    assert_diag_codes(
        "@entry procedure p() -> i64 { batch_delete(batch(), \"d\", \"users\"); return 0; }",
        &["T004"],
    );
}

#[test]
fn mutation_rejects_extra_arguments_with_t004() {
    assert_diag_codes(
        "@entry procedure p() -> i64 { batch_update(batch(), \"u\", \"users\", { id: 1 }, { name: \"x\" }, 7); return 0; }",
        &["T004"],
    );
    assert_diag_codes(
        "@entry procedure p() -> i64 { batch_delete(batch(), \"d\", \"users\", { id: 1 }, 7); return 0; }",
        &["T004"],
    );
}

#[test]
fn mutation_rejects_empty_predicates_with_t001() {
    assert_predicate_rejected("{}", "", "T001");
}

#[test]
fn mutation_rejects_null_predicates_and_fields_with_t001() {
    assert_predicate_rejected("null", "", "T001");
    assert_predicate_rejected("{ id: null }", "", "T001");
}

#[test]
fn mutation_rejects_scalar_predicates_with_t001() {
    assert_predicate_rejected("key", "key: i64", "T001");
}

#[test]
fn mutation_rejects_nullable_predicate_values_with_t001() {
    assert_predicate_rejected("{ id: maybe }", "maybe: i64?", "T001");
}

#[test]
fn mutation_rejects_nullable_predicate_records_with_t001() {
    assert_predicate_rejected("filter", "filter: { id: i64 }?", "T001");
}

#[test]
fn mutation_rejects_nonliteral_tables_with_t001() {
    assert_diag_codes(
        &update_source(
            "\"u\"",
            "table",
            "{ id: 1 }",
            "{ name: \"x\" }",
            "table: string",
        ),
        &["T001"],
    );
    assert_diag_codes(
        &delete_source("\"d\"", "table", "{ id: 1 }", "table: string"),
        &["T001"],
    );
}

#[test]
fn mutation_rejects_nonliteral_table_types_with_t001() {
    assert_diag_codes(
        &update_source(
            "\"u\"",
            "table",
            "{ id: 1 }",
            "{ name: \"x\" }",
            "table: [string]",
        ),
        &["T001"],
    );
    assert_diag_codes(
        &delete_source("\"d\"", "table", "{ id: 1 }", "table: { name: string }"),
        &["T001"],
    );
}

#[test]
fn update_rejects_empty_sets_with_t001() {
    assert_diag_codes(
        &update_source("\"u\"", "\"users\"", "{ id: 1 }", "{}", ""),
        &["T001"],
    );
}

#[test]
fn mutation_rejects_expression_predicate_values_with_t001() {
    assert_predicate_rejected("{ id: key + 1 }", "key: i64", "T001");
}

#[test]
fn mutation_rejects_local_field_predicate_values_with_t001() {
    assert_predicate_rejected("{ id: row.id }", "row: { id: i64 }", "T001");
}

#[test]
fn mutation_rejects_duplicate_aliases_with_t010() {
    let source = r#"
@entry procedure p() -> i64 {
  let first = batch_update(batch(), "same", "users", { id: 1 }, { name: "x" });
  let second = batch_delete(first, "same", "users", { id: 1 });
  return 0;
}
"#;
    assert_diag_codes(source, &["T010"]);
}

#[test]
fn mutation_rejects_unknown_batch_helper_provenance_with_t010() {
    let source = r#"
fn change(b: Batch) -> Batch {
  return batch_update(b, "u", "users", { id: 1 }, { name: "x" });
}
@entry procedure p() -> i64 { return 0; }
"#;
    assert_diag_codes(source, &["T010"]);
}

#[test]
fn affected_projection_rejects_unknown_helper_provenance_with_t010() {
    let source = r#"
fn affected(r: Response) -> i64? { return response_affected(r, "u"); }
@entry procedure p() -> i64 { return 0; }
"#;
    assert_diag_codes(source, &["T010"]);
}

#[test]
fn affected_projection_rejects_wrong_context_with_t010() {
    let source = r#"
@entry procedure p() -> i64 {
  let b = batch_update(batch(), "u", "users", { id: 1 }, { name: "x" });
  let response = db_execute(request(b));
  let count: i64 = response_affected(response, "u");
  return count;
}
"#;
    assert_diag_codes(source, &["T010"]);
}

#[test]
fn mutations_accept_scalar_parameters_and_nonempty_typed_sets() {
    let source = r#"
@entry procedure p(key: i64) -> i64 {
  let b = batch_update(batch(), "u", "users", { id: key }, { name: "Ada", score: key });
  let d = batch_delete(b, "d", "users", { id: key });
  return 0;
}
"#;
    let options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    crate::compile(source, &options).expect("valid mutation procedure compiles");
}

#[test]
fn mutation_rejects_nonliteral_aliases_with_t001() {
    assert_diag_codes(
        &update_source(
            "alias",
            "\"users\"",
            "{ id: 1 }",
            "{ name: \"x\" }",
            "alias: string",
        ),
        &["T001"],
    );
    assert_diag_codes(
        &delete_source("alias", "\"users\"", "{ id: 1 }", "alias: string"),
        &["T001"],
    );
}

#[test]
fn mutation_predicates_reject_local_scalars_and_record_variables_with_t001() {
    let local_scalar = r#"
@entry procedure p() -> i64 {
  let key = 1;
  let b = batch_update(batch(), "u", "users", { id: key }, { name: "x" });
  return 0;
}
"#;
    assert_diag_codes(local_scalar, &["T001"]);
    assert_predicate_rejected("row", "row: { id: i64 }", "T001");
}

#[test]
fn mutation_predicates_reject_nested_and_list_fields_with_t001() {
    assert_predicate_rejected("{ profile: { id: 1 } }", "", "T001");
    assert_predicate_rejected("{ ids: ids }", "ids: [i64]", "T001");
}

#[test]
fn mutation_predicates_with_unresolved_reference_calls_report_n001() {
    assert_diag_codes(
        "@entry procedure p() -> i64 { let b = batch_delete(batch(), \"d\", \"users\", { id: qref(\"users\", \"[0].id\") }); return 0; }",
        &["T001", "N001"],
    );
}

#[test]
fn mutation_predicates_reject_function_call_values_with_t001() {
    let source = r#"
fn key() -> i64 { return 1; }
@entry procedure p() -> i64 {
  let b = batch_update(batch(), "u", "users", { id: key() }, { name: "x" });
  return 0;
}
"#;
    assert_diag_codes(source, &["T001"]);
}

#[test]
fn update_rejects_nonrecord_and_null_sets_with_t001() {
    assert_diag_codes(
        &update_source("\"u\"", "\"users\"", "{ id: 1 }", "7", ""),
        &["T001"],
    );
    assert_diag_codes(
        &update_source("\"u\"", "\"users\"", "{ id: 1 }", "null", ""),
        &["T001"],
    );
}

#[test]
fn affected_projection_rejects_read_and_insert_aliases_with_t001() {
    let read = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_read(batch(), "r", "users", { id: 1 })));
  let count = response_affected(response, "r");
  return 0;
}
"#;
    assert_diag_codes(read, &["T001"]);

    let insert = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_insert(batch(), "i", "users", { id: 1 })));
  let count = response_affected(response, "i");
  return 0;
}
"#;
    assert_diag_codes(insert, &["T001"]);
}

#[test]
fn row_projections_reject_mutation_aliases_with_t001() {
    let rows = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  let result: [{ id: i64 } ]? = response_rows(response, "u");
  return 0;
}
"#;
    assert_diag_codes(rows, &["T001"]);

    let inserted = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_delete(batch(), "d", "users", { id: 1 })));
  let result: [{ id: i64 } ]? = response_inserted(response, "d");
  return 0;
}
"#;
    assert_diag_codes(inserted, &["T001"]);
}

#[test]
fn affected_projection_after_response_reassignment_uses_the_new_read_plan() {
    let source = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  response = db_execute(request(batch_read(batch(), "u", "users", { id: 1 })));
  let count = response_affected(response, "u");
  return 0;
}
"#;
    assert_diag_codes(source, &["T001"]);
}

#[test]
fn affected_projection_after_control_flow_invalidation_reports_t010() {
    let branch = r#"
@entry procedure p(flag: bool) -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  if (flag) { let marker = 1; }
  let count = response_affected(response, "u");
  return 0;
}
"#;
    assert_diag_codes(branch, &["T010"]);

    let looped = r#"
@entry procedure p(flag: bool) -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  while (flag) { let marker = 1; }
  let count = response_affected(response, "u");
  return 0;
}
"#;
    assert_diag_codes(looped, &["T010"]);
}

#[test]
fn affected_projection_does_not_leak_a_then_fact_without_else() {
    let source = r#"
@entry procedure p(flag: bool) -> i64 {
  let response = db_execute(request(batch_read(batch(), "u", "users", { id: 1 })));
  if (flag) {
    response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  }
  let count = response_affected(response, "u");
  return 0;
}
"#;
    assert_diag_codes(source, &["T010"]);
}

#[test]
fn affected_projection_accepts_an_inferred_result_for_an_absent_alias() {
    let source = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  let count = response_affected(response, "absent");
  return 0;
}
"#;
    let options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    crate::compile(source, &options).expect("absent alias has an inferred optional count");
}

#[test]
fn stage_two_projection_effects_reach_scalar_and_validator_entries() {
    let scalar = r#"
fn affected() -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  let count = response_affected(response, "u");
  if (is_null(count)) { return 0; }
  return unwrap(count);
}
@entry scalar p() -> i64 { return affected(); }
"#;
    assert_diag_codes(scalar, &["T007"]);

    let validator = r#"
fn affected() -> i64 {
  let response = db_execute(request(batch_update(batch(), "u", "users", { id: 1 }, { name: "x" })));
  let count = response_affected(response, "u");
  if (is_null(count)) { return 0; }
  return unwrap(count);
}
@entry validator p(record: { name: string }?, old_record: { name: string }?) -> Validation {
  let count = affected();
  return accept();
}
"#;
    assert_diag_codes(validator, &["T007"]);
}

#[test]
fn diagnostic_message_and_related_message_are_bounded_and_utf8_safe() {
    let span = Span::new(
        2,
        4,
        Position { line: 3, column: 5 },
        Position { line: 3, column: 7 },
    );
    let long = "界".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES);
    let diagnostic = Diagnostic::new("L002", long.clone(), span).with_related(span, long);
    assert!(diagnostic.message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
    assert!(diagnostic
        .message
        .is_char_boundary(diagnostic.message.len()));
    let related = diagnostic.related.as_ref().expect("related source");
    assert!(related.message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
    assert!(related.message.is_char_boundary(related.message.len()));
}

#[test]
fn checked_call_emits_the_exact_import_and_is_deterministic() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options.callable_catalog.insert(
        "increment".to_string(),
        b"procedure(value:i64)->i64".to_vec(),
    );
    let source = "@entry procedure caller(value: i64) -> i64 { return call(\"increment\", { value: value }); }";
    let first = compile(source, &options).expect("catalogued call compiles");
    let again = compile(source, &options).expect("same catalogued call compiles");
    assert_eq!(first.wasm, again.wasm);
    assert_eq!(first.report.import_count, 2);
    let mut types = Vec::new();
    for payload in Parser::new(0).parse_all(&first.wasm) {
        if let Payload::TypeSection(reader) = payload.expect("valid wasm") {
            types.extend(
                reader
                    .into_iter_err_on_gc_types()
                    .map(|ty| ty.expect("type")),
            );
        }
    }
    let mut checked = 0;
    for payload in Parser::new(0).parse_all(&first.wasm) {
        if let Payload::ImportSection(reader) = payload.expect("valid wasm") {
            for import in reader.into_imports() {
                let import = import.expect("valid import");
                if import.module == "shamir_host" && import.name == "call_checked" {
                    let (TypeRef::Func(index) | TypeRef::FuncExact(index)) = import.ty else {
                        panic!("call_checked must be a function import");
                    };
                    let ty = &types[index as usize];
                    assert_eq!(ty.params(), &[ValType::I32; 6]);
                    assert_eq!(ty.results(), &[ValType::I64]);
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 1);
}

#[test]
fn checked_call_catalog_descriptor_and_nullable_nil_are_exact() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options.callable_catalog.insert(
        "transform".to_string(),
        b"procedure(input:{items:[{label:string?}],meta:{count:i32,enabled:bool}},maybe:i64?)->{result:[bytes?]}?".to_vec(),
    );
    let source = "@entry procedure caller(maybe: i64?, input: { items: [{ label: string? }], meta: { enabled: bool, count: i32 } }) -> { result: [bytes?] }? { return call(\"transform\", { maybe: null, input: { items: [{ label: null }], meta: { enabled: true, count: 1 } } }); }";
    let compiled = compile(source, &options).expect("exact nested signature compiles");
    assert_eq!(compiled.report.import_count, 2);
    assert_eq!(compiled.signature.as_deref(), Some(b"procedure(maybe:i64?,input:{items:[{label:string?}],meta:{count:i32,enabled:bool}})->{result:[bytes?]}?".as_slice()));
    let signature = b"procedure(input:{items:[{label:string?}],meta:{count:i32,enabled:bool}},maybe:i64?)->{result:[bytes?]}?";
    assert!(compiled
        .wasm
        .windows(signature.len())
        .any(|bytes| bytes == signature));
}

#[test]
fn checked_call_accepts_empty_map_and_infers_result() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options
        .callable_catalog
        .insert("empty".to_string(), b"procedure()->i64".to_vec());
    let compiled = compile(
        "@entry procedure caller() -> i64 { let result = call(\"empty\", {}); return result; }",
        &options,
    )
    .expect("empty map call infers its result");
    assert_eq!(compiled.report.import_count, 2);
}

#[test]
fn checked_call_grouping_preserves_exact_record_argument_types() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options
        .callable_catalog
        .insert("remote".into(), b"procedure(value:i64?)->i64".to_vec());

    for source in [
        "@entry procedure caller(value: i64) -> i64 { return call(\"remote\", { value: value }); }",
        "@entry procedure caller(value: i64) -> i64 { return call(\"remote\", ({ value: value })); }",
        "@entry procedure caller(value: i64) -> i64 { return call(\"remote\", (({ value: value }))); }",
    ] {
        assert_call_diag_codes(source, &options, "T004");
    }

    for source in [
        "@entry procedure caller(value: i64?) -> i64 { return call(\"remote\", ({ value: value })); }",
        "@entry procedure caller() -> i64 { return call(\"remote\", (({ value: null }))); }",
    ] {
        compile(source, &options).expect("exact nullable grouped argument compiles");
    }
}

#[test]
fn checked_call_rejects_contextual_optional_composite_literals() {
    for (signature, values) in [
        (
            b"procedure(p:{a:i64}?)->i64".as_slice(),
            [
                "{ p: { a: 1 } }",
                "{ p: ({ a: 1 }) }",
                "({ p: { a: 1 } })",
                "({ p: ({ a: 1 }) })",
            ],
        ),
        (
            b"procedure(p:{nested:{a:i64}?})->i64".as_slice(),
            [
                "{ p: { nested: { a: 1 } } }",
                "{ p: { nested: ({ a: 1 }) } }",
                "({ p: { nested: { a: 1 } } })",
                "({ p: { nested: ({ a: 1 }) } })",
            ],
        ),
        (
            b"procedure(p:[i64]?)->i64".as_slice(),
            [
                "{ p: [1] }",
                "{ p: ([1]) }",
                "({ p: [1] })",
                "({ p: ([1]) })",
            ],
        ),
    ] {
        let options = CompileOptions {
            stage: Stage::Procedures,
            callable_catalog: [("remote".to_string(), signature.to_vec())]
                .into_iter()
                .collect(),
            ..CompileOptions::default()
        };
        for arguments in values {
            let source = format!(
                "@entry procedure caller() -> i64 {{ return call(\"remote\", {arguments}); }}"
            );
            assert_call_diag_codes(&source, &options, "T004");
        }
    }
}

#[test]
fn checked_call_accepts_exact_optional_composite_values_and_null() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options.callable_catalog.insert(
        "remote".into(),
        b"procedure(p:{a:i64}?,items:[i64]?)->i64".to_vec(),
    );
    for source in [
        "@entry procedure caller(p: { a: i64 }?, items: [i64]?) -> i64 { return call(\"remote\", { p: p, items: items }); }",
        "@entry procedure caller() -> i64 { return call(\"remote\", { p: null, items: null }); }",
    ] {
        compile(source, &options).expect("exact optional value or null compiles");
    }
}

#[test]
fn checked_call_rejects_invalid_signatures_and_argument_contracts() {
    let mut catalog = std::collections::BTreeMap::new();
    catalog.insert("remote".to_string(), b"procedure(value:i64)->i64".to_vec());
    for source in [
        "@entry procedure caller() -> i64 { return call(\"missing\", { value: 1 }); }",
        "@entry procedure caller(target: string) -> i64 { return call(target, { value: 1 }); }",
        "@entry procedure caller() -> i64 { return call(\"remote\", { wrong: 1 }); }",
        "@entry procedure caller() -> i64 { return call(\"remote\", { value: 1, extra: 2 }); }",
        "@entry procedure caller(value: i32) -> i64 { return call(\"remote\", { value: value }); }",
        "@entry procedure caller() -> i64 { return call(\"remote\", {}); }",
    ] {
        let options = CompileOptions {
            stage: Stage::Procedures,
            callable_catalog: catalog.clone(),
            ..CompileOptions::default()
        };
        assert_call_diag_codes(source, &options, "T004");
    }
    let mut nullable = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    nullable
        .callable_catalog
        .insert("remote".into(), b"procedure(value:i64?)->i64".to_vec());
    assert_call_diag_codes(
        "@entry procedure caller() -> i64 { return call(\"remote\", {}); }",
        &nullable,
        "T004",
    );
    let mut malformed = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    malformed
        .callable_catalog
        .insert("bad".into(), b"procedure( value:i64)->i64".to_vec());
    assert_call_diag_codes(
        "@entry procedure caller(value: i64) -> i64 { return call(\"bad\", { value: value }); }",
        &malformed,
        "T004",
    );
    let mut result_mismatch = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    result_mismatch
        .callable_catalog
        .insert("remote".into(), b"procedure()->i64?".to_vec());
    assert_call_diag_codes(
        "@entry procedure caller() -> i64 { return call(\"remote\", {}); }",
        &result_mismatch,
        "T004",
    );
}

#[test]
fn checked_call_capability_rejects_nonprocedure_entries_and_helpers() {
    let mut options = CompileOptions::default();
    options
        .callable_catalog
        .insert("remote".to_string(), b"procedure()->i64".to_vec());
    for source in [
        "@entry scalar caller() -> i64 { return call(\"remote\", {}); }",
        "@entry validator caller(record: { name: string }?, old_record: { name: string }?) -> Validation { call(\"remote\", {}); return accept(); }",
        "fn call_remote() -> i64 { return call(\"remote\", {}); }\n@entry scalar caller() -> i64 { return call_remote(); }",
    ] {
        assert_call_diag_codes(source, &options, "T007");
    }
}

#[test]
fn checked_call_reports_cycle_and_opaque_boundaries() {
    let cycle = "fn loop(value: i64) -> i64 { return loop(value); }\n@entry procedure caller(value: i64) -> i64 { return loop(value); }";
    assert_diag_codes(cycle, &["T008"]);
    for (signature, source) in [
        (
            b"procedure(value:Batch)->i64".as_slice(),
            "@entry procedure caller() -> i64 { return call(\"remote\", { value: batch() }); }",
        ),
        (
            b"procedure()->Response".as_slice(),
            "@entry procedure caller() -> i64 { return call(\"remote\", {}); }",
        ),
    ] {
        let mut options = CompileOptions {
            stage: Stage::Procedures,
            ..CompileOptions::default()
        };
        options
            .callable_catalog
            .insert("remote".into(), signature.to_vec());
        assert_call_diag_codes(source, &options, "T009");
    }
    let mut opaque_result = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    opaque_result
        .callable_catalog
        .insert("remote".into(), b"procedure()->Response".to_vec());
    assert_call_diag_codes(
        "@entry procedure caller() -> i64 { return call(\"remote\", {}); }",
        &opaque_result,
        "T009",
    );
    let mut opaque_null = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    opaque_null
        .callable_catalog
        .insert("bad".into(), b"procedure(value:null?)->i64".to_vec());
    assert_call_diag_codes(
        "@entry procedure caller() -> i64 { return call(\"bad\", { value: null }); }",
        &opaque_null,
        "T009",
    );
}

#[test]
fn checked_call_import_ceiling_is_six() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options
        .callable_catalog
        .insert("remote".to_string(), b"procedure()->i64".to_vec());
    let source = r#"@entry procedure caller() -> i64 {
      let user: { id: i64 }? = db_get("users", 1);
      let rows: [{ id: i64 }] = db_query("users", null);
      let stored: { id: i64 } = db_insert("users", { id: 1 });
      let request_value = request(batch_read(batch(), "r", "users", null));
      let response = db_execute(request_value);
      let result: [{ id: i64 }]? = response_rows(response, "r");
      if (is_null(user) || is_null(result)) { return call("remote", {}); }
      return stored.id + to_i64(len(rows));
    }"#;
    let compiled = compile(source, &options).expect("imports compile");
    assert_eq!(compiled.report.import_count, 6);
}

#[test]
fn diagnostic_rendering_is_deterministic_and_orders_related_location_after_primary() {
    let primary = Span::new(
        0,
        2,
        Position { line: 2, column: 4 },
        Position { line: 2, column: 6 },
    );
    let secondary = Span::new(
        8,
        9,
        Position { line: 5, column: 1 },
        Position { line: 5, column: 2 },
    );
    let diagnostic =
        Diagnostic::new("P002", "duplicate entry", primary).with_related(secondary, "first entry");
    let rendered = "P002:2:4: duplicate entry\n  related 5:1: first entry";
    assert_eq!(diagnostic.to_string(), rendered);
    assert_eq!(diagnostic.to_string(), rendered);
}

fn checked_call_diagnostics(source: &str, options: &CompileOptions) -> (Vec<Diagnostic>, String) {
    let error = compile(source, options).expect_err("source must produce diagnostics");
    let diagnostics = match &error {
        CompileError::Diagnostics(diagnostics) => diagnostics.clone(),
        other => panic!("expected source diagnostics, got {other:?}; source: {source}"),
    };
    (diagnostics, error.to_string())
}

#[test]
fn checked_call_missing_catalog_targets_have_uniform_diagnostics() {
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options.callable_catalog.insert(
        "unrelated_visible".to_string(),
        b"procedure(unrelated_arg:string)->{unrelated_result:i64}".to_vec(),
    );
    let hidden_signature = b"procedure(private_arg:string)->{private_result:i64}";
    assert!(!options.callable_catalog.contains_key("hidden"));
    assert!(!options
        .callable_catalog
        .values()
        .any(|signature| signature.as_slice() == hidden_signature));

    let targets = ["absent", "ghosts", "hidden"];
    assert_eq!(targets[0].len(), targets[1].len());
    assert_eq!(targets[1].len(), targets[2].len());
    let outputs: Vec<_> = targets
        .into_iter()
        .map(|target| {
            let source = format!(
                "@entry procedure caller() -> i64 {{ return call(\"{target}\", {{ probe: 1 }}); }}"
            );
            checked_call_diagnostics(&source, &options)
        })
        .collect();

    for (diagnostics, rendered) in &outputs {
        assert_eq!(diagnostics[0].code, "T004");
        assert_eq!(diagnostics[0].message, "callable procedure is unavailable");
        assert!(!rendered.contains("private_arg"));
        assert!(!rendered.contains("private_result"));
        assert!(!rendered.contains("unrelated_arg"));
        assert!(!rendered.contains("unrelated_result"));
    }
    for output in outputs.iter().skip(1) {
        assert_eq!(outputs[0].0, output.0);
        assert_eq!(outputs[0].1.as_bytes(), output.1.as_bytes());
    }
}

#[test]
fn checked_call_visible_catalog_keeps_precise_signature_mismatches() {
    let names = CompileOptions {
        stage: Stage::Procedures,
        callable_catalog: [(
            "visible".to_string(),
            b"procedure(expected:i64)->i64".to_vec(),
        )]
        .into_iter()
        .collect(),
        ..CompileOptions::default()
    };
    let (name_diagnostics, _) = checked_call_diagnostics(
        "@entry procedure caller() -> i64 { return call(\"visible\", { actual: 1 }); }",
        &names,
    );
    let name_messages: Vec<&str> = name_diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(name_messages.contains(&"unknown record field actual"));
    assert!(name_messages.contains(&"missing record field expected"));
    assert!(name_messages.contains(&"call arguments must name exactly the declared parameters"));

    let types = CompileOptions {
        stage: Stage::Procedures,
        callable_catalog: [("visible".to_string(), b"procedure(value:i64)->i64".to_vec())]
            .into_iter()
            .collect(),
        ..CompileOptions::default()
    };
    let (type_diagnostics, _) = checked_call_diagnostics(
        "@entry procedure caller(value: string) -> i64 { return call(\"visible\", { value: value }); }",
        &types,
    );
    let type_messages: Vec<&str> = type_diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(type_messages.contains(&"expected i64, found string (field value)"));
    assert!(
        type_messages.contains(&"call arguments must exactly match the declared parameter types")
    );

    let result = CompileOptions {
        stage: Stage::Procedures,
        callable_catalog: [("visible".to_string(), b"procedure()->i64?".to_vec())]
            .into_iter()
            .collect(),
        ..CompileOptions::default()
    };
    let (result_diagnostics, _) = checked_call_diagnostics(
        "@entry procedure caller() -> i64 { return call(\"visible\", {}); }",
        &result,
    );
    assert!(result_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message == "call result is i64?, not i64"));
}

#[test]
fn checked_call_compiles_when_target_signature_is_added_to_catalog() {
    let target = compile(
        "@entry procedure target(value: i64) -> i64 { return value; }",
        &CompileOptions {
            stage: Stage::Procedures,
            ..CompileOptions::default()
        },
    )
    .expect("target procedure compiles");
    let mut options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    options.callable_catalog.insert(
        "target".to_string(),
        target.signature.expect("procedure emits its signature"),
    );

    compile(
        "@entry procedure caller(value: i64) -> i64 { return call(\"target\", { value: value }); }",
        &options,
    )
    .expect("caller compiles once target signature is catalogued");
}
