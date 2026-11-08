use crate::api::Span;
use crate::front::{BinaryOp, ExprId, ExprKind, IntegerSuffix, UnaryOp};
use crate::sem::tables::{Callee, ResolvedTarget};
use crate::sem::types::{TypeData, TypeId};

use super::builtins;
use super::entry::{
    error_type, expr_span, intern_list, intern_optional, primitive_type, report, Checker,
};
use super::statement::FunctionFrame;

/// Checks one expression against an optional expected type.
pub(super) fn check_expression(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    expected: Option<TypeId>,
    id: ExprId,
    depth: u32,
) -> TypeId {
    let span = expr_span(c, id);
    let kind = match c.module.expression(id) {
        Some(expr) => expr.kind.clone(),
        None => return error_type(c),
    };
    if depth > c.limits.max_ast_depth as u32 {
        report(
            c,
            "B001",
            "expression nesting exceeds the limit".to_string(),
            span,
        );
        return error_type(c);
    }
    let next = depth.saturating_add(1);
    let result = match kind {
        ExprKind::Identifier(_) => check_identifier(c, frame, id, span),
        ExprKind::Integer {
            magnitude,
            suffix,
            negative_min_candidate,
            ..
        } => integer_literal_type(c, magnitude, suffix, negative_min_candidate, expected),
        ExprKind::Float(_) => primitive_type(c, "f64"),
        ExprKind::String(_) => primitive_type(c, "string"),
        ExprKind::Bytes(_) => primitive_type(c, "bytes"),
        ExprKind::Boolean(_) => primitive_type(c, "bool"),
        ExprKind::Null => primitive_type(c, "null"),
        ExprKind::Group(inner) => check_expression(c, frame, expected, inner, next),
        ExprKind::List(items) => check_list(c, frame, expected, span, &items, next),
        ExprKind::Record(fields) => check_record(c, frame, expected, span, &fields, next),
        ExprKind::Unary { op, operand } => check_unary(c, frame, op, operand, expected, next),
        ExprKind::Binary { op, left, right } => {
            check_binary(c, frame, op, left, right, expected, next)
        }
        ExprKind::Conditional {
            condition,
            then_value,
            else_value,
        } => check_conditional(c, frame, condition, then_value, else_value, expected, next),
        // The callee name is not a value: it keeps no type entry.
        ExprKind::Call { arguments, .. } => check_call(c, frame, id, span, &arguments, depth),
        ExprKind::Field { target, name } => {
            let target_span = expr_span(c, target);
            let t = check_expression(c, frame, None, target, next);
            if c.types.is_error(t) {
                error_type(c)
            } else if let Some(TypeData::Record(index)) = c.types.data(t) {
                match c.types.record(index).and_then(|shape| shape.field(&name)) {
                    Some(field_ty) => field_ty,
                    None => {
                        report(c, "N001", format!("unknown record field {name}"), span);
                        error_type(c)
                    }
                }
            } else {
                report(
                    c,
                    "T003",
                    "field access requires a record".to_string(),
                    target_span,
                );
                error_type(c)
            }
        }
        ExprKind::Index { target, index } => {
            let target_span = expr_span(c, target);
            let index_span = expr_span(c, index);
            let t = check_expression(c, frame, None, target, next);
            let i32_ty = primitive_type(c, "i32");
            let it = check_expression(c, frame, Some(i32_ty), index, next);
            if !c.types.is_error(it) && it != i32_ty {
                report(c, "T001", "list index must be i32".to_string(), index_span);
                return error_type(c);
            }
            if c.types.is_error(t) {
                error_type(c)
            } else if let Some(TypeData::List(elem)) = c.types.data(t) {
                elem
            } else {
                report(
                    c,
                    "T003",
                    "list indexing requires a list".to_string(),
                    target_span,
                );
                error_type(c)
            }
        }
    };
    if let Some(slot) = c.expr_types.get_mut(id.0 as usize) {
        *slot = Some(result);
    }
    result
}

