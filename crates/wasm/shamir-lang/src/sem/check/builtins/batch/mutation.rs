use crate::api::Span;
use crate::front::{ExprId, ExprKind};
use crate::sem::effects::{builtin_name, BuiltinId};
use crate::sem::types::{TypeData, TypeId};

use super::super::super::entry::{expr_span, report, Checker, PlanFact, PlanOperation};
use super::super::super::expression::{assignable, check_expression, report_mismatch};
use super::super::super::statement::FunctionFrame;

pub(in crate::sem::check::builtins) fn check_batch_mutation(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    bid: BuiltinId,
    call_span: Span,
    args: &[ExprId],
    depth: u32,
) -> TypeId {
    let name = builtin_name(bid);
    let arity = if bid == BuiltinId::BatchUpdate { 5 } else { 4 };
    if args.len() != arity {
        report(
            c,
            "T004",
            format!("builtin {name} expects {arity} arguments"),
            call_span,
        );
        return c.types.error();
    }
    let batch = crate::sem::check::entry::primitive_type(c, "batch");
    let string = crate::sem::check::entry::primitive_type(c, "string");
    let batch_expr = args[0];
    let alias_expr = args[1];
    let table_expr = args[2];
    let filter_expr = args[3];
    let batch_ty = check_expression(c, frame, Some(batch), batch_expr, depth);
    if !c.types.is_error(batch_ty) && !assignable(c, batch, batch_ty) {
        report_mismatch(
            c,
            batch,
            batch_ty,
            batch_expr,
            expr_span(c, batch_expr),
            "argument",
        );
    }
    for (expr, label) in [(alias_expr, "alias"), (table_expr, "table")] {
        let actual = check_expression(c, frame, Some(string), expr, depth);
        if !c.types.is_error(actual) && !assignable(c, string, actual) {
            report_mismatch(c, string, actual, expr, expr_span(c, expr), "argument");
        }
        if literal_string(c, expr).is_none() {
            report(
                c,
                "T001",
                format!("builtin {name} requires a literal {label}"),
                expr_span(c, expr),
            );
        }
    }
    let filter_ty = check_record_argument(c, frame, filter_expr, depth);
    check_mutation_predicate(c, bid, filter_expr, filter_ty);
    if bid == BuiltinId::BatchUpdate {
        let set_expr = args[4];
        let set_ty = check_record_argument(c, frame, set_expr, depth);
        check_mutation_set(c, set_expr, set_ty);
    }
    if let Some(effects) = c.own_effects.get_mut(frame.fid.0 as usize) {
        effects.record(crate::sem::effects::BuiltinCategory::Stage2);
    }
    if !frame
        .ceiling
        .permits(crate::sem::effects::BuiltinCategory::Stage2)
    {
        report(
            c,
            "T007",
            format!("builtin {name} requires a procedure entry"),
            call_span,
        );
        return c.types.error();
    }
    batch
}

fn check_record_argument(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    expr: ExprId,
    depth: u32,
) -> TypeId {
    let inferred = infer_record_literal(c, frame, expr, depth);
    check_expression(c, frame, inferred, expr, depth)
}

fn infer_record_literal(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    expr: ExprId,
    depth: u32,
) -> Option<TypeId> {
    let fields = match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => return infer_record_literal(c, frame, inner, depth),
        Some(ExprKind::Record(fields)) => fields,
        _ => return None,
    };
    let mut typed = Vec::with_capacity(fields.len());
    for field_id in fields {
        let (field_name, field_value) = c
            .module
            .value_field(field_id)
            .map(|field| (field.name.clone(), field.value))?;
        let field_ty = match infer_record_literal(c, frame, field_value, depth) {
            Some(ty) => ty,
            None => check_expression(c, frame, None, field_value, depth),
        };
        typed.push((field_name, field_ty));
    }
    Some(c.types.intern_record(typed))
}

fn check_mutation_set(c: &mut Checker, expr: ExprId, ty: TypeId) {
    let nonempty = match c.types.data(ty) {
        Some(TypeData::Record(index)) => c
            .types
            .record(index)
            .is_some_and(|shape| !shape.fields.is_empty()),
        _ => false,
    };
    if !nonempty || !c.types.wire_safe(ty, 0) {
        report(
            c,
            "T001",
            "batch_update set must be a non-empty wire-safe record".to_string(),
            expr_span(c, expr),
        );
    }
}

