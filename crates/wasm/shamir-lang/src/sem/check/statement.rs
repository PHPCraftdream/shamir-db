use crate::api::Span;
use crate::front::{EntryKind, ExprId, ExprKind, ForKind, FunctionId, StatementId, StatementKind};
use crate::sem::effects::Capability;
use crate::sem::tables::{ResolvedTarget, TypedFunction, TypedLocal, TypedParameter};
use crate::sem::types::{TypeData, TypeId};
use std::collections::BTreeMap;

use super::entry::{
    capability_of, convert_type, error_type, expr_span, primitive_type, report, Checker,
    TopPosition,
};
use super::expression::{assignable, check_expression, report_mismatch};
use crate::sem::flow;

/// Per-function checking state threaded through the body walk.
pub(super) struct FunctionFrame {
    pub(super) fid: FunctionId,
    pub(super) return_ty: TypeId,
    pub(super) ceiling: Capability,
    pub(super) loop_depth: u32,
    /// Types indexed by combined slot; error-filled until assigned.
    pub(super) local_types: Vec<TypeId>,
    pub(super) local_plan_facts: Vec<super::entry::PlanFact>,
    pub(super) ref_locals: Vec<Option<super::builtins::RefLocal>>,
    pub(super) ref_sites: BTreeMap<u32, super::builtins::ReferenceSite>,
    pub(super) ref_sink: Option<ExprId>,
    pub(super) in_entry: bool,
    pub(super) kind: Option<EntryKind>,
    /// Set once a control-flow diagnostic (T005) was reported in this body.
    pub(super) flow_reported: bool,
}

/// Walks every function body and assembles the typed functions.
pub(super) fn check_all_bodies(c: &mut Checker) -> Vec<TypedFunction> {
    let count = c.module.functions.len();
    let mut out: Vec<TypedFunction> = Vec::new();
    for index in 0..count {
        let fid = FunctionId(index as u32);
        let Some(func) = c.module.function(fid) else {
            continue;
        };
        let kind = c.entry_kinds.get(index).copied().flatten();
        let ceiling = capability_of(kind);
        let return_ty = c
            .signatures
            .get(index)
            .map(|sig| sig.result)
            .unwrap_or_else(|| error_type(c));
        let slot_count = c
            .slot_mutability
            .get(index)
            .map(|slots| slots.len())
            .unwrap_or(0);
        let initial = error_type(c);
        let mut local_types = vec![initial; slot_count];
        let local_plan_facts = vec![super::entry::PlanFact::Unknown; slot_count];
        let ref_locals = vec![None; slot_count];

        let param_count = func.parameters.len();
        if let Some(sig) = c.signatures.get(index) {
            for (position, ty) in sig.params.iter().enumerate().take(param_count) {
                if let Some(slot) = local_types.get_mut(position) {
                    *slot = *ty;
                }
            }
        }
        let mut frame = FunctionFrame {
            fid,
            return_ty,
            ceiling,
            loop_depth: 0,
            local_types,
            local_plan_facts,
            ref_locals,
            ref_sites: BTreeMap::new(),
            ref_sink: None,
            in_entry: kind.is_some(),
            kind,
            flow_reported: false,
        };
        let mut locals: Vec<TypedLocal> = Vec::new();
        let body = func.body;
        check_statement(c, &mut frame, body, 0, &mut locals);
        let span = func.span;
        let name = func.name.clone();
        if !c.types.is_null_type(frame.return_ty)
            && !frame.flow_reported
            && !flow::always_terminates(c.module, body, 0)
        {
            report(
                c,
                "T005",
                "not every path returns a value".to_string(),
                span,
            );
        }
        let mut parameters: Vec<TypedParameter> = Vec::new();
        for (position, pid) in func.parameters.iter().enumerate() {
            let Some(param) = c.module.parameter(*pid) else {
                continue;
            };
            let ty = c
                .signatures
                .get(index)
                .and_then(|sig| sig.params.get(position).copied())
                .unwrap_or_else(|| error_type(c));
            parameters.push(TypedParameter {
                slot: position as u32,
                name: param.name.clone(),
                ty,
                span: param.span,
            });
        }
        let effects = c.own_effects.get(index).copied().unwrap_or_default();
        out.push(TypedFunction {
            id: fid,
            name,
            kind,
            parameters,
            locals,
            return_type: frame.return_ty,
            effects,
            body,
            span,
        });
    }
    out
}

