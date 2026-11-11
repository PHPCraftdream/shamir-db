use crate::api::{CompileLimits, Stage};
use crate::sem::tests::helper::check_with_stage;
use crate::sem::{ReferenceKind, ReferenceMarker, TypedModule};

fn check(source: &str) -> Result<TypedModule, Vec<crate::api::Diagnostic>> {
    check_with_stage(source, &CompileLimits::default(), Stage::Procedures)
}

fn clean(source: &str) -> TypedModule {
    check(source).unwrap_or_else(|diagnostics| {
        panic!(
            "expected clean semantic check, got: {:?}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        )
    })
}

fn diagnostic(source: &str, code: &str, message: &str) -> crate::api::Diagnostic {
    let diagnostics = check(source).expect_err("source must be rejected");
    diagnostics
        .into_iter()
        .find(|diag| diag.code == code && diag.message.contains(message))
        .unwrap_or_else(|| panic!("missing {code} diagnostic containing {message:?}"))
}

fn assert_span_contains(source: &str, diagnostic: &crate::api::Diagnostic, text: &str) {
    let start = diagnostic.primary.start_byte as usize;
    let end = diagnostic.primary.end_byte as usize;
    assert!(
        source
            .get(start..end)
            .is_some_and(|span| span.contains(text)),
        "diagnostic span did not cover {text:?}: {diagnostic}"
    );
}

fn assert_span_equals(source: &str, diagnostic: &crate::api::Diagnostic, expected: &str) {
    let start = diagnostic.primary.start_byte as usize;
    let end = diagnostic.primary.end_byte as usize;
    assert_eq!(
        source.get(start..end),
        Some(expected),
        "diagnostic span did not match {expected:?}: {diagnostic}"
    );
}

fn markers(module: &TypedModule) -> Vec<ReferenceMarker> {
    let mut markers: Vec<_> = module.reference_markers.iter().flatten().cloned().collect();
    markers.sort_by(|left, right| (&left.alias, &left.path).cmp(&(&right.alias, &right.path)));
    markers
}

fn marker(alias: &str, path: &str, kind: ReferenceKind) -> ReferenceMarker {
    ReferenceMarker {
        alias: alias.to_string(),
        path: path.to_string(),
        kind,
        transactional: false,
    }
}

#[test]
fn scalar_reference_flows_from_read_to_read_equality() {
    let source = r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch(), "src", "users", { id: 1 });
    const key: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const b1 = batch_read(b0, "dependent", "users", key);
    return 0;
}
"#;
    let module = clean(source);
    assert_eq!(
        markers(&module),
        vec![marker("@src", "[0].id", ReferenceKind::I64)]
    );
    assert_eq!(
        module.reference_sinks.iter().map(Vec::len).sum::<usize>(),
        1
    );
}

#[test]
fn scalar_reference_flows_from_read_to_insert() {
    let source = r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch(), "src", "users", { id: 1 });
    const value: { name: string } = { name: query_ref_field(b0, "src", 0, "name") };
    const b1 = batch_insert(b0, "created", "users", value);
    return 0;
}
"#;
    let module = clean(source);
    assert_eq!(
        markers(&module),
        vec![marker("@src", "[0].name", ReferenceKind::String)]
    );
    assert_eq!(
        module.reference_sinks.iter().map(Vec::len).sum::<usize>(),
        1
    );
}

#[test]
fn all_six_non_null_scalar_reference_kinds_are_recorded() {
    let source = r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch(), "src", "users", {
        bytes_value: b"raw",
        bool_value: true,
        f64_value: 1.5,
        i32_value: 1,
        i64_value: 2,
        string_value: "value"
    });
    const values: {
        bytes_value: bytes,
        bool_value: bool,
        f64_value: f64,
        i32_value: i32,
        i64_value: i64,
        string_value: string
    } = {
        bytes_value: query_ref_field(b0, "src", 0, "bytes_value"),
        bool_value: query_ref_field(b0, "src", 0, "bool_value"),
        f64_value: query_ref_field(b0, "src", 0, "f64_value"),
        i32_value: query_ref_field(b0, "src", 0, "i32_value"),
        i64_value: query_ref_field(b0, "src", 0, "i64_value"),
        string_value: query_ref_field(b0, "src", 0, "string_value")
    };
    const b1 = batch_insert(b0, "created", "users", values);
    return 0;
}
"#;
    let module = clean(source);
    assert_eq!(
        markers(&module),
        vec![
            marker("@src", "[0].bool_value", ReferenceKind::Bool),
            marker("@src", "[0].bytes_value", ReferenceKind::Bytes),
            marker("@src", "[0].f64_value", ReferenceKind::F64),
            marker("@src", "[0].i32_value", ReferenceKind::I32),
            marker("@src", "[0].i64_value", ReferenceKind::I64),
            marker("@src", "[0].string_value", ReferenceKind::String),
        ]
    );
    assert_eq!(
        module.reference_sinks.iter().map(Vec::len).sum::<usize>(),
        6
    );
}

