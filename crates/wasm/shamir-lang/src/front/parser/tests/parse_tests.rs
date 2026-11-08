use crate::api::CompileLimits;
use crate::front::{parse, BinaryOp, ExprKind, StatementKind};

const SCALAR: &str = "fn add(a: i64, b: i64) -> i64 { return a + b; }\n@entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }";
const VALIDATOR: &str = "@entry validator require_name(record: { name: string }?, old_record: { name: string }?) -> Validation { if (is_null(record)) { return record_error(\"record_required\"); } let current: { name: string } = unwrap(record); if (current.name == \"\") { return reject(\"name\", \"name_empty\"); } return accept(); }";

#[test]
fn worked_examples_and_precedence_build_flat_nodes() {
    let scalar = parse(SCALAR, &CompileLimits::default()).expect("scalar example parses");
    assert_eq!(scalar.functions.len(), 2);
    assert_eq!(
        scalar
            .functions
            .get(1)
            .and_then(|function| function.entry_kind),
        Some(crate::front::EntryKind::Scalar)
    );
    let addition = scalar.expressions.iter().find(|node| {
        matches!(
            node.kind,
            ExprKind::Binary {
                op: BinaryOp::Add,
                ..
            }
        )
    });
    assert!(addition.is_some());
    let validator = parse(VALIDATOR, &CompileLimits::default()).expect("validator example parses");
    assert_eq!(validator.functions.len(), 1);
    assert!(validator
        .types
        .iter()
        .any(|node| matches!(node.kind, crate::front::TypeKind::Optional(_))));

    let precedence = parse(
        "@entry scalar f() -> i64 { return 1 + 2 * 3; }",
        &CompileLimits::default(),
    )
    .expect("precedence parses");
    let root = precedence
        .statements
        .iter()
        .find_map(|statement| match &statement.kind {
            StatementKind::Return(Some(id)) => precedence.expression(*id),
            _ => None,
        });
    assert!(
        matches!(root.map(|node| &node.kind), Some(ExprKind::Binary { op: BinaryOp::Add, right, .. }) if precedence.expression(*right).is_some_and(|child| matches!(child.kind, ExprKind::Binary { op: BinaryOp::Multiply, .. })))
    );
}

#[test]
fn syntax_codes_grouping_and_entry_locations_are_stable() {
    for (source, code) in [
        ("@entry scalar f() -> i64 { return 1 < 2 < 3; }", "P003"),
        ("@entry scalar f() -> i64 { return 1 == 2 == 3; }", "P003"),
        ("@entry scalar f() -> i64 { return (x)(1); }", "P003"),
        ("@entry scalar f() -> i64 { return x()(); }", "P003"),
        (
            "@entry scalar f() -> i64 { return 9223372036854775808; }",
            "L002",
        ),
        ("scalar f() -> i64 {}", "P002"),
        (
            "@entry scalar f() -> i64 {} @entry procedure g() -> null {}",
            "P002",
        ),
        ("fn helper() -> null {}", "P002"),
    ] {
        let error = parse(source, &CompileLimits::default()).expect_err("invalid grammar rejected");
        assert_eq!(error.code, code);
        assert!(error.primary.end_byte >= error.primary.start_byte);
    }
    let duplicate = parse(
        "@entry scalar f() -> null {} @entry scalar g() -> null {}",
        &CompileLimits::default(),
    )
    .expect_err("duplicate entry rejected");
    assert!(duplicate.related.is_some());
    assert!(duplicate.to_string().starts_with("P002:1:"));
    assert!(parse(
        "@entry scalar f() -> i64 { return -9223372036854775808; }",
        &CompileLimits::default()
    )
    .is_ok());
}

#[test]
fn limits_and_malformed_prefixes_return_diagnostics_without_panicking() {
    let limits = CompileLimits {
        max_functions: 1,
        ..CompileLimits::default()
    };
    let error = parse(SCALAR, &limits).expect_err("function ceiling enforced");
    assert_eq!(error.code, "B001");

    for example in [SCALAR, VALIDATOR] {
        for end in example
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(example.len()))
        {
            let prefix = example.get(..end).unwrap_or("");
            let result = std::panic::catch_unwind(|| parse(prefix, &CompileLimits::default()));
            assert!(result.is_ok());
        }
        for (index, character) in example.char_indices() {
            let end = index.saturating_add(character.len_utf8());
            let deletion = format!(
                "{}{}",
                example.get(..index).unwrap_or(""),
                example.get(end..).unwrap_or("")
            );
            let result = std::panic::catch_unwind(|| parse(&deletion, &CompileLimits::default()));
            assert!(result.is_ok());
        }
    }
}

#[test]
fn record_literal_expression_statements_parse_and_blocks_stay_blocks() {
    let record = parse(
        "@entry scalar f() -> null {\n    { a: 1 };\n}",
        &CompileLimits::default(),
    )
    .expect("a record literal is an expression statement");
    assert!(record
        .statements
        .iter()
        .any(|statement| matches!(statement.kind, StatementKind::Expression(_))));
    let block = parse(
        "@entry scalar f() -> null {\n    { let x = 1; }\n    {}\n}",
        &CompileLimits::default(),
    )
    .expect("nested blocks still parse as blocks");
    assert_eq!(
        block
            .statements
            .iter()
            .filter(|statement| matches!(statement.kind, StatementKind::Block(_)))
            .count(),
        3
    );
}

#[test]
fn record_type_depth_counts_its_field_nodes() {
    let source = "@entry scalar f() -> { a: i64 } {}";
    let limited = |max_ast_depth| CompileLimits {
        max_ast_depth,
        ..CompileLimits::default()
    };
    assert_eq!(
        parse(source, &limited(3)).err().map(|error| error.code),
        Some("B001")
    );
    assert!(parse(source, &limited(4)).is_ok());
}
