use crate::api::Span;
use crate::front::{ExprId, ExprKind};
use crate::sem::effects::{BuiltinCategory, BuiltinId};
use crate::sem::tables::{ReferenceKind, ReferenceMarker, ResolvedTarget};
use crate::sem::types::{TypeData, TypeId};

use super::super::super::entry::{
    error_type, expr_span, primitive_type, report, Checker, PlanFact, PlanOperation,
};
use super::super::super::statement::FunctionFrame;

#[derive(Clone, Debug)]
pub(in crate::sem::check) struct QueryRefFact {
    pub(in crate::sem::check) root: ExprId,
    pub(in crate::sem::check) snapshot: Vec<(String, PlanOperation)>,
    pub(in crate::sem::check) alias: String,
    pub(in crate::sem::check) index: u64,
    pub(in crate::sem::check) field: String,
    pub(in crate::sem::check) kind: ReferenceKind,
}

#[derive(Clone, Debug)]
pub(in crate::sem::check) struct RefLocal {
    pub(in crate::sem::check) markers: Vec<QueryRefFact>,
    pub(in crate::sem::check) consumed: bool,
}

#[derive(Clone, Debug)]
pub(in crate::sem::check) struct ReferenceSite {
    expected: TypeId,
    fact: Option<QueryRefFact>,
}

pub(in crate::sem::check) fn register_reference_sites(
    c: &Checker,
    frame: &mut FunctionFrame,
    mutable: bool,
    expected: Option<TypeId>,
    initializer: ExprId,
) {
    if mutable {
        return;
    }
    let Some(expected) = expected else {
        return;
    };
    let Some(TypeData::Record(shape_id)) =
        c.types
            .data(expected)
            .or_else(|| match c.types.data(expected) {
                Some(TypeData::Optional(inner)) => c.types.data(inner),
                _ => None,
            })
    else {
        return;
    };
    let Some(ExprKind::Record(fields)) = c
        .module
        .expression(initializer)
        .map(|node| node.kind.clone())
    else {
        return;
    };
    for field_id in fields {
        let Some(field) = c.module.value_field(field_id) else {
            continue;
        };
        if !is_query_ref_call(c, field.value) {
            continue;
        }
        let Some(field_type) = c
            .types
            .record(shape_id)
            .and_then(|shape| shape.field(&field.name))
        else {
            continue;
        };
        frame.ref_sites.insert(
            field.value.0,
            ReferenceSite {
                expected: field_type,
                fact: None,
            },
        );
    }
}