#[test]
fn mixed_literal_and_reference_fields_and_two_read_producers_are_preserved() {
    let source = r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch(), "first", "users", { id: 1 });
    const b1 = batch_read(b0, "second", "users", { id: 2 });
    const values: { first_id: i64, second_name: string, label: string } = {
        first_id: query_ref_field(b1, "first", 0, "id"),
        second_name: query_ref_field(b1, "second", 1, "name"),
        label: "literal"
    };
    const b2 = batch_insert(b1, "created", "users", values);
    return 0;
}
"#;
    let module = clean(source);
    assert_eq!(
        markers(&module),
        vec![
            marker("@first", "[0].id", ReferenceKind::I64),
            marker("@second", "[1].name", ReferenceKind::String),
        ]
    );
    assert_eq!(
        module.reference_sinks.iter().map(Vec::len).sum::<usize>(),
        2
    );
}

#[test]
fn query_ref_field_requires_four_arguments() {
    let source = r#"@entry procedure p() -> i64 {
    const b0 = batch();
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0) };
    return 0;
}"#;
    let diag = diagnostic(source, "T004", "query_ref_field expects 4 arguments");
    assert_span_contains(source, &diag, "query_ref_field(b0, \"src\", 0)");
}

#[test]
fn query_ref_field_checks_argument_types() {
    for (source, message, span) in [
        (
            "@entry procedure p() -> i64 { const value: { id: i64 } = { id: query_ref_field(\"not_batch\", \"src\", 0, \"id\") }; return 0; }",
            "argument must be batch",
            "\"not_batch\"",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch(); const value: { id: i64 } = { id: query_ref_field(b0, 1, 0, \"id\") }; return 0; }",
            "argument must be string",
            "1",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch(); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", \"zero\", \"id\") }; return 0; }",
            "index must be an integer",
            "\"zero\"",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch(); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, 1) }; return 0; }",
            "argument must be string",
            "1",
        ),
    ] {
        let diag = diagnostic(source, "T004", message);
        assert_span_contains(source, &diag, span);
    }
}

#[test]
fn scalar_and_validator_entries_cannot_use_scalar_references() {
    for source in [
        "@entry scalar p() -> i64 { const value: { id: i64 } = { id: query_ref_field(batch(), \"src\", 0, \"id\") }; return 0; }",
        "@entry validator p(record: { id: i64 }?, old_record: { id: i64 }?) -> Validation { const value: { id: i64 } = { id: query_ref_field(batch(), \"src\", 0, \"id\") }; return accept(); }",
    ] {
        let diag = diagnostic(source, "T007", "query_ref_field requires a procedure entry");
        assert_span_contains(source, &diag, "query_ref_field");
    }
}

#[test]
fn scalar_reference_producer_alias_and_field_must_be_literal_strings() {
    let alias_source = "@entry procedure p(alias: string) -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, alias, 0, \"id\") }; return 0; }";
    let diag = diagnostic(
        alias_source,
        "T009",
        "query_ref_field producer alias must be a string literal",
    );
    assert_eq!(
        diag.message,
        "query_ref_field producer alias must be a string literal"
    );
    assert_span_equals(alias_source, &diag, "alias");
    assert_eq!(
        diag.primary.start_byte as usize,
        alias_source
            .rfind("alias")
            .expect("call argument is present")
    );

    let field_source = "@entry procedure p(field: string) -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, field) }; return 0; }";
    let diag = diagnostic(
        field_source,
        "T009",
        "query_ref_field field must be a string literal",
    );
    assert_eq!(
        diag.message,
        "query_ref_field field must be a string literal"
    );
    assert_span_equals(field_source, &diag, "field");
    assert_eq!(
        diag.primary.start_byte as usize,
        field_source
            .rfind("field")
            .expect("call argument is present")
    );
}