fn check_mutation_predicate(c: &mut Checker, bid: BuiltinId, expr: ExprId, ty: TypeId) {
    let name = builtin_name(bid);
    let Some(fields) = record_fields(c, expr) else {
        report(
            c,
            "T001",
            format!("builtin {name} filter must be a non-empty record literal"),
            expr_span(c, expr),
        );
        return;
    };
    if fields.is_empty() {
        report(
            c,
            "T001",
            format!("builtin {name} filter must not be empty"),
            expr_span(c, expr),
        );
        return;
    }
    let typed_ok = fields.iter().all(|field_id| {
        c.module.value_field(*field_id).is_some_and(|field| {
            let field_ty = match c.types.data(ty) {
                Some(TypeData::Record(index)) => c
                    .types
                    .record(index)
                    .and_then(|shape| shape.field(&field.name)),
                _ => None,
            };
            field_ty.is_some_and(|field_ty| is_scalar(c, field_ty))
        })
    });
    if !typed_ok {
        report(
            c,
            "T001",
            format!("builtin {name} filter fields must be non-null scalars"),
            expr_span(c, expr),
        );
        return;
    }
    for field_id in fields {
        let Some(field) = c.module.value_field(field_id) else {
            continue;
        };
        if !is_filter_value(c, field.value) {
            report(
                c,
                "T001",
                format!("builtin {name} filter values must be scalar literals or parameters"),
                expr_span(c, field.value),
            );
        }
    }
}

fn record_fields(c: &Checker, expr: ExprId) -> Option<Vec<crate::front::ValueFieldId>> {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => record_fields(c, inner),
        Some(ExprKind::Record(fields)) => Some(fields),
        _ => None,
    }
}

fn is_filter_value(c: &Checker, expr: ExprId) -> bool {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(
            ExprKind::Integer { .. }
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bytes(_)
            | ExprKind::Boolean(_),
        ) => true,
        Some(ExprKind::Group(inner)) => is_filter_value(c, inner),
        Some(ExprKind::Unary {
            op: crate::front::UnaryOp::Negate | crate::front::UnaryOp::Positive,
            operand,
        }) => {
            matches!(
                c.module.expression(operand).map(|node| node.kind.clone()),
                Some(ExprKind::Integer { .. } | ExprKind::Float(_))
            )
        }
        Some(ExprKind::Identifier(_)) => matches!(
            c.resolved.get(expr.0 as usize).copied().flatten(),
            Some(crate::sem::ResolvedTarget::Parameter(_))
        ),
        _ => false,
    }
}

fn is_scalar(c: &Checker, ty: TypeId) -> bool {
    matches!(
        c.types.data(ty),
        Some(
            TypeData::I32
                | TypeData::I64
                | TypeData::F64
                | TypeData::Bool
                | TypeData::String
                | TypeData::Bytes
        )
    )
}

fn literal_string(c: &Checker, expr: ExprId) -> Option<String> {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(ExprKind::String(value)) => Some(value),
        Some(ExprKind::Group(inner)) => literal_string(c, inner),
        _ => None,
    }
}

pub(super) fn append_mutation_fact(
    c: &mut Checker,
    bid: BuiltinId,
    alias_expr: ExprId,
    fact: PlanFact,
) -> Option<PlanFact> {
    let name = builtin_name(bid);
    let Some(alias) = literal_string(c, alias_expr) else {
        return Some(PlanFact::Unknown);
    };
    let PlanFact::Batch {
        root,
        mut operations,
        unknown_alias,
    } = fact
    else {
        report(
            c,
            "T010",
            format!("builtin {name} cannot prove the batch operation plan"),
            expr_span(c, alias_expr),
        );
        return Some(PlanFact::Unknown);
    };
    if unknown_alias || operations.iter().any(|(existing, _)| existing == &alias) {
        report(
            c,
            "T010",
            format!("builtin {name} cannot prove unique mutation aliases"),
            expr_span(c, alias_expr),
        );
        return Some(PlanFact::Unknown);
    }
    operations.push((
        alias,
        if bid == BuiltinId::BatchUpdate {
            PlanOperation::Update
        } else {
            PlanOperation::Delete
        },
    ));
    Some(PlanFact::Batch {
        root,
        operations,
        unknown_alias: false,
    })
}