pub(in crate::sem::check) fn check_query_ref_field(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    call_id: ExprId,
    call_span: Span,
    args: &[ExprId],
    depth: u32,
) -> TypeId {
    if args.len() != 4 {
        report(
            c,
            "T004",
            "builtin query_ref_field expects 4 arguments".to_string(),
            call_span,
        );
        for &arg in args {
            super::super::super::expression::check_expression(c, frame, None, arg, depth);
        }
        return error_type(c);
    }
    let permitted = frame.ceiling.permits(BuiltinCategory::Stage2);
    if !permitted {
        report(
            c,
            "T007",
            "builtin query_ref_field requires a procedure entry".to_string(),
            call_span,
        );
    }

    let batch = primitive_type(c, "batch");
    let string = primitive_type(c, "string");
    let (batch_arg, alias_arg, index_arg, field_arg) = (args[0], args[1], args[2], args[3]);
    let batch_type =
        super::super::super::expression::check_expression(c, frame, Some(batch), batch_arg, depth);
    let alias_type =
        super::super::super::expression::check_expression(c, frame, Some(string), alias_arg, depth);
    let index_type =
        super::super::super::expression::check_expression(c, frame, None, index_arg, depth);
    let field_type =
        super::super::super::expression::check_expression(c, frame, Some(string), field_arg, depth);
    let mut bad_type = false;
    for (want, actual, expr) in [
        (batch, batch_type, batch_arg),
        (string, alias_type, alias_arg),
        (string, field_type, field_arg),
    ] {
        if !c.types.is_error(actual) && actual != want {
            report(
                c,
                "T004",
                format!("query_ref_field argument must be {}", type_name(c, want)),
                expr_span(c, expr),
            );
            bad_type = true;
        }
    }
    if !c.types.is_error(index_type)
        && !matches!(
            c.types.data(index_type),
            Some(TypeData::I32 | TypeData::I64)
        )
    {
        report(
            c,
            "T004",
            "query_ref_field index must be an integer".to_string(),
            expr_span(c, index_arg),
        );
        bad_type = true;
    }
    if bad_type
        || c.types.is_error(batch_type)
        || c.types.is_error(alias_type)
        || c.types.is_error(field_type)
        || c.types.is_error(index_type)
    {
        return error_type(c);
    }
    if !permitted {
        return error_type(c);
    }

    let Some(alias) = literal_string(c, alias_arg) else {
        report(
            c,
            "T009",
            "query_ref_field producer alias must be a string literal".to_string(),
            expr_span(c, alias_arg),
        );
        return error_type(c);
    };
    if !valid_field_name(&alias) {
        report(
            c,
            "T009",
            "query_ref_field producer alias must match [A-Za-z_][A-Za-z0-9_]{0,63}".to_string(),
            expr_span(c, alias_arg),
        );
        return error_type(c);
    }
    let Some((index, negative)) = literal_index(c, index_arg) else {
        report(
            c,
            "T009",
            "query_ref_field index must be a non-negative integer literal".to_string(),
            expr_span(c, index_arg),
        );
        return error_type(c);
    };
    if negative {
        report(
            c,
            "T009",
            "query_ref_field index cannot be negative".to_string(),
            expr_span(c, index_arg),
        );
        return error_type(c);
    }
    let Some(field) = literal_string(c, field_arg) else {
        report(
            c,
            "T009",
            "query_ref_field field must be a string literal".to_string(),
            expr_span(c, field_arg),
        );
        return error_type(c);
    };
    if !valid_field_name(&field) {
        report(
            c,
            "T009",
            "query_ref_field field must match [A-Za-z_][A-Za-z0-9_]{0,63}".to_string(),
            expr_span(c, field_arg),
        );
        return error_type(c);
    }
    let Some(site) = frame.ref_sites.get(&call_id.0) else {
        report(
            c,
            "T010",
            "query_ref_field is only allowed in a const annotated record local".to_string(),
            call_span,
        );
        return error_type(c);
    };
    let Some(kind) = reference_kind(c, site.expected) else {
        report(
            c,
            "T009",
            "query_ref_field requires a non-null scalar field type".to_string(),
            call_span,
        );
        return error_type(c);
    };
    let (root, snapshot) = match super::reference_provenance(c, frame, args) {
        Ok(provenance) => provenance,
        Err(message) => {
            report(c, "T010", message.to_string(), call_span);
            return error_type(c);
        }
    };
    let fact = QueryRefFact {
        root,
        snapshot,
        alias,
        index,
        field,
        kind,
    };
    if let Some(site) = frame.ref_sites.get_mut(&call_id.0) {
        site.fact = Some(fact);
    }
    site_type(frame, call_id).unwrap_or_else(|| error_type(c))
}

pub(in crate::sem::check) fn finish_reference_local(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    slot: Option<u32>,
    mutable: bool,
    annotated: bool,
    initializer: ExprId,
) {
    let Some(slot) = slot else {
        return;
    };
    if mutable || !annotated {
        return;
    }
    let Some(ExprKind::Record(fields)) = c
        .module
        .expression(initializer)
        .map(|node| node.kind.clone())
    else {
        return;
    };
    let mut sites: Vec<(String, ExprId, QueryRefFact)> = fields
        .into_iter()
        .filter_map(|field_id| {
            let field = c.module.value_field(field_id)?;
            let fact = frame.ref_sites.get(&field.value.0)?.fact.clone()?;
            Some((field.name.clone(), field.value, fact))
        })
        .collect();
    sites.sort_by(|left, right| left.0.cmp(&right.0));
    if sites.is_empty() {
        return;
    }
    let mut markers = Vec::with_capacity(sites.len());
    for (_, call_id, fact) in sites {
        if let Some(marker_slot) = c.reference_markers.get_mut(call_id.0 as usize) {
            *marker_slot = Some(ReferenceMarker {
                alias: normalize_alias(&fact.alias),
                path: format!("[{}].{}", fact.index, fact.field),
                kind: fact.kind,
            });
        }
        markers.push(fact);
    }
    if let Some(local) = frame.ref_locals.get_mut(slot as usize) {
        *local = Some(RefLocal {
            markers,
            consumed: false,
        });
    }
}

