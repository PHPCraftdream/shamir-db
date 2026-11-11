use crate::front::{ExprId, ExprKind};
use crate::sem::effects::BuiltinId;

use super::super::super::entry::{Checker, PlanFact, PlanOperation};
use super::super::super::statement::FunctionFrame;

pub(in crate::sem::check::builtins) fn record_plan_fact(
    c: &mut Checker,
    frame: &FunctionFrame,
    bid: BuiltinId,
    call_id: ExprId,
    args: &[ExprId],
) -> Option<PlanFact> {
    match bid {
        BuiltinId::Batch => Some(PlanFact::Batch {
            root: call_id,
            operations: Vec::new(),
            unknown_alias: false,
        }),
        BuiltinId::BatchId => plan_argument(c, frame, *args.first()?),
        BuiltinId::BatchRead | BuiltinId::BatchInsert => {
            let base = plan_argument(c, frame, *args.first()?)?;
            append_legacy(c, bid, *args.get(1)?, base)
        }
        BuiltinId::BatchUpdate | BuiltinId::BatchDelete => {
            let base = plan_argument(c, frame, *args.first()?).unwrap_or(PlanFact::Unknown);
            super::mutation::append_mutation_fact(c, bid, *args.get(1)?, base)
        }
        BuiltinId::Request => match plan_argument(c, frame, *args.first()?)? {
            PlanFact::Batch {
                operations,
                unknown_alias,
                ..
            } => Some(PlanFact::Request {
                operations,
                unknown_alias,
            }),
            _ => Some(PlanFact::Unknown),
        },
        BuiltinId::DbExecute => match plan_argument(c, frame, *args.first()?)? {
            PlanFact::Request {
                operations,
                unknown_alias,
                ..
            } => Some(PlanFact::Response {
                operations,
                unknown_alias,
            }),
            _ => Some(PlanFact::Unknown),
        },
        _ => None,
    }
}

type ReferenceProvenance = (ExprId, Vec<(String, PlanOperation)>);

pub(in crate::sem::check) fn reference_provenance(
    c: &Checker,
    frame: &FunctionFrame,
    args: &[ExprId],
) -> Result<ReferenceProvenance, &'static str> {
    let base = args
        .first()
        .copied()
        .ok_or("query_ref_field requires a batch argument")?;
    let producer_alias = args
        .get(1)
        .copied()
        .ok_or("query_ref_field requires an alias argument")?;
    reference_sink_provenance(c, frame, base, producer_alias)
}

/// Returns the known batch root and snapshot for a sink base after proving
/// that `producer_alias` occurs exactly once in that snapshot and is a Read.
/// Unlike the sink's alias, the producer alias must already exist in `base`.
pub(in crate::sem::check) fn reference_sink_provenance(
    c: &Checker,
    frame: &FunctionFrame,
    base: ExprId,
    producer_alias: ExprId,
) -> Result<ReferenceProvenance, &'static str> {
    let Some(PlanFact::Batch {
        root,
        operations,
        unknown_alias: false,
    }) = plan_argument(c, frame, base)
    else {
        return Err("query_ref_field requires a known batch plan");
    };
    let alias = literal_alias(c, producer_alias)
        .ok_or("query_ref_field requires a literal producer alias")?;
    if operations.iter().enumerate().any(|(index, (known, _))| {
        operations[..index]
            .iter()
            .any(|(prior, _)| normalized_alias(prior) == normalized_alias(known))
    }) {
        return Err("query_ref_field requires unique batch aliases");
    }
    let mut matching = operations
        .iter()
        .filter(|(known, _)| normalized_alias(known) == normalized_alias(&alias));
    match (matching.next(), matching.next()) {
        (Some((_, PlanOperation::Read)), None) => Ok((root, operations)),
        _ => Err("query_ref_field alias must identify exactly one read"),
    }
}

fn normalized_alias(alias: &str) -> &str {
    alias.strip_prefix('@').unwrap_or(alias)
}

fn plan_argument(c: &Checker, frame: &FunctionFrame, expr: ExprId) -> Option<PlanFact> {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => plan_argument(c, frame, inner),
        Some(ExprKind::Identifier(_)) => match c.resolved.get(expr.0 as usize).copied().flatten() {
            Some(
                crate::sem::ResolvedTarget::Local(slot)
                | crate::sem::ResolvedTarget::Parameter(slot),
            ) => frame.local_plan_facts.get(slot as usize).cloned(),
            _ => None,
        },
        Some(ExprKind::Call { .. }) => c.plan_facts.get(expr.0 as usize).and_then(Clone::clone),
        _ => None,
    }
}

fn literal_alias(c: &Checker, expr: ExprId) -> Option<String> {
    match c.module.expression(expr).map(|node| node.kind.clone()) {
        Some(ExprKind::Group(inner)) => literal_alias(c, inner),
        Some(ExprKind::String(value)) => Some(value),
        _ => None,
    }
}

fn append_legacy(
    c: &mut Checker,
    bid: BuiltinId,
    expr: ExprId,
    fact: PlanFact,
) -> Option<PlanFact> {
    let PlanFact::Batch {
        root,
        mut operations,
        mut unknown_alias,
    } = fact
    else {
        return Some(PlanFact::Unknown);
    };
    let alias = literal_alias(c, expr);
    let has_mutation = operations
        .iter()
        .any(|(_, kind)| matches!(kind, PlanOperation::Update | PlanOperation::Delete));
    if has_mutation
        && (alias.is_none()
            || alias
                .as_ref()
                .is_some_and(|value| operations.iter().any(|(old, _)| old == value)))
    {
        crate::sem::check::entry::report(
            c,
            "T010",
            format!(
                "builtin {} cannot prove unique aliases after a mutation",
                crate::sem::effects::builtin_name(bid)
            ),
            crate::sem::check::entry::expr_span(c, expr),
        );
        return Some(PlanFact::Unknown);
    }
    if let Some(alias) = alias {
        let kind = if bid == BuiltinId::BatchRead {
            PlanOperation::Read
        } else {
            PlanOperation::Insert
        };
        operations.push((alias, kind));
    } else {
        unknown_alias = true;
    }
    Some(PlanFact::Batch {
        root,
        operations,
        unknown_alias,
    })
}