/// Types a variable reference through its resolved slot.
fn check_identifier(c: &mut Checker, frame: &mut FunctionFrame, id: ExprId, span: Span) -> TypeId {
    let resolved = c.resolved.get(id.0 as usize).copied().flatten();
    match resolved {
        Some(ResolvedTarget::Parameter(slot)) | Some(ResolvedTarget::Local(slot)) => frame
            .local_types
            .get(slot as usize)
            .copied()
            .unwrap_or_else(|| error_type(c)),
        Some(ResolvedTarget::Function(_)) | Some(ResolvedTarget::Builtin(_)) => {
            report(
                c,
                "T004",
                "function and builtin names are not values".to_string(),
                span,
            );
            error_type(c)
        }
        Some(ResolvedTarget::Error) | None => error_type(c),
    }
}

/// Selects the integer literal type via suffix and expected type.
fn integer_literal_type(
    c: &mut Checker,
    magnitude: u64,
    suffix: Option<IntegerSuffix>,
    negated: bool,
    expected: Option<TypeId>,
) -> TypeId {
    match suffix {
        Some(IntegerSuffix::I32) => primitive_type(c, "i32"),
        Some(IntegerSuffix::I64) => primitive_type(c, "i64"),
        None => {
            let fits_i32 = magnitude <= 2_147_483_647 || (negated && magnitude == 2_147_483_648);
            let wants_i32 = expected
                .map(|exp| match c.types.data(exp) {
                    Some(TypeData::I32) => true,
                    Some(TypeData::Optional(inner)) => c.types.data(inner) == Some(TypeData::I32),
                    _ => false,
                })
                .unwrap_or(false);
            if wants_i32 && fits_i32 {
                primitive_type(c, "i32")
            } else {
                primitive_type(c, "i64")
            }
        }
    }
}

/// Checks a list literal with a uniform element type.
fn check_list(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    expected: Option<TypeId>,
    span: Span,
    items: &[ExprId],
    depth: u32,
) -> TypeId {
    let expected_elem = match expected {
        Some(exp) => match c.types.data(exp) {
            Some(TypeData::Error) => {
                for &item in items {
                    check_expression(c, frame, None, item, depth);
                }
                return error_type(c);
            }
            Some(TypeData::Optional(inner)) => match c.types.data(inner) {
                Some(TypeData::List(elem)) => Some(elem),
                _ => None,
            },
            Some(TypeData::List(elem)) => Some(elem),
            _ => None,
        },
        None => None,
    };
    let mut first_ty = None;
    for (position, &item) in items.iter().enumerate() {
        let expected_item = expected_elem.or(first_ty);
        let actual = check_expression(c, frame, expected_item, item, depth);
        if position == 0 {
            first_ty = Some(actual);
        }
        if let Some(expected) = expected_elem.or(first_ty) {
            if !c.types.is_error(expected)
                && !c.types.is_error(actual)
                && !assignable(c, expected, actual)
            {
                report_mismatch(c, expected, actual, item, expr_span(c, item), "element");
            }
        }
    }
    match expected_elem {
        Some(elem) => intern_list(c, elem),
        None => match first_ty {
            Some(elem) => intern_list(c, elem),
            None => {
                report(
                    c,
                    "T001",
                    "empty list literal requires an expected list type".to_string(),
                    span,
                );
                error_type(c)
            }
        },
    }
}

