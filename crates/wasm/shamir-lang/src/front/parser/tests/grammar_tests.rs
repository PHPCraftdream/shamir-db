use crate::api::CompileLimits;
use crate::front::{parse, EntryKind, ExprKind, ForKind, PrimitiveType, StatementKind, TypeKind};

const HELPERS: &str = r#"
fn inc(value: i64,) -> i64 { return value + 1; }
fn empty() -> null { return; }
@entry scalar run(input: i64,) -> i64 {
    const fixed: i64 = inc(input,);
    let inferred = fixed;
    let record: { name: string, nested: [{ count: i32? }], } = {
        name: "Ada",
        nested: [{ count: null, },],
    };
    let values: [i64]? = [1, 2,];
    record.name = "Grace";
    record.nested[0].count = 1;
    {
        let shadow: i32 = 1;
        shadow = shadow + 1;
    }
    if (true) { inferred = inferred + 1; }
    else if (false) { inferred = inferred - 1; }
    else { inferred = inferred * 2; }
    while (false) { continue; break; }
    for (i in 0..3) { if (false) { break; } }
    for (item in values) { item; }
    throw "failure";
    empty();
    return inferred;
}
"#;

#[test]
fn worked_examples_and_all_statement_forms_have_expected_ast_shapes() {
    let scalar = parse(
        "fn add(a: i64, b: i64) -> i64 { return a + b; }\n@entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }",
        &CompileLimits::default(),
    )
    .expect("scalar worked example");
    assert_eq!(scalar.functions.len(), 2);
    assert_eq!(
        scalar.functions.get(1).map(|f| f.entry_kind),
        Some(Some(EntryKind::Scalar))
    );
    assert_eq!(scalar.parameters.len(), 4);

    let validator = parse(
        r#"@entry validator require_name(
            record: { name: string }?,
            old_record: { name: string }?,
        ) -> Validation {
            if (is_null(record)) { return record_error("record_required"); }
            let current: { name: string } = unwrap(record);
            if (current.name == "") { return reject("name", "name_empty"); }
            return accept();
        }"#,
        &CompileLimits::default(),
    )
    .expect("typed validator worked example");
    assert_eq!(validator.functions.len(), 1);
    assert!(validator
        .types
        .iter()
        .any(|ty| matches!(ty.kind, TypeKind::Optional(_))));
    assert!(validator
        .types
        .iter()
        .any(|ty| matches!(ty.kind, TypeKind::Primitive(PrimitiveType::Validation))));

    let module = parse(HELPERS, &CompileLimits::default()).expect("multi-helper grammar fixture");
    assert_eq!(module.functions.len(), 3);
    assert_eq!(
        module
            .functions
            .iter()
            .filter(|f| f.entry_kind.is_none())
            .count(),
        2
    );
    assert_eq!(
        module.functions.get(2).map(|f| f.entry_kind),
        Some(Some(EntryKind::Scalar))
    );
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Let { mutable: false, .. })));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Let { mutable: true, .. })));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Assign { .. })));
    assert!(module.statements.iter().any(|s| matches!(
        s.kind,
        StatementKind::If {
            else_branch: Some(_),
            ..
        }
    )));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::While { .. })));
    assert!(module.statements.iter().any(|s| matches!(
        s.kind,
        StatementKind::For {
            kind: ForKind::Range,
            ..
        }
    )));
    assert!(module.statements.iter().any(|s| matches!(
        s.kind,
        StatementKind::For {
            kind: ForKind::List,
            ..
        }
    )));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Break)));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Continue)));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Return(None))));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Return(Some(_)))));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Throw(_))));
    assert!(module
        .statements
        .iter()
        .any(|s| matches!(s.kind, StatementKind::Expression(_))));
    assert!(module
        .types
        .iter()
        .any(|ty| matches!(ty.kind, TypeKind::Record(_))));
    assert!(module
        .expressions
        .iter()
        .any(|expr| matches!(expr.kind, ExprKind::Record(_))));
    assert!(module
        .expressions
        .iter()
        .any(|expr| matches!(expr.kind, ExprKind::List(_))));
}

#[test]
fn empty_lists_records_calls_and_parameter_lists_and_optional_types_parse() {
    let module = parse(
        "fn helper() -> null { return; }\n@entry scalar main() -> null? { let none: [i64]? = []; let empty: { }? = {}; helper(); return null; }",
        &CompileLimits::default(),
    )
    .expect("empty and optional forms");
    assert_eq!(module.functions.len(), 2);
    assert!(module
        .functions
        .iter()
        .all(|function| function.parameters.is_empty()));
    assert!(module
        .expressions
        .iter()
        .any(|expr| matches!(&expr.kind, ExprKind::List(items) if items.is_empty())));
    assert!(module
        .expressions
        .iter()
        .any(|expr| matches!(&expr.kind, ExprKind::Record(fields) if fields.is_empty())));
}

#[test]
fn parser_rejects_missing_entry_delimiters_and_invalid_assignment_targets() {
    let cases = [
        ("", "P002"),
        ("@entry scalar f() -> i64 { return 1 }", "P001"),
        (
            "@entry scalar f() -> i64 { if (true) {} else return 1; }",
            "P001",
        ),
        ("@entry scalar f() -> i64 { return (1 + 2; }", "P001"),
        (
            "@entry scalar f() -> i64 { (a + b) = 1; return 1; }",
            "P003",
        ),
        ("@entry scalar f() -> i64 { return f()(); }", "P003"),
        ("@entry scalar f() -> i64 { return (f)(1); }", "P003"),
        ("@entry scalar f() -> i64 { return r.f(1); }", "P003"),
        ("@entry scalar f() -> i64 { return a = b; }", "P001"),
        ("@entry scalar f() -> i64 { return 1;", "P001"),
        ("@entry scalar f() -> { return 1; }", "P001"),
        ("scalar f() -> null {}", "P002"),
    ];
    for (source, code) in cases {
        let error = parse(source, &CompileLimits::default()).expect_err("malformed form rejected");
        assert_eq!(error.code, code, "{source}");
    }
}

#[test]
fn signed_minimum_literals_are_accepted_only_as_direct_unary_minus_operands() {
    let valid = "@entry scalar f() -> i64 { return -2147483648i32; return -9223372036854775808i64; return -0x8000000000000000i64; }";
    assert!(parse(valid, &CompileLimits::default()).is_ok());

    for (source, code) in [
        (
            "@entry scalar f(a: i64) -> i64 { return a - 9223372036854775808; }",
            "L002",
        ),
        (
            "@entry scalar f() -> i64 { return +9223372036854775808; }",
            "L002",
        ),
        (
            "@entry scalar f() -> i64 { return -(9223372036854775808); }",
            "L002",
        ),
        (
            "@entry scalar f() -> i64 { return -9223372036854775808.field; }",
            "L002",
        ),
        (
            "@entry scalar f() -> i64 { return -9223372036854775808[0]; }",
            "L002",
        ),
    ] {
        let error = parse(source, &CompileLimits::default()).expect_err("invalid MIN use");
        assert_eq!(error.code, code, "{source}");
    }
}
