//! Tests for `SelectExpr::to_filter_value` — the #1024 translation from the
//! narrow `SelectExpr` (computed SELECT expressions) AST into the equivalent
//! `FilterValue`/`FilterExpr` shape, so `SelectItem::Expression` reuses the
//! SAME `resolve_filter_query` evaluator `SelectItem::Function` already
//! uses instead of a bespoke evaluator.

use crate::filter::{FilterExpr, FilterExprOp, FilterValue};
use crate::read::{SelectExpr, SelectExprValue};

// ============================================================================
// Literal variants — all 5 `SelectExprValue` cases map 1:1 to `FilterValue`.
// ============================================================================

#[test]
fn literal_null_translates_to_filter_value_null() {
    let expr = SelectExpr::Literal {
        value: SelectExprValue::Null,
    };
    assert_eq!(expr.to_filter_value(), FilterValue::Null);
}

#[test]
fn literal_bool_translates_to_filter_value_bool() {
    let expr = SelectExpr::Literal {
        value: SelectExprValue::Bool(true),
    };
    assert_eq!(expr.to_filter_value(), FilterValue::Bool(true));
}

#[test]
fn literal_int_translates_to_filter_value_int() {
    let expr = SelectExpr::Literal {
        value: SelectExprValue::Int(42),
    };
    assert_eq!(expr.to_filter_value(), FilterValue::Int(42));
}

#[test]
fn literal_float_translates_to_filter_value_float() {
    let expr = SelectExpr::Literal {
        value: SelectExprValue::Float(3.5),
    };
    assert_eq!(expr.to_filter_value(), FilterValue::Float(3.5));
}

#[test]
fn literal_string_translates_to_filter_value_string() {
    let expr = SelectExpr::Literal {
        value: SelectExprValue::String("hi".to_string()),
    };
    assert_eq!(
        expr.to_filter_value(),
        FilterValue::String("hi".to_string())
    );
}

// ============================================================================
// Field reference — `SelectExpr::Field { path }` → `FilterValue::FieldRef`.
// ============================================================================

#[test]
fn field_translates_to_filter_value_field_ref_with_identical_path() {
    let expr = SelectExpr::Field {
        path: vec!["address".to_string(), "city".to_string()],
    };
    assert_eq!(
        expr.to_filter_value(),
        FilterValue::FieldRef {
            path: vec!["address".to_string(), "city".to_string()],
        }
    );
}

// ============================================================================
// Arithmetic variants — Add/Sub/Mul/Div each become a `FilterValue::Expr`
// wrapping a `FilterExpr` with the matching `FilterExprOp` and two args.
// ============================================================================

#[test]
fn add_translates_to_filter_expr_add() {
    let expr = SelectExpr::Add {
        left: Box::new(SelectExpr::Field {
            path: vec!["price".to_string()],
        }),
        right: Box::new(SelectExpr::Literal {
            value: SelectExprValue::Int(1),
        }),
    };
    assert_eq!(
        expr.to_filter_value(),
        FilterValue::Expr {
            expr: FilterExpr::new(
                FilterExprOp::Add,
                vec![
                    FilterValue::FieldRef {
                        path: vec!["price".to_string()],
                    },
                    FilterValue::Int(1),
                ],
            ),
        }
    );
}

#[test]
fn sub_translates_to_filter_expr_sub() {
    let expr = SelectExpr::Sub {
        left: Box::new(SelectExpr::Literal {
            value: SelectExprValue::Int(10),
        }),
        right: Box::new(SelectExpr::Literal {
            value: SelectExprValue::Int(3),
        }),
    };
    assert_eq!(
        expr.to_filter_value(),
        FilterValue::Expr {
            expr: FilterExpr::new(
                FilterExprOp::Sub,
                vec![FilterValue::Int(10), FilterValue::Int(3)],
            ),
        }
    );
}

#[test]
fn mul_translates_to_filter_expr_mul() {
    let expr = SelectExpr::Mul {
        left: Box::new(SelectExpr::Field {
            path: vec!["price".to_string()],
        }),
        right: Box::new(SelectExpr::Field {
            path: vec!["qty".to_string()],
        }),
    };
    assert_eq!(
        expr.to_filter_value(),
        FilterValue::Expr {
            expr: FilterExpr::new(
                FilterExprOp::Mul,
                vec![
                    FilterValue::FieldRef {
                        path: vec!["price".to_string()],
                    },
                    FilterValue::FieldRef {
                        path: vec!["qty".to_string()],
                    },
                ],
            ),
        }
    );
}

#[test]
fn div_translates_to_filter_expr_div() {
    let expr = SelectExpr::Div {
        left: Box::new(SelectExpr::Literal {
            value: SelectExprValue::Float(10.0),
        }),
        right: Box::new(SelectExpr::Literal {
            value: SelectExprValue::Float(4.0),
        }),
    };
    assert_eq!(
        expr.to_filter_value(),
        FilterValue::Expr {
            expr: FilterExpr::new(
                FilterExprOp::Div,
                vec![FilterValue::Float(10.0), FilterValue::Float(4.0)],
            ),
        }
    );
}

// ============================================================================
// Nested tree — Add{Mul{Field, Literal}, Field} — proves recursion produces
// the correct nested `FilterValue::Expr` shape, not just flat 1-level ops.
// ============================================================================

#[test]
fn nested_add_of_mul_and_field_translates_recursively() {
    // (price * qty) + tax
    let expr = SelectExpr::Add {
        left: Box::new(SelectExpr::Mul {
            left: Box::new(SelectExpr::Field {
                path: vec!["price".to_string()],
            }),
            right: Box::new(SelectExpr::Field {
                path: vec!["qty".to_string()],
            }),
        }),
        right: Box::new(SelectExpr::Field {
            path: vec!["tax".to_string()],
        }),
    };

    let expected = FilterValue::Expr {
        expr: FilterExpr::new(
            FilterExprOp::Add,
            vec![
                FilterValue::Expr {
                    expr: FilterExpr::new(
                        FilterExprOp::Mul,
                        vec![
                            FilterValue::FieldRef {
                                path: vec!["price".to_string()],
                            },
                            FilterValue::FieldRef {
                                path: vec!["qty".to_string()],
                            },
                        ],
                    ),
                },
                FilterValue::FieldRef {
                    path: vec!["tax".to_string()],
                },
            ],
        ),
    };

    assert_eq!(expr.to_filter_value(), expected);
}