fn plan_expression_fact(
    c: &Checker,
    frame: &FunctionFrame,
    expr: ExprId,
) -> super::entry::PlanFact {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => plan_expression_fact(c, frame, inner),
        Some(ExprKind::Identifier(_)) => match c.resolved.get(expr.0 as usize).copied().flatten() {
            Some(ResolvedTarget::Local(slot) | ResolvedTarget::Parameter(slot)) => frame
                .local_plan_facts
                .get(slot as usize)
                .cloned()
                .unwrap_or(super::entry::PlanFact::Unknown),
            _ => super::entry::PlanFact::Unknown,
        },
        Some(ExprKind::Call { .. }) => c
            .plan_facts
            .get(expr.0 as usize)
            .and_then(Clone::clone)
            .unwrap_or(super::entry::PlanFact::Unknown),
        _ => super::entry::PlanFact::Unknown,
    }
}

fn invalidate_plan_facts(frame: &mut FunctionFrame) {
    for fact in &mut frame.local_plan_facts {
        *fact = super::entry::PlanFact::Unknown;
    }
}

/// Checks one statement; `locals` collects typed locals in slot order.
pub(super) fn check_statement(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    stmt: StatementId,
    depth: u32,
    locals: &mut Vec<TypedLocal>,
) {
    let (kind, span) = match c.module.statement(stmt) {
        Some(stmt) => (stmt.kind.clone(), stmt.span),
        None => return,
    };
    if depth > c.limits.max_ast_depth as u32 {
        report(
            c,
            "B001",
            "statement nesting exceeds the limit".to_string(),
            span,
        );
        return;
    }
    let next = depth.saturating_add(1);
    match kind {
        StatementKind::Block(inner) => {
            for child in inner {
                check_statement(c, frame, child, next, locals);
            }
        }
        StatementKind::Let {
            mutable,
            name,
            ty,
            value,
        } => {
            let converted = ty
                .map(|ast| convert_type(c, ast, TopPosition::Local, frame.in_entry, frame.kind, 0));
            let value_span = expr_span(c, value);
            super::builtins::register_reference_sites(c, frame, mutable, converted, value);
            let actual = check_expression(c, frame, converted, value, next);
            let local_ty = match converted {
                Some(expected) => {
                    if !assignable(c, expected, actual) {
                        report_mismatch(c, expected, actual, value, value_span, "initializer");
                    }
                    expected
                }
                None => check_inferred_type(c, actual, value_span, 0),
            };
            let slot = c.binding_slots.get(stmt.0 as usize).copied().flatten();
            if let Some(slot) = slot {
                if let Some(slot_entry) = frame.local_types.get_mut(slot as usize) {
                    *slot_entry = local_ty;
                }
                let fact = plan_expression_fact(c, frame, value);
                if let Some(slot_entry) = frame.local_plan_facts.get_mut(slot as usize) {
                    *slot_entry = fact;
                }
                super::builtins::finish_reference_local(
                    c,
                    frame,
                    Some(slot),
                    mutable,
                    ty.is_some(),
                    value,
                );
                locals.push(TypedLocal {
                    slot,
                    name: name.clone(),
                    ty: local_ty,
                    mutable,
                    span,
                });
            }
        }
        StatementKind::Assign { target, value } => {
            check_assign(c, frame, target, value, next);
            let slot = match c.resolved.get(target.0 as usize).copied().flatten() {
                Some(ResolvedTarget::Local(slot)) => Some(slot as usize),
                _ => None,
            };
            let fact = plan_expression_fact(c, frame, value);
            if let Some(fact_slot) = slot.and_then(|slot| frame.local_plan_facts.get_mut(slot)) {
                *fact_slot = fact;
            }
        }
        StatementKind::If {
            condition,
            then_block,
            else_branch,
        } => {
            let bool_ty = primitive_type(c, "bool");
            let condition_span = expr_span(c, condition);
            let cond_ty = check_expression(c, frame, Some(bool_ty), condition, next);
            if !c.types.is_error(cond_ty) && cond_ty != bool_ty {
                report(
                    c,
                    "T003",
                    "condition must be bool".to_string(),
                    condition_span,
                );
            }
            invalidate_plan_facts(frame);
            check_statement(c, frame, then_block, next, locals);
            invalidate_plan_facts(frame);
            if let Some(else_block) = else_branch {
                check_statement(c, frame, else_block, next, locals);
            }
            invalidate_plan_facts(frame);
        }
        StatementKind::While { condition, body } => {
            let bool_ty = primitive_type(c, "bool");
            let condition_span = expr_span(c, condition);
            let cond_ty = check_expression(c, frame, Some(bool_ty), condition, next);
            if !c.types.is_error(cond_ty) && cond_ty != bool_ty {
                report(
                    c,
                    "T003",
                    "condition must be bool".to_string(),
                    condition_span,
                );
            }
            let saved = frame.loop_depth;
            invalidate_plan_facts(frame);
            frame.loop_depth = saved.saturating_add(1);
            check_statement(c, frame, body, next, locals);
            frame.loop_depth = saved;
            invalidate_plan_facts(frame);
        }
        StatementKind::For {
            kind,
            name,
            start_or_list,
            end,
            body,
        } => {
            let item_ty = match kind {
                ForKind::Range => check_range(c, frame, start_or_list, end, next),
                ForKind::List => check_list_iteration(c, frame, start_or_list, next),
            };
            let item_ty = check_inferred_type(c, item_ty, span, 0);
            let slot = c.binding_slots.get(stmt.0 as usize).copied().flatten();
            if let Some(slot) = slot {
                if let Some(slot_entry) = frame.local_types.get_mut(slot as usize) {
                    *slot_entry = item_ty;
                }
                locals.push(TypedLocal {
                    slot,
                    name: name.clone(),
                    ty: item_ty,
                    mutable: false,
                    span,
                });
            }
            let saved = frame.loop_depth;
            invalidate_plan_facts(frame);
            frame.loop_depth = saved.saturating_add(1);
            check_statement(c, frame, body, next, locals);
            frame.loop_depth = saved;
            invalidate_plan_facts(frame);
        }
        StatementKind::Return(None) => {
            if !c.types.is_null_type(frame.return_ty) {
                frame.flow_reported = true;
                report(
                    c,
                    "T005",
                    "bare return requires a -> null function".to_string(),
                    span,
                );
            }
        }
        StatementKind::Return(Some(value)) => {
            let value_span = expr_span(c, value);
            if c.types.is_null_type(frame.return_ty) {
                report(
                    c,
                    "T001",
                    "value return in a -> null function".to_string(),
                    value_span,
                );
                check_expression(c, frame, None, value, next);
            } else {
                let expected = frame.return_ty;
                let actual = check_expression(c, frame, Some(expected), value, next);
                if !assignable(c, expected, actual) {
                    report_mismatch(c, expected, actual, value, value_span, "return value");
                }
            }
        }
        StatementKind::Break | StatementKind::Continue => {
            if frame.loop_depth == 0 {
                frame.flow_reported = true;
                report(c, "T005", "break/continue outside a loop".to_string(), span);
            }
        }
        StatementKind::Throw(value) => {
            let string_ty = primitive_type(c, "string");
            let value_span = expr_span(c, value);
            let actual = check_expression(c, frame, Some(string_ty), value, next);
            if !assignable(c, string_ty, actual) {
                report(c, "T001", "throw requires a string".to_string(), value_span);
            }
        }
        StatementKind::Expression(expr) => {
            check_expression(c, frame, None, expr, next);
        }
    }
}