pub(in crate::sem::check) fn reference_identifier(c: &Checker, id: ExprId) -> Option<ExprId> {
    grouped_identifier(c, id)
}

pub(in crate::sem::check) fn check_reference_use(
    c: &mut Checker,
    frame: &FunctionFrame,
    id: ExprId,
) -> bool {
    let slot = match c.resolved.get(id.0 as usize).copied().flatten() {
        Some(ResolvedTarget::Local(slot)) => slot,
        _ => return true,
    };
    let Some(Some(local)) = frame.ref_locals.get(slot as usize) else {
        return true;
    };
    if frame.ref_sink == Some(id) && !local.consumed {
        return true;
    }
    report(
        c,
        "T010",
        "a scalar reference local may only be used once as a batch_read or batch_insert value"
            .to_string(),
        expr_span(c, id),
    );
    false
}

pub(in crate::sem::check) fn check_reference_sink(
    c: &mut Checker,
    frame: &mut FunctionFrame,
    call_id: ExprId,
    bid: BuiltinId,
    args: &[ExprId],
) {
    let Some(value_expr) = args.get(3).copied() else {
        return;
    };
    let Some(identifier) = grouped_identifier(c, value_expr) else {
        return;
    };
    let Some(ResolvedTarget::Local(slot)) =
        c.resolved.get(identifier.0 as usize).copied().flatten()
    else {
        return;
    };
    let Some(Some(mut local)) = frame.ref_locals.get(slot as usize).cloned() else {
        return;
    };
    let mut valid = true;
    let Some(base) = args.first().copied() else {
        return;
    };
    let (root, operations) = match reference_batch_plan(c, frame, base) {
        Some(plan) => plan,
        None => {
            report(
                c,
                "T010",
                "reference sink requires a known batch plan".to_string(),
                expr_span(c, call_id),
            );
            return;
        }
    };
    let sink_alias = args
        .get(1)
        .and_then(|id| literal_string(c, *id))
        .map(|alias| normalize_alias(&alias));
    if sink_alias.is_none() {
        report(
            c,
            "T010",
            "reference sink alias must be a literal not already in the base batch".to_string(),
            expr_span(c, call_id),
        );
        valid = false;
    }
    if let Some(alias) = sink_alias {
        if operations
            .iter()
            .any(|(existing, _)| normalize_alias(existing) == alias)
        {
            report(
                c,
                "T010",
                "reference sink alias must be unique in the batch".to_string(),
                expr_span(c, call_id),
            );
            valid = false;
        }
    }
    for fact in &local.markers {
        if fact.root != root || !operations.starts_with(&fact.snapshot) {
            report(
                c,
                "T010",
                "reference sink batch must share the root and extend the reference snapshot"
                    .to_string(),
                expr_span(c, identifier),
            );
            valid = false;
            break;
        }
    }
    for fact in &local.markers {
        if !operations.iter().any(|(alias, kind)| {
            alias_key(alias) == alias_key(&fact.alias) && matches!(kind, PlanOperation::Read)
        }) {
            report(
                c,
                "T010",
                "reference producer must remain a read in the sink batch".to_string(),
                expr_span(c, identifier),
            );
            valid = false;
        }
    }
    if local.consumed {
        report(
            c,
            "T010",
            "a scalar reference local may only be consumed once".to_string(),
            expr_span(c, identifier),
        );
        valid = false;
    }
    local.consumed = true;
    if let Some(frame_local) = frame.ref_locals.get_mut(slot as usize) {
        *frame_local = Some(local.clone());
    }
    if !valid {
        return;
    }
    if !matches!(bid, BuiltinId::BatchRead | BuiltinId::BatchInsert) {
        report(
            c,
            "T010",
            "references are only allowed in batch_read and batch_insert values".to_string(),
            expr_span(c, call_id),
        );
        return;
    }
    let markers = local
        .markers
        .iter()
        .map(|fact| ReferenceMarker {
            alias: normalize_alias(&fact.alias),
            path: format!("[{}].{}", fact.index, fact.field),
            kind: fact.kind,
        })
        .collect();
    if let Some(sink) = c.reference_sinks.get_mut(call_id.0 as usize) {
        *sink = markers;
    }
}