#[test]
fn scalar_reference_index_must_be_nonnegative_literal() {
    let negative = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", -1, \"id\") }; return 0; }";
    let diag = diagnostic(negative, "T009", "index cannot be negative");
    assert_span_contains(negative, &diag, "-1");

    let dynamic = "@entry procedure p(index: i64) -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", index, \"id\") }; return 0; }";
    let diag = diagnostic(
        dynamic,
        "T009",
        "index must be a non-negative integer literal",
    );
    assert_span_contains(dynamic, &diag, "index");
}

#[test]
fn scalar_reference_producer_alias_must_match_bounded_ascii_identifier() {
    let long_alias = "a".repeat(65);
    for alias in [
        "@src",
        "src.name",
        "src[0]",
        "",
        "1leading",
        "naïve",
        long_alias.as_str(),
    ] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"src\", \"users\", {{ id: 1 }}); const value: {{ id: i64 }} = {{ id: query_ref_field(b0, \"{alias}\", 0, \"id\") }}; return 0; }}"
        );
        let diag = diagnostic(
            &source,
            "T009",
            "query_ref_field producer alias must match [A-Za-z_][A-Za-z0-9_]{0,63}",
        );
        assert_eq!(
            diag.message,
            "query_ref_field producer alias must match [A-Za-z_][A-Za-z0-9_]{0,63}"
        );
        let alias_literal = format!("\"{alias}\"");
        assert_span_equals(&source, &diag, &alias_literal);
    }
}

#[test]
fn underscore_and_maximum_length_ascii_reference_producer_aliases_are_valid() {
    let long_alias = "a".repeat(64);
    for alias in ["_", long_alias.as_str()] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"{alias}\", \"users\", {{ id: 1 }}); const value: {{ item: i64 }} = {{ item: query_ref_field(b0, \"{alias}\", 0, \"id\") }}; const b1 = batch_insert(b0, \"created\", \"users\", value); return 0; }}"
        );
        let module = clean(&source);
        assert_eq!(
            markers(&module),
            vec![marker(&format!("@{alias}"), "[0].id", ReferenceKind::I64)]
        );
    }
}

#[test]
fn scalar_reference_producer_aliases_must_be_unique_after_normalization() {
    for (source, message) in [
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const b1 = batch_read(b0, \"@src\", \"users\", { id: 2 }); const value: { id: i64 } = { id: query_ref_field(b1, \"src\", 0, \"id\") }; return 0; }",
            "query_ref_field requires unique batch aliases",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const b1 = batch_insert(b0, \"@src\", \"users\", { id: 2 }); const value: { id: i64 } = { id: query_ref_field(b1, \"src\", 0, \"id\") }; return 0; }",
            "query_ref_field requires unique batch aliases",
        ),
    ] {
        let diag = diagnostic(source, "T010", message);
        assert_eq!(diag.message, message);
        assert_span_equals(source, &diag, "query_ref_field(b1, \"src\", 0, \"id\")");
    }
}

#[test]
fn scalar_reference_sink_plan_rejects_aliases_that_collide_after_normalization() {
    let sink_collision = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(b0, \"@src\", \"users\", value); return 0; }";
    let diag = diagnostic(
        sink_collision,
        "T010",
        "reference sink alias must be unique in the batch",
    );
    assert_eq!(
        diag.message,
        "reference sink alias must be unique in the batch"
    );
    assert_span_contains(
        sink_collision,
        &diag,
        "batch_read(b0, \"@src\", \"users\", value)",
    );

    let collision_after_snapshot = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(b0, \"@src\", \"users\", { id: 2 }); const b2 = batch_read(b1, \"dst\", \"users\", value); return 0; }";
    let diag = diagnostic(
        collision_after_snapshot,
        "T010",
        "reference sink requires a known batch plan",
    );
    assert_eq!(diag.message, "reference sink requires a known batch plan");
    assert_span_contains(
        collision_after_snapshot,
        &diag,
        "batch_read(b1, \"dst\", \"users\", value)",
    );
}

#[test]
fn scalar_reference_field_name_must_match_the_bounded_identifier_pattern() {
    let long_field = "a".repeat(65);
    for field in [
        "",
        "bad-name",
        "bad.name",
        "bad[name]",
        "1leading",
        "naïve",
        long_field.as_str(),
    ] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"src\", \"users\", {{ id: 1 }}); const value: {{ id: i64 }} = {{ id: query_ref_field(b0, \"src\", 0, \"{field}\") }}; return 0; }}"
        );
        let diag = diagnostic(
            &source,
            "T009",
            "query_ref_field field must match [A-Za-z_][A-Za-z0-9_]{0,63}",
        );
        assert_eq!(
            diag.message,
            "query_ref_field field must match [A-Za-z_][A-Za-z0-9_]{0,63}"
        );
        let field_literal = format!("\"{field}\"");
        assert_span_equals(&source, &diag, &field_literal);
    }
}

#[test]
fn underscore_and_maximum_length_ascii_reference_fields_are_valid() {
    let long_field = "a".repeat(64);
    for field in ["_", long_field.as_str()] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"src\", \"users\", {{ id: 1 }}); const value: {{ item: i64 }} = {{ item: query_ref_field(b0, \"src\", 0, \"{field}\") }}; const b1 = batch_insert(b0, \"created\", \"users\", value); return 0; }}"
        );
        let module = clean(&source);
        assert_eq!(
            markers(&module),
            vec![marker("@src", &format!("[0].{field}"), ReferenceKind::I64)]
        );
        let mut sinks: Vec<_> = module.reference_sinks.iter().flatten().cloned().collect();
        sinks.sort_by(|left, right| (&left.alias, &left.path).cmp(&(&right.alias, &right.path)));
        assert_eq!(
            sinks,
            vec![marker("@src", &format!("[0].{field}"), ReferenceKind::I64)]
        );
    }
}