/// Checks a record literal against an expected record shape.
fn check_record(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    expected: Option<TypeId>,
    span: Span,
    fields: &[crate::front::ValueFieldId],
    depth: u32,
) -> TypeId {
    let expected_data = expected.map(|exp| c.types.data(exp));
    let shape_index = match expected_data {
        Some(Some(TypeData::Error)) => {
            for &field_id in fields {
                if let Some(field) = c.module.value_field(field_id) {
                    let value = field.value;
                    check_expression(c, frame, None, value, depth);
                }
            }
            return error_type(c);
        }
        Some(Some(TypeData::Record(index))) => Some(index),
        Some(Some(TypeData::Optional(inner))) => match c.types.data(inner) {
            Some(TypeData::Record(index)) => Some(index),
            _ => None,
        },
        _ => None,
    };
    let Some(shape_index) = shape_index else {
        report(
            c,
            "T001",
            "record literal requires an expected record type".to_string(),
            span,
        );
        for &field_id in fields {
            if let Some(field) = c.module.value_field(field_id) {
                let value = field.value;
                check_expression(c, frame, None, value, depth);
            }
        }
        return error_type(c);
    };
    let field_names: Vec<String> = match c.types.record(shape_index) {
        Some(shape) => shape.fields.iter().map(|(name, _)| name.clone()).collect(),
        None => Vec::new(),
    };
    let mut seen: Vec<String> = Vec::new();
    for &field_id in fields {
        let Some(field) = c.module.value_field(field_id) else {
            continue;
        };
        let value_span = expr_span(c, field.value);
        if seen.contains(&field.name) {
            report(
                c,
                "T001",
                format!("duplicate record field {}", field.name),
                field.span,
            );
            check_expression(c, frame, None, field.value, depth);
            continue;
        }
        seen.push(field.name.clone());
        let field_ty = c
            .types
            .record(shape_index)
            .and_then(|shape| shape.field(&field.name));
        match field_ty {
            Some(field_ty) => {
                let actual = check_expression(c, frame, Some(field_ty), field.value, depth);
                if !c.types.is_error(field_ty)
                    && !c.types.is_error(actual)
                    && !assignable(c, field_ty, actual)
                {
                    report_mismatch(c, field_ty, actual, field.value, value_span, "field value");
                }
            }
            None => {
                report(
                    c,
                    "T001",
                    format!("unknown record field {}", field.name),
                    field.span,
                );
                check_expression(c, frame, None, field.value, depth);
            }
        }
    }
    let missing: Vec<String> = field_names
        .iter()
        .filter(|name| !seen.iter().any(|seen_name| seen_name == *name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        report(
            c,
            "T001",
            format!("missing record field {}", missing.join(", ")),
            span,
        );
    }
    expected.unwrap_or_else(|| error_type(c))
}

/// Checks unary operators.
fn check_unary(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    op: UnaryOp,
    operand: ExprId,
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let operand_span = expr_span(c, operand);
    match op {
        UnaryOp::Not => {
            let bool_ty = primitive_type(c, "bool");
            let ty = check_expression(c, frame, Some(bool_ty), operand, depth);
            if !c.types.is_error(ty) && ty != bool_ty {
                report(
                    c,
                    "T003",
                    "operator ! requires bool".to_string(),
                    operand_span,
                );
                return error_type(c);
            }
            bool_ty
        }
        UnaryOp::Positive => {
            let ty = check_expression(c, frame, None, operand, depth);
            if !c.types.is_error(ty) && !c.types.is_numeric(ty) {
                report(
                    c,
                    "T003",
                    "unary + requires a numeric operand".to_string(),
                    operand_span,
                );
                return error_type(c);
            }
            ty
        }
        UnaryOp::Negate => {
            let literal = match c.module.expression(operand) {
                Some(expr) => match expr.kind {
                    ExprKind::Integer {
                        magnitude,
                        suffix,
                        negative_min_candidate: _,
                        ..
                    } => Some((magnitude, suffix)),
                    ExprKind::Identifier(_)
                    | ExprKind::Float(_)
                    | ExprKind::String(_)
                    | ExprKind::Bytes(_)
                    | ExprKind::Boolean(_)
                    | ExprKind::Null
                    | ExprKind::Group(_)
                    | ExprKind::List(_)
                    | ExprKind::Record(_)
                    | ExprKind::Unary { .. }
                    | ExprKind::Binary { .. }
                    | ExprKind::Conditional { .. }
                    | ExprKind::Call { .. }
                    | ExprKind::Field { .. }
                    | ExprKind::Index { .. } => None,
                },
                None => None,
            };
            if let Some((magnitude, suffix)) = literal {
                let ty = integer_literal_type(c, magnitude, suffix, true, expected);
                if let Some(slot) = c.expr_types.get_mut(operand.0 as usize) {
                    *slot = Some(ty);
                }
                return ty;
            }
            let ty = check_expression(c, frame, None, operand, depth);
            if c.types.is_error(ty) || c.types.is_numeric(ty) {
                ty
            } else {
                report(
                    c,
                    "T003",
                    "unary - requires a numeric operand".to_string(),
                    operand_span,
                );
                error_type(c)
            }
        }
    }
}

/// Checks binary operators by family.
fn check_binary(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    op: BinaryOp,
    left: ExprId,
    right: ExprId,
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    match op {
        BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Remainder
        | BinaryOp::Add
        | BinaryOp::Subtract => check_arithmetic(c, frame, op, left, right, expected, depth),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
            check_ordering(c, frame, left, right, depth)
        }
        BinaryOp::Equal | BinaryOp::NotEqual => check_equality(c, frame, left, right, depth),
        BinaryOp::And | BinaryOp::Or => {
            let bool_ty = primitive_type(c, "bool");
            let left_span = expr_span(c, left);
            let right_span = expr_span(c, right);
            let lt = check_expression(c, frame, Some(bool_ty), left, depth);
            let rt = check_expression(c, frame, Some(bool_ty), right, depth);
            if !c.types.is_error(lt) && lt != bool_ty {
                report(
                    c,
                    "T003",
                    "logical operators require bool operands".to_string(),
                    left_span,
                );
            }
            if !c.types.is_error(rt) && rt != bool_ty {
                report(
                    c,
                    "T003",
                    "logical operators require bool operands".to_string(),
                    right_span,
                );
            }
            if c.types.is_error(lt) || c.types.is_error(rt) {
                return error_type(c);
            }
            bool_ty
        }
    }
}

