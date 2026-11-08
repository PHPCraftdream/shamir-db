use crate::api::CompileLimits;
use crate::front::{parse, BinaryOp, ExprId, ExprKind, StatementKind, UnaryOp};

fn returned(source_expression: &str) -> (crate::front::Module, ExprId) {
    let source = format!("@entry scalar f() -> i64 {{ return {source_expression}; }}");
    let module = parse(&source, &CompileLimits::default()).expect("expression parses");
    let id = module
        .statements
        .iter()
        .find_map(|statement| match statement.kind {
            StatementKind::Return(Some(id)) => Some(id),
            _ => None,
        })
        .expect("return expression is present");
    (module, id)
}

fn binary(module: &crate::front::Module, id: ExprId) -> (BinaryOp, ExprId, ExprId) {
    match module.expression(id).map(|node| &node.kind) {
        Some(ExprKind::Binary { op, left, right }) => (*op, *left, *right),
        other => panic!("expected binary expression, got {other:?}"),
    }
}

fn is_binary(module: &crate::front::Module, id: ExprId, expected: BinaryOp) -> bool {
    matches!(module.expression(id).map(|node| &node.kind), Some(ExprKind::Binary { op, .. }) if *op == expected)
}

#[test]
fn every_binary_precedence_row_and_associativity_is_structural() {
    let (module, root) = returned("a + b * c - d / e % f");
    let (op, left, right) = binary(&module, root);
    assert_eq!(op, BinaryOp::Subtract);
    assert!(is_binary(&module, left, BinaryOp::Add));
    assert!(is_binary(&module, right, BinaryOp::Remainder));
    let (add_op, _, add_right) = binary(&module, left);
    assert_eq!(add_op, BinaryOp::Add);
    assert!(is_binary(&module, add_right, BinaryOp::Multiply));
    let (rem_op, rem_left, rem_right) = binary(&module, right);
    assert_eq!(rem_op, BinaryOp::Remainder);
    assert!(is_binary(&module, rem_left, BinaryOp::Divide));
    assert!(
        matches!(module.expression(rem_right).map(|node| &node.kind), Some(ExprKind::Identifier(name)) if name == "f")
    );

    let (module, root) = returned("a + b < c + d == e && g || h");
    let (op, left, right) = binary(&module, root);
    assert_eq!(op, BinaryOp::Or);
    let (and_op, and_left, _) = binary(&module, left);
    assert_eq!(and_op, BinaryOp::And);
    assert!(is_binary(&module, and_left, BinaryOp::Equal));
    assert!(
        matches!(module.expression(right).map(|node| &node.kind), Some(ExprKind::Identifier(name)) if name == "h")
    );
    let (eq_op, comparison, _) = binary(&module, and_left);
    assert_eq!(eq_op, BinaryOp::Equal);
    assert!(is_binary(&module, comparison, BinaryOp::Less));

    let (module, root) = returned("a != b");
    assert_eq!(binary(&module, root).0, BinaryOp::NotEqual);
    let (module, root) = returned("a - b - c");
    let (op, left, _) = binary(&module, root);
    assert_eq!(op, BinaryOp::Subtract);
    assert!(is_binary(&module, left, BinaryOp::Subtract));

    for (operator, expected) in [
        ("<", BinaryOp::Less),
        ("<=", BinaryOp::LessEqual),
        (">", BinaryOp::Greater),
        (">=", BinaryOp::GreaterEqual),
    ] {
        let (module, root) = returned(&format!("a {operator} b"));
        assert_eq!(binary(&module, root).0, expected);
    }
}

#[test]
fn postfix_is_left_associative_and_unary_chains_are_right_nested() {
    let (module, root) = returned("factory(0,).field[0]");
    let field_target = match module.expression(root).map(|node| &node.kind) {
        Some(ExprKind::Index { target, .. }) => *target,
        other => panic!("expected final index, got {other:?}"),
    };
    let call_target = match module.expression(field_target).map(|node| &node.kind) {
        Some(ExprKind::Field { target, name }) => {
            assert_eq!(name, "field");
            *target
        }
        other => panic!("expected field, got {other:?}"),
    };
    let callee = match module.expression(call_target).map(|node| &node.kind) {
        Some(ExprKind::Call { target, arguments }) => {
            assert_eq!(arguments.len(), 1);
            *target
        }
        other => panic!("expected call, got {other:?}"),
    };
    assert!(
        matches!(module.expression(callee).map(|node| &node.kind), Some(ExprKind::Identifier(name)) if name == "factory")
    );

    let (module, mut root) = returned("!!-+value");
    for expected in [
        UnaryOp::Not,
        UnaryOp::Not,
        UnaryOp::Negate,
        UnaryOp::Positive,
    ] {
        match module.expression(root).map(|node| &node.kind) {
            Some(ExprKind::Unary { op, operand }) => {
                assert_eq!(*op, expected);
                root = *operand;
            }
            other => panic!("expected unary node, got {other:?}"),
        }
    }
    assert!(
        matches!(module.expression(root).map(|node| &node.kind), Some(ExprKind::Identifier(name)) if name == "value")
    );
}

#[test]
fn comparison_equality_order_and_right_associative_conditionals_are_exact() {
    let (module, root) = returned("a < b == c");
    let (op, left, right) = binary(&module, root);
    assert_eq!(op, BinaryOp::Equal);
    assert!(is_binary(&module, left, BinaryOp::Less));
    assert!(
        matches!(module.expression(right).map(|node| &node.kind), Some(ExprKind::Identifier(name)) if name == "c")
    );

    let (module, root) = returned("a ? b : c ? d : e");
    let (else_value, then_value) = match module.expression(root).map(|node| &node.kind) {
        Some(ExprKind::Conditional {
            then_value,
            else_value,
            ..
        }) => (*else_value, *then_value),
        other => panic!("expected conditional, got {other:?}"),
    };
    assert!(
        matches!(module.expression(then_value).map(|node| &node.kind), Some(ExprKind::Identifier(name)) if name == "b")
    );
    assert!(matches!(
        module.expression(else_value).map(|node| &node.kind),
        Some(ExprKind::Conditional { .. })
    ));

    let (module, root) = returned("a ? b ? c : d : e");
    let then_value = match module.expression(root).map(|node| &node.kind) {
        Some(ExprKind::Conditional { then_value, .. }) => *then_value,
        other => panic!("expected conditional, got {other:?}"),
    };
    assert!(matches!(
        module.expression(then_value).map(|node| &node.kind),
        Some(ExprKind::Conditional { .. })
    ));
}

#[test]
fn chained_comparisons_and_equalities_report_stable_offending_operator() {
    for (source, expected_column) in [
        ("@entry scalar f() -> bool { return a < b < c; }", 42),
        ("@entry scalar f() -> bool { return a == b != c; }", 43),
    ] {
        let error = parse(source, &CompileLimits::default()).expect_err("chains are forbidden");
        assert_eq!(error.code, "P003");
        assert_eq!(error.primary.start.column, expected_column);
        assert!(error
            .to_string()
            .starts_with(&format!("P003:1:{expected_column}:")));
    }
    let source = "@entry scalar f() -> null { return 1 }";
    let error = parse(source, &CompileLimits::default()).expect_err("semicolon required");
    assert_eq!(error.to_string(), "P001:1:38: expected ';', found '}'");
    let duplicate = "@entry scalar first() -> null {} @entry scalar second() -> null {}";
    let error = parse(duplicate, &CompileLimits::default()).expect_err("only one entry");
    assert_eq!(error.to_string(), "P002:1:34: module has more than one @entry declaration\n  related 1:1: first @entry declaration");
}