#[test]
fn scalar_reference_sink_field_must_be_a_non_null_scalar() {
    for field_type in ["i64?", "{ nested: i64 }", "[i64]"] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"src\", \"users\", {{ id: 1 }}); const value: {{ item: {field_type} }} = {{ item: query_ref_field(b0, \"src\", 0, \"id\") }}; return 0; }}"
        );
        let diag = diagnostic(&source, "T009", "requires a non-null scalar field type");
        assert_eq!(
            diag.message,
            "query_ref_field requires a non-null scalar field type"
        );
        assert_span_equals(&source, &diag, "query_ref_field(b0, \"src\", 0, \"id\")");
    }

    let null_field = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { item: null } = { item: query_ref_field(b0, \"src\", 0, \"id\") }; return 0; }";
    let diag = diagnostic(null_field, "T009", "null is not permitted here");
    assert_eq!(diag.message, "null is not permitted here");
    assert_span_equals(null_field, &diag, "null");
}

#[test]
fn scalar_reference_requires_a_const_annotated_direct_record_field() {
    for binding in [
        "let value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") };",
        "const value = { id: query_ref_field(b0, \"src\", 0, \"id\") };",
    ] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"src\", \"users\", {{ id: 1 }}); {binding} return 0; }}"
        );
        let diag = diagnostic(
            &source,
            "T010",
            "only allowed in a const annotated record local",
        );
        assert_span_contains(&source, &diag, "query_ref_field");
    }

    let direct = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const b1 = batch_read(b0, \"next\", \"users\", { id: query_ref_field(b0, \"src\", 0, \"id\") }); return 0; }";
    let diag = diagnostic(
        direct,
        "T010",
        "only allowed in a const annotated record local",
    );
    assert_span_contains(direct, &diag, "query_ref_field");
}