fn site_type(frame: &FunctionFrame, id: ExprId) -> Option<TypeId> {
    frame.ref_sites.get(&id.0).map(|site| site.expected)
}

fn is_query_ref_call(c: &Checker, id: ExprId) -> bool {
    matches!(
        c.calls.get(id.0 as usize).copied().flatten(),
        Some(crate::sem::tables::ResolvedCall {
            callee: crate::sem::tables::Callee::Builtin(BuiltinId::QueryRefField)
        })
    )
}

fn reference_kind(c: &Checker, ty: TypeId) -> Option<ReferenceKind> {
    match c.types.data(ty) {
        Some(TypeData::String) => Some(ReferenceKind::String),
        Some(TypeData::Bool) => Some(ReferenceKind::Bool),
        Some(TypeData::Bytes) => Some(ReferenceKind::Bytes),
        Some(TypeData::F64) => Some(ReferenceKind::F64),
        Some(TypeData::I32) => Some(ReferenceKind::I32),
        Some(TypeData::I64) => Some(ReferenceKind::I64),
        _ => None,
    }
}

fn literal_string(c: &Checker, id: ExprId) -> Option<String> {
    match c.module.expression(id).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => literal_string(c, inner),
        Some(ExprKind::String(value)) => Some(value),
        _ => None,
    }
}

fn literal_index(c: &Checker, id: ExprId) -> Option<(u64, bool)> {
    match c.module.expression(id).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => literal_index(c, inner),
        Some(ExprKind::Integer { magnitude, .. }) if magnitude <= i32::MAX as u64 => {
            Some((magnitude, false))
        }
        Some(ExprKind::Unary {
            op: crate::front::UnaryOp::Negate,
            operand,
        }) => match c.module.expression(operand).map(|node| node.kind.clone()) {
            Some(ExprKind::Integer { magnitude, .. }) if magnitude <= (i32::MAX as u64) + 1 => {
                Some((magnitude, true))
            }
            _ => None,
        },
        _ => None,
    }
}

fn valid_field_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn normalize_alias(alias: &str) -> String {
    if alias.starts_with('@') {
        alias.to_string()
    } else {
        format!("@{alias}")
    }
}

fn grouped_identifier(c: &Checker, id: ExprId) -> Option<ExprId> {
    match c.module.expression(id).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => grouped_identifier(c, inner),
        Some(ExprKind::Identifier(_)) => Some(id),
        _ => None,
    }
}

fn reference_batch_plan(
    c: &Checker,
    frame: &FunctionFrame,
    base: ExprId,
) -> Option<(ExprId, Vec<(String, PlanOperation)>)> {
    let fact = match c.module.expression(base).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => return reference_batch_plan(c, frame, inner),
        Some(ExprKind::Identifier(_)) => match c.resolved.get(base.0 as usize).copied().flatten() {
            Some(ResolvedTarget::Local(slot) | ResolvedTarget::Parameter(slot)) => {
                frame.local_plan_facts.get(slot as usize)?.clone()
            }
            _ => return None,
        },
        Some(ExprKind::Call { .. }) => c.plan_facts.get(base.0 as usize)?.clone()?,
        _ => return None,
    };
    let PlanFact::Batch {
        root,
        operations,
        unknown_alias: false,
    } = fact
    else {
        return None;
    };
    if operations.iter().enumerate().any(|(index, (alias, _))| {
        operations[..index]
            .iter()
            .any(|(prior, _)| alias_key(prior) == alias_key(alias))
    }) {
        return None;
    }
    Some((root, operations))
}

fn alias_key(alias: &str) -> &str {
    alias.strip_prefix('@').unwrap_or(alias)
}

fn type_name(c: &Checker, ty: TypeId) -> &'static str {
    match c.types.data(ty) {
        Some(TypeData::Batch) => "batch",
        Some(TypeData::String) => "string",
        _ => "the expected type",
    }
}