/// The source spelling of one arithmetic operator.
fn operator_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn check_arithmetic(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    op: BinaryOp,
    left: ExprId,
    right: ExprId,
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let left_span = expr_span(c, left);
    let right_span = expr_span(c, right);
    let lt = check_expression(c, frame, expected, left, depth);
    let rt = check_expression(c, frame, Some(lt), right, depth);
    if c.types.is_error(lt) || c.types.is_error(rt) {
        return lt;
    }
    if !c.types.is_numeric(lt) {
        report(
            c,
            "T003",
            format!("operator {} requires numeric operands", operator_name(op)),
            left_span,
        );
        return error_type(c);
    }
    if rt != lt {
        report(
            c,
            "T003",
            "operators require identical operand types".to_string(),
            right_span,
        );
        return error_type(c);
    }
    if op == BinaryOp::Remainder && c.types.data(lt) == Some(TypeData::F64) {
        report(
            c,
            "T003",
            "remainder requires integer operands".to_string(),
            right_span,
        );
        return error_type(c);
    }
    lt
}

fn check_ordering(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    left: ExprId,
    right: ExprId,
    depth: u32,
) -> TypeId {
    let right_span = expr_span(c, right);
    let lt = check_expression(c, frame, None, left, depth);
    let rt = check_expression(c, frame, Some(lt), right, depth);
    if c.types.is_error(lt) || c.types.is_error(rt) {
        return lt;
    }
    let comparable =
        lt == rt && (c.types.is_numeric(lt) || c.types.data(lt) == Some(TypeData::String));
    if comparable {
        return primitive_type(c, "bool");
    }
    report(
        c,
        "T003",
        "ordering requires equal numeric or string operands".to_string(),
        right_span,
    );
    error_type(c)
}

fn check_equality(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    left: ExprId,
    right: ExprId,
    depth: u32,
) -> TypeId {
    let right_span = expr_span(c, right);
    let lt = check_expression(c, frame, None, left, depth);
    let rt = check_expression(c, frame, Some(lt), right, depth);
    if c.types.is_error(lt) || c.types.is_error(rt) {
        return lt;
    }
    let lt_data = c.types.data(lt);
    let rt_data = c.types.data(rt);
    let mismatches_null = |data: Option<TypeData>| {
        !matches!(data, Some(TypeData::Optional(_)) | Some(TypeData::Null))
    };
    if lt == rt && c.types.supports_equality(lt, 0) {
        return primitive_type(c, "bool");
    }
    if lt_data == Some(TypeData::Null) {
        if mismatches_null(rt_data) {
            report(
                c,
                "T003",
                "null compares only against optional or null".to_string(),
                right_span,
            );
            return error_type(c);
        }
        return primitive_type(c, "bool");
    }
    if rt_data == Some(TypeData::Null) {
        if mismatches_null(lt_data) {
            report(
                c,
                "T003",
                "null compares only against optional or null".to_string(),
                right_span,
            );
            return error_type(c);
        }
        return primitive_type(c, "bool");
    }
    let optional_compatible = match (lt_data, rt_data) {
        (Some(TypeData::Optional(inner)), Some(_)) => {
            inner == rt
                || matches!(c.types.data(rt), Some(TypeData::Optional(other)) if other == inner)
        }
        (Some(_), Some(TypeData::Optional(inner))) => inner == lt,
        _ => false,
    };
    if optional_compatible {
        return primitive_type(c, "bool");
    }
    report(
        c,
        "T003",
        "these types cannot be compared".to_string(),
        right_span,
    );
    error_type(c)
}