#[test]
fn scalar_reference_producer_must_be_a_unique_prior_read() {
    let cases = [
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"other\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"missing\", 0, \"id\") }; return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"missing\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_insert(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"src\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const b1 = batch_insert(b0, \"src\", \"users\", { id: 2 }); const value: { id: i64 } = { id: query_ref_field(b1, \"src\", 0, \"id\") }; return 0; }",
            "query_ref_field requires unique batch aliases",
            "query_ref_field(b1, \"src\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch(); const value: { id: i64 } = { id: query_ref_field(b0, \"self\", 0, \"id\") }; const b1 = batch_read(b0, \"self\", \"users\", value); return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"self\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"next\", 0, \"id\") }; const b1 = batch_read(b0, \"next\", \"users\", value); return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"next\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch(); const value: { id: i64 } = { id: query_ref_field(b0, \"later\", 0, \"id\") }; const b1 = batch_read(b0, \"later\", \"users\", { id: 1 }); return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"later\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_delete(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"src\", 0, \"id\")",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_update(batch(), \"src\", \"users\", { id: 1 }, { name: \"x\" }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; return 0; }",
            "query_ref_field alias must identify exactly one read",
            "query_ref_field(b0, \"src\", 0, \"id\")",
        ),
    ];
    for (source, message, span) in cases {
        let diag = diagnostic(source, "T010", message);
        assert_eq!(diag.message, message);
        assert_span_equals(source, &diag, span);
    }
}

#[test]
fn scalar_reference_cannot_use_its_producer_alias_for_its_sink() {
    let source = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(b0, \"src\", \"users\", value); return 0; }";
    let diag = diagnostic(
        source,
        "T010",
        "reference sink alias must be unique in the batch",
    );
    assert_eq!(
        diag.message,
        "reference sink alias must be unique in the batch"
    );
    assert_span_equals(source, &diag, "batch_read(b0, \"src\", \"users\", value)");
}

#[test]
fn scalar_reference_sink_must_share_its_root_and_extend_its_snapshot() {
    let other_root = "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(batch(), \"dst\", \"users\", value); return 0; }";
    let diag = diagnostic(
        other_root,
        "T010",
        "share the root and extend the reference snapshot",
    );
    assert_span_contains(other_root, &diag, "value");

    let not_prefix = "@entry procedure p() -> i64 { const b0 = batch(); const b1 = batch_read(b0, \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b1, \"src\", 0, \"id\") }; const b2 = batch_read(b0, \"dst\", \"users\", value); return 0; }";
    let diag = diagnostic(
        not_prefix,
        "T010",
        "share the root and extend the reference snapshot",
    );
    assert_span_contains(not_prefix, &diag, "value");
}

#[test]
fn scalar_reference_sink_alias_must_be_new_and_literal() {
    for source in [
        "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(b0, \"src\", \"users\", value); return 0; }",
        "@entry procedure p(alias: string) -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(b0, alias, \"users\", value); return 0; }",
    ] {
        let diag = diagnostic(source, "T010", "reference sink alias");
        assert_span_contains(source, &diag, "batch_read");
    }
}

#[test]
fn scalar_reference_requires_a_statically_known_straight_line_plan() {
    for control in [
        "if (true) { const ignored = 1; }",
        "while (false) { const ignored = 1; }",
    ] {
        let source = format!(
            "@entry procedure p() -> i64 {{ const b0 = batch_read(batch(), \"src\", \"users\", {{ id: 1 }}); {control} const value: {{ id: i64 }} = {{ id: query_ref_field(b0, \"src\", 0, \"id\") }}; return 0; }}"
        );
        let diag = diagnostic(&source, "T010", "requires a known batch plan");
        assert_span_contains(&source, &diag, "query_ref_field");
    }
}

#[test]
fn scalar_reference_local_cannot_escape_or_be_consumed_twice() {
    let cases = [
        (
            "fn helper(value: { id: i64 }) -> i64 { return 0; } @entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; return helper(value); }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> { id: i64 } { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; return value; }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const inserted = db_insert(\"users\", value); return 0; }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_update(b0, \"changed\", \"users\", { id: 1 }, value); return 0; }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_delete(b0, \"removed\", \"users\", value); return 0; }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const values: [{ id: i64 }] = [value]; return 0; }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const id = value.id; return 0; }",
            "may only be used once",
            "value",
        ),
        (
            "@entry procedure p() -> i64 { const b0 = batch_read(batch(), \"src\", \"users\", { id: 1 }); const value: { id: i64 } = { id: query_ref_field(b0, \"src\", 0, \"id\") }; const b1 = batch_read(b0, \"first\", \"users\", value); const b2 = batch_insert(b1, \"second\", \"users\", value); return 0; }",
            "may only be used once",
            "value",
        ),
    ];
    for (source, message, span) in cases {
        let diag = diagnostic(source, "T010", message);
        assert_span_contains(source, &diag, span);
    }
}
