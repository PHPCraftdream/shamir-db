use crate::api::{
    CompileError, CompileOptions, Diagnostic, Position, Span, Stage, MAX_DIAGNOSTIC_MESSAGE_BYTES,
};

fn assert_diag_codes(source: &str, expected: &[&str]) {
    let options = CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    };
    let error = crate::compile(source, &options).expect_err("invalid procedure source rejects");
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