/// Checks a ternary and unifies its branch types.
fn check_conditional(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    condition: ExprId,
    then_value: ExprId,
    else_value: ExprId,
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let bool_ty = primitive_type(c, "bool");
    let condition_span = expr_span(c, condition);
    let else_span = expr_span(c, else_value);
    let cond_ty = check_expression(c, frame, Some(bool_ty), condition, depth);
    if !c.types.is_error(cond_ty) && cond_ty != bool_ty {
        report(
            c,
            "T003",
            "condition must be bool".to_string(),
            condition_span,
        );
    }
    let t1 = check_expression(c, frame, expected, then_value, depth);
    let t2 = check_expression(c, frame, expected, else_value, depth);
    let t1_error = c.types.is_error(t1);
    let t2_error = c.types.is_error(t2);
    if t1_error && t2_error {
        return error_type(c);
    }
    if t1_error {
        return t2;
    }
    if t2_error {
        return t1;
    }
    if t1 == t2 {
        return t1;
    }
    let t1_data = c.types.data(t1);
    let t2_data = c.types.data(t2);
    if t1_data == Some(TypeData::Null) {
        return optional_form(c, t2);
    }
    if t2_data == Some(TypeData::Null) {
        return optional_form(c, t1);
    }
    if let Some(TypeData::Optional(inner)) = t2_data {
        if inner == t1 {
            return t2;
        }
    }
    if let Some(TypeData::Optional(inner)) = t1_data {
        if inner == t2 {
            return t1;
        }
    }
    report(
        c,
        "T003",
        "conditional branches have incompatible types".to_string(),
        else_span,
    );
    error_type(c)
}

/// Lifts one branch type to its optional form for null-branch joining.
fn optional_form(c: &mut Checker, ty: TypeId) -> TypeId {
    if c.types.is_optional(ty) {
        ty
    } else {
        intern_optional(c, ty)
    }
}

/// Checks a call through its resolved callee.
fn check_call(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    id: ExprId,
    span: Span,
    arguments: &[ExprId],
    depth: u32,
) -> TypeId {
    let callee = c
        .calls
        .get(id.0 as usize)
        .copied()
        .flatten()
        .map(|call| call.callee);
    match callee {
        Some(Callee::Function(fid)) => {
            if c.entry_kinds
                .get(fid.0 as usize)
                .copied()
                .flatten()
                .is_some()
            {
                report(c, "T004", "cannot call an entry function".to_string(), span);
            }
            let name = c
                .module
                .function(fid)
                .map(|func| func.name.clone())
                .unwrap_or_default();
            let (params, result) = match c.signatures.get(fid.0 as usize) {
                Some(sig) => (sig.params.clone(), sig.result),
                None => (Vec::new(), error_type(c)),
            };
            if arguments.len() != params.len() {
                report(
                    c,
                    "T004",
                    format!("function {name} expects {} arguments", params.len()),
                    span,
                );
            }
            for (position, &argument) in arguments.iter().enumerate() {
                let expected = params.get(position).copied();
                let actual = check_expression(c, frame, expected, argument, depth);
                if let Some(expected) = expected {
                    if !c.types.is_error(expected)
                        && !c.types.is_error(actual)
                        && !assignable(c, expected, actual)
                    {
                        report_mismatch(
                            c,
                            expected,
                            actual,
                            argument,
                            expr_span(c, argument),
                            "argument",
                        );
                    }
                }
            }
            result
        }
        Some(Callee::Builtin(bid)) => {
            builtins::check_builtin_call(c, frame, bid, id, arguments, depth)
        }
        Some(Callee::Error) | None => {
            for &argument in arguments {
                check_expression(c, frame, None, argument, depth);
            }
            error_type(c)
        }
    }
}

