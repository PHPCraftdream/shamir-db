use crate::api::Span;
use crate::front::ExprId;
use crate::sem::effects::{builtin_name, BuiltinCategory, BuiltinId};
use crate::sem::types::TypeId;

use super::super::super::entry::{
    error_type, expr_span, intern_optional, primitive_type, report, Checker, PlanFact,
    PlanOperation,
};
use super::super::super::expression::{assignable, check_expression, report_mismatch};
use super::super::super::statement::FunctionFrame;

pub(in crate::sem::check::builtins) fn check_response_affected(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    call_span: Span,
    args: &[ExprId],
    expected: Option<TypeId>,
    depth: u32,
) -> TypeId {
    let name = builtin_name(BuiltinId::ResponseAffected);
    if args.len() != 2 {
        report(
            c,
            "T004",
            format!("builtin {name} expects 2 arguments"),
            call_span,
        );
        for &arg in args {
            check_expression(c, frame, None, arg, depth);
        }
        return error_type(c);
    }
    let response = primitive_type(c, "response");
    let string = primitive_type(c, "string");
    let response_ty = check_expression(c, frame, Some(response), args[0], depth);
    if !c.types.is_error(response_ty) && !assignable(c, response, response_ty) {
        report_mismatch(
            c,
            response,
            response_ty,
            args[0],
            expr_span(c, args[0]),
            "argument",
        );
    }
    let alias_ty = check_expression(c, frame, Some(string), args[1], depth);
    if !c.types.is_error(alias_ty) && !assignable(c, string, alias_ty) {
        report_mismatch(
            c,
            string,
            alias_ty,
            args[1],
            expr_span(c, args[1]),
            "argument",
        );
    }
    match response_fact(c, frame, args[0]) {
        Some(PlanFact::Response {
            operations,
            unknown_alias: false,
        }) => match literal_string(c, args[1]) {
            Some(alias) => match operations
                .iter()
                .find(|(candidate, _)| candidate == &alias)
                .map(|(_, kind)| kind)
            {
                Some(PlanOperation::Update | PlanOperation::Delete) | None => {}
                Some(PlanOperation::Read | PlanOperation::Insert) => report(
                    c,
                    "T001",
                    format!("builtin {name} requires an update or delete alias"),
                    expr_span(c, args[1]),
                ),
            },
            None => report(
                c,
                "T010",
                format!("builtin {name} cannot prove the mutation alias"),
                expr_span(c, args[1]),
            ),
        },
        _ => report(
            c,
            "T010",
            format!("builtin {name} cannot prove the alias is a mutation result"),
            expr_span(c, args[1]),
        ),
    }
    if let Some(effects) = c.own_effects.get_mut(frame.fid.0 as usize) {
        effects.record(BuiltinCategory::Stage2);
    }
    if !frame.ceiling.permits(BuiltinCategory::Stage2) {
        report(
            c,
            "T007",
            format!("builtin {name} requires a procedure entry"),
            call_span,
        );
        return error_type(c);
    }
    let i64_ty = primitive_type(c, "i64");
    let result = intern_optional(c, i64_ty);
    if let Some(expected) = expected {
        if !c.types.is_error(expected) && expected != result {
            report(
                c,
                "T010",
                format!("builtin {name} needs i64? as its contextual result type"),
                call_span,
            );
            return error_type(c);
        }
        expected
    } else {
        result
    }
}

pub(in crate::sem::check::builtins) fn check_row_projection(
    c: &mut Checker,
    frame: &FunctionFrame,
    bid: BuiltinId,
    args: &[ExprId],
) {
    if args.len() != 2 {
        return;
    }
    let Some(PlanFact::Response {
        operations,
        unknown_alias: false,
    }) = response_fact(c, frame, args[0])
    else {
        return;
    };
    let Some(alias) = literal_string(c, args[1]) else {
        return;
    };
    if operations.iter().any(|(known, kind)| {
        known == &alias && matches!(kind, PlanOperation::Update | PlanOperation::Delete)
    }) {
        report(
            c,
            "T001",
            format!(
                "builtin {} cannot project rows for a mutation alias",
                builtin_name(bid)
            ),
            expr_span(c, args[1]),
        );
    }
}

fn response_fact(c: &Checker, frame: &FunctionFrame, expr: ExprId) -> Option<PlanFact> {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(crate::front::ExprKind::Group(inner)) => response_fact(c, frame, inner),
        Some(crate::front::ExprKind::Identifier(_)) => {
            match c.resolved.get(expr.0 as usize).copied().flatten() {
                Some(
                    crate::sem::ResolvedTarget::Local(slot)
                    | crate::sem::ResolvedTarget::Parameter(slot),
                ) => frame.local_plan_facts.get(slot as usize).cloned(),
                _ => None,
            }
        }
        Some(crate::front::ExprKind::Call { .. }) => {
            c.plan_facts.get(expr.0 as usize).and_then(Clone::clone)
        }
        _ => None,
    }
}

fn literal_string(c: &Checker, expr: ExprId) -> Option<String> {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(crate::front::ExprKind::String(value)) => Some(value),
        Some(crate::front::ExprKind::Group(inner)) => literal_string(c, inner),
        _ => None,
    }
}