/// Checks one assignment statement per target shape.
fn check_assign(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    target: ExprId,
    value: ExprId,
    depth: u32,
) {
    let target_span = expr_span(c, target);
    let target_kind = c.module.expression(target).map(|expr| expr.kind.clone());
    match target_kind {
        Some(ExprKind::Identifier(_)) => {
            let resolved = c.resolved.get(target.0 as usize).copied().flatten();
            match resolved {
                Some(ResolvedTarget::Local(slot)) => {
                    let mutable = c
                        .slot_mutability
                        .get(frame.fid.0 as usize)
                        .and_then(|slots| slots.get(slot as usize))
                        .copied();
                    match mutable {
                        Some(false) => {
                            let target_ty = frame
                                .local_types
                                .get(slot as usize)
                                .copied()
                                .unwrap_or_else(|| error_type(c));
                            record_target_type(c, target, target_ty);
                            report(
                                c,
                                "T006",
                                "cannot assign to an immutable local".to_string(),
                                target_span,
                            );
                            check_expression(c, frame, None, value, depth);
                        }
                        Some(true) => {
                            let expected = frame
                                .local_types
                                .get(slot as usize)
                                .copied()
                                .unwrap_or_else(|| error_type(c));
                            record_target_type(c, target, expected);
                            let value_span = expr_span(c, value);
                            let actual = check_expression(c, frame, Some(expected), value, depth);
                            if !assignable(c, expected, actual) {
                                report_mismatch(
                                    c,
                                    expected,
                                    actual,
                                    value,
                                    value_span,
                                    "assigned value",
                                );
                            }
                        }
                        None => {
                            let target_ty = error_type(c);
                            record_target_type(c, target, target_ty);
                            check_expression(c, frame, None, value, depth);
                        }
                    }
                }
                Some(ResolvedTarget::Parameter(slot)) => {
                    let target_ty = frame
                        .local_types
                        .get(slot as usize)
                        .copied()
                        .unwrap_or_else(|| error_type(c));
                    record_target_type(c, target, target_ty);
                    report(
                        c,
                        "T006",
                        "cannot assign to a parameter".to_string(),
                        target_span,
                    );
                    check_expression(c, frame, None, value, depth);
                }
                Some(ResolvedTarget::Function(_)) | Some(ResolvedTarget::Builtin(_)) => {
                    let target_ty = error_type(c);
                    record_target_type(c, target, target_ty);
                    report(
                        c,
                        "T006",
                        "cannot assign to a function or builtin".to_string(),
                        target_span,
                    );
                    check_expression(c, frame, None, value, depth);
                }
                Some(ResolvedTarget::Error) | None => {
                    let target_ty = error_type(c);
                    record_target_type(c, target, target_ty);
                    check_expression(c, frame, None, value, depth);
                }
            }
        }
        Some(ExprKind::Field { .. }) | Some(ExprKind::Index { .. }) => {
            report(
                c,
                "T006",
                "record fields and list slots are immutable".to_string(),
                target_span,
            );
            check_expression(c, frame, None, target, depth);
            check_expression(c, frame, None, value, depth);
        }
        Some(ExprKind::Group(_))
        | Some(ExprKind::Integer { .. })
        | Some(ExprKind::Float(_))
        | Some(ExprKind::String(_))
        | Some(ExprKind::Bytes(_))
        | Some(ExprKind::Boolean(_))
        | Some(ExprKind::Null)
        | Some(ExprKind::List(_))
        | Some(ExprKind::Record(_))
        | Some(ExprKind::Unary { .. })
        | Some(ExprKind::Binary { .. })
        | Some(ExprKind::Conditional { .. })
        | Some(ExprKind::Call { .. }) => {
            report(
                c,
                "T006",
                "invalid assignment target".to_string(),
                target_span,
            );
            check_expression(c, frame, None, value, depth);
        }
        None => {
            check_expression(c, frame, None, value, depth);
        }
    }
}