/// True iff `actual` may flow into `expected`.
pub(super) fn assignable(c: &Checker, expected: TypeId, actual: TypeId) -> bool {
    if c.types.is_error(expected) || c.types.is_error(actual) {
        return true;
    }
    if expected == actual {
        return true;
    }
    if c.types.data(actual) == Some(TypeData::Null) && c.types.is_optional(expected) {
        return true;
    }
    if let Some(inner) = c.types.underlying_optional(expected) {
        if inner == actual {
            return true;
        }
    }
    false
}

/// Reports a type mismatch at `span` with a short usage context.
pub(super) fn report_mismatch(
    c: &mut Checker,
    expected: TypeId,
    actual: TypeId,
    actual_expr: ExprId,
    span: Span,
    context: &str,
) {
    if c.types.is_error(expected) || c.types.is_error(actual) {
        return;
    }
    let expected_name = type_name(c, expected);
    let actual_name = type_name(c, actual);
    let is_integer_literal = matches!(
        c.module.expression(actual_expr).map(|expr| &expr.kind),
        Some(ExprKind::Integer { .. })
    );
    if c.types.is_numeric(expected) && c.types.is_numeric(actual) && expected != actual {
        let hint = match c.types.data(expected) {
            Some(TypeData::I32) => " (i32 range -2147483648..=2147483647)",
            Some(TypeData::I64) => " (i64 range -9223372036854775808..=9223372036854775807)",
            _ => "",
        };
        if is_integer_literal {
            report(
                c,
                "T001",
                format!("integer literal not representable as {expected_name} ({context}){hint}"),
                span,
            );
        } else {
            report(
                c,
                "T002",
                format!(
                    "no implicit conversion from {actual_name} to {expected_name} ({context}){hint}"
                ),
                span,
            );
        }
    } else {
        report(
            c,
            "T001",
            format!("expected {expected_name}, found {actual_name} ({context})"),
            span,
        );
    }
}

/// Renders one interned type for diagnostics.
pub(super) fn type_name(c: &Checker, id: TypeId) -> String {
    type_name_inner(c, id, 0)
}

fn type_name_inner(c: &Checker, id: TypeId, depth: u32) -> String {
    if depth > 32 {
        return "?".to_string();
    }
    match c.types.data(id) {
        Some(TypeData::I32) => "i32".to_string(),
        Some(TypeData::I64) => "i64".to_string(),
        Some(TypeData::F64) => "f64".to_string(),
        Some(TypeData::Bool) => "bool".to_string(),
        Some(TypeData::String) => "string".to_string(),
        Some(TypeData::Bytes) => "bytes".to_string(),
        Some(TypeData::Null) => "null".to_string(),
        Some(TypeData::Validation) => "Validation".to_string(),
        Some(TypeData::Batch) => "Batch".to_string(),
        Some(TypeData::Request) => "Request".to_string(),
        Some(TypeData::Response) => "Response".to_string(),
        Some(TypeData::Optional(inner)) => {
            format!("{}?", type_name_inner(c, inner, depth.saturating_add(1)))
        }
        Some(TypeData::List(inner)) => {
            format!("[{}]", type_name_inner(c, inner, depth.saturating_add(1)))
        }
        Some(TypeData::Record(index)) => match c.types.record(index) {
            Some(shape) => {
                let rendered: Vec<String> = shape
                    .fields
                    .iter()
                    .map(|(name, ty)| {
                        format!(
                            "{}: {}",
                            name,
                            type_name_inner(c, *ty, depth.saturating_add(1))
                        )
                    })
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            None => "{record}".to_string(),
        },
        Some(TypeData::Error) | None => "error".to_string(),
    }
}