/// Records the type of an assignment-target expression in the type table.
fn record_target_type(c: &mut Checker, target: ExprId, ty: TypeId) {
    if let Some(slot) = c.expr_types.get_mut(target.0 as usize) {
        *slot = Some(ty);
    }
}

/// Reports opaque types nested inside inferred aggregates (`T009`).
fn check_inferred_type(c: &mut Checker, ty: TypeId, span: Span, depth: u32) -> TypeId {
    if depth > 32 || c.types.is_error(ty) {
        return ty;
    }
    match c.types.data(ty) {
        Some(TypeData::Optional(inner)) | Some(TypeData::List(inner)) => {
            if c.types.is_opaque(inner) {
                report(
                    c,
                    "T009",
                    "opaque type is not permitted here".to_string(),
                    span,
                );
            } else {
                check_inferred_type(c, inner, span, depth.saturating_add(1));
            }
            ty
        }
        Some(TypeData::Record(index)) => {
            let field_types: Vec<TypeId> = c
                .types
                .record(index)
                .map(|shape| shape.fields.iter().map(|(_, ty)| *ty).collect())
                .unwrap_or_default();
            for field_ty in field_types {
                if c.types.is_opaque(field_ty) {
                    report(
                        c,
                        "T009",
                        "opaque type is not permitted here".to_string(),
                        span,
                    );
                } else {
                    check_inferred_type(c, field_ty, span, depth.saturating_add(1));
                }
            }
            ty
        }
        _ => ty,
    }
}

/// Checks a `for i in a..b` header; returns the loop item type.
fn check_range(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    start: ExprId,
    end: Option<ExprId>,
    depth: u32,
) -> TypeId {
    let start_span = expr_span(c, start);
    let start_ty = check_expression(c, frame, None, start, depth);
    let start_ok = c.types.is_integer(start_ty);
    if !c.types.is_error(start_ty) && !start_ok {
        report(
            c,
            "T003",
            "range bounds must be i32 or i64".to_string(),
            start_span,
        );
    }
    if let Some(end_expr) = end {
        let end_span = expr_span(c, end_expr);
        let expected_end = if start_ok { Some(start_ty) } else { None };
        let end_ty = check_expression(c, frame, expected_end, end_expr, depth);
        if start_ok && !c.types.is_error(end_ty) && !assignable(c, start_ty, end_ty) {
            report(
                c,
                "T003",
                "range bounds must have the same type".to_string(),
                end_span,
            );
        }
    }
    if start_ok {
        start_ty
    } else {
        error_type(c)
    }
}

/// Checks a `for item in list` header; returns the element type.
fn check_list_iteration(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    list: ExprId,
    depth: u32,
) -> TypeId {
    let list_span = expr_span(c, list);
    let list_ty = check_expression(c, frame, None, list, depth);
    match c.types.data(list_ty) {
        Some(TypeData::List(elem)) => elem,
        Some(TypeData::Error) => error_type(c),
        _ => {
            report(
                c,
                "T003",
                "list iteration expects a list".to_string(),
                list_span,
            );
            error_type(c)
        }
    }
}
