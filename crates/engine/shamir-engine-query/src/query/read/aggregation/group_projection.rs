//! Group-level aggregate projection: build each group's output object from
//! the select items (split from the former `read/aggregate.rs`).

use bytes::Bytes;
use shamir_funclib::agg::{Aggregator, DistinctWrapper};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::{RecordRef, RecordView, ScalarRef};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::{
    prescan_cond_cache, prescan_field_path_cache, prescan_query_ref_cache, FilterContext,
    FilterValue, FnCall,
};
use crate::query::read::{AggregateField, Select, SelectItem};

use super::accumulator::{
    intern_field_path_keys_with_overlay, step_aggregate_input, AggAccum, AggState,
};
use super::function_inputs::{
    aggregate_accumulation_error, make_aggregate_fn, resolve_aggregate_input,
};

// ============================================================================
// build_aggregate_object
// ============================================================================

/// Build a QueryValue map from select items for a group of records.
///
/// S4: the group is walked as `&[(RecordId, Bytes)]`; a per-row `RecordView`
/// (with a bare-scalar `InnerValue::from_bytes` fallback) supplies every
/// accumulator via the `RecordRef` lens. `SelectItem::CountAll` is
/// short-circuited to `group_records.len()` (no lens touched).
pub(in crate::query::read) fn build_aggregate_object(
    group_records: &[(RecordId, Bytes)],
    select: &Select,
    group_key_values: Option<&[(String, QueryValue)]>,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> DbResult<QueryValue> {
    let mut obj: indexmap::IndexMap<String, QueryValue, shamir_collections::THasher> =
        new_map_wc(select.items.len());

    // Add group key values if provided
    if let Some(keys) = group_key_values {
        for (key, val) in keys {
            obj.insert(key.clone(), val.clone());
        }
    }

    // First pass over select items: allocate accumulators and remember the
    // output key for each one.
    let mut agg_slots: Vec<(String, AggAccum, Option<&FilterValue>)> = Vec::new();
    let mut field_slots: Vec<(String, &[String])> = Vec::new();
    #[allow(clippy::type_complexity)] // parallel to agg_slots; clarity over brevity
    let mut fn_slots: Vec<(
        String,
        Option<Vec<InternerKey>>,
        bool,
        Option<Box<dyn Aggregator>>,
        Option<&FilterValue>,
    )> = Vec::new();
    let mut func_slots: Vec<(String, FilterValue)> = Vec::new();

    for item in &select.items {
        match item {
            SelectItem::CountAll { alias } => {
                let key = alias.as_deref().unwrap_or("count");
                obj.insert(key.to_string(), QueryValue::Int(group_records.len() as i64));
            }
            SelectItem::Aggregate {
                func,
                field,
                input,
                alias,
                ..
            } => {
                let key = match alias {
                    Some(a) => a.clone(),
                    None => match field {
                        AggregateField::Field(f) => {
                            format!("{:?}_{}", func, f.join(".")).to_lowercase()
                        }
                        AggregateField::All => format!("{:?}", func).to_lowercase(),
                    },
                };
                agg_slots.push((
                    key,
                    AggAccum::new_with_snapshots(
                        *func,
                        field,
                        interner,
                        ctx.interner_overlay_ids.as_deref(),
                        ctx.interner_overlay.clone(),
                    ),
                    input.as_ref(),
                ));
            }
            SelectItem::Field { path, alias } => {
                let default_key = path.last().map(|s| s.as_str()).unwrap_or("");
                let key = alias.as_deref().unwrap_or(default_key);
                if !obj.contains_key(key) {
                    field_slots.push((key.to_string(), path));
                }
            }
            SelectItem::AggregateFn {
                name,
                field,
                args,
                alias,
                distinct,
                input,
                ..
            } => {
                let key = match alias {
                    Some(a) => a.clone(),
                    None => match field {
                        AggregateField::Field(f) => format!("{}_{}", name, f.join(".")),
                        AggregateField::All => name.clone(),
                    },
                };
                let (field_path, all_field) = match field {
                    AggregateField::Field(p) => (
                        intern_field_path_keys_with_overlay(
                            p,
                            interner,
                            ctx.interner_overlay_ids.as_deref(),
                        ),
                        false,
                    ),
                    AggregateField::All => (None, true),
                };
                let agg = make_aggregate_fn(name, args, ctx)?;
                let agg = if *distinct {
                    agg.map(|a| Box::new(DistinctWrapper::new(a)) as Box<dyn Aggregator>)
                } else {
                    agg
                };
                fn_slots.push((key, field_path, all_field, agg, input.as_ref()));
            }
            SelectItem::Function { name, args, alias } => {
                let key = alias.clone().unwrap_or_else(|| name.clone());
                let fv = FilterValue::FnCall {
                    call: FnCall::complex(name.clone(), args.clone()),
                };
                func_slots.push((key, fv));
            }
            // #1024: a computed SELECT expression inside an aggregate
            // context (e.g. `SELECT price * qty AS total GROUP BY ...`) is
            // evaluated the SAME way as a plain-projection `Expression` item
            // — translate `SelectExpr` to the equivalent `FilterValue::Expr`
            // tree and evaluate it against the group's first record, reusing
            // the SAME `func_slots` pipeline `SelectItem::Function` already
            // populates below. This mirrors the field-projection fallback's
            // "first record represents the group" convention (a computed
            // expression is not itself an aggregate, so it has no per-group
            // accumulation semantics — same rationale `SelectItem::Field`
            // uses here).
            SelectItem::Expression { expr, alias } => {
                let key = alias.clone().unwrap_or_else(|| "expr".to_string());
                func_slots.push((key, expr.to_filter_value()));
            }
            SelectItem::All => {}
        }
    }

    // Fresh per-call caches: never inherit a producer/ForEach snapshot.
    let mut cond_cache = crate::query::filter::CondCache::new();
    let mut field_path_cache = new_map_wc(0);
    let mut query_ref_cache = new_map_wc(0);
    for input in agg_slots
        .iter()
        .filter_map(|slot| slot.2)
        .chain(fn_slots.iter().filter_map(|slot| slot.4))
    {
        prescan_cond_cache(input, interner, &mut cond_cache);
        prescan_field_path_cache(input, interner, &mut field_path_cache);
        prescan_query_ref_cache(input, &mut query_ref_cache);
    }
    let input_ctx = FilterContext::new(interner, ctx.resolved_refs)
        .with_params(ctx.params)
        .with_scalars(ctx.scalars.clone())
        .with_overlay_snapshot(ctx.interner_overlay.clone())
        .with_overlay_ids_snapshot(ctx.interner_overlay_ids.clone())
        .with_cond_cache(&cond_cache)
        .with_field_path_cache(&field_path_cache)
        .with_query_ref_cache(&query_ref_cache);

    // Single walk over the group: build a per-row `RecordRef` (RecordView on
    // the hot path, InnerValue on the bare-scalar fallback) and feed every
    // accumulator + funclib slot at once.
    if !agg_slots.is_empty() || !fn_slots.is_empty() {
        for (_, bytes) in group_records {
            // Per-row lens. RecordView::new fails only for bare-scalar /
            // non-map records (legacy rows) — fall back to InnerValue, which
            // is also a RecordRef. Both arms share the same accumulator
            // surface (scalar_at / materialize_at).
            let view: RowView = match RecordView::new(bytes) {
                Ok(v) => RowView::Lens(v),
                Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                    Ok(iv) => RowView::Tree(iv),
                    // Malformed row — skip defensively (mirrors the read
                    // branches' behaviour).
                    Err(_) => continue,
                },
            };

            for (_, acc, input) in agg_slots.iter_mut() {
                if let Some(input) = input {
                    let value = view.with_ref(|r| resolve_aggregate_input(input, r, &input_ctx))?;
                    step_aggregate_input(acc, &value, interner)?;
                } else {
                    view.with_ref(|r| acc.step(r));
                }
            }
            for (_, path, all_field, agg, input) in fn_slots.iter_mut() {
                if let Some(agg) = agg {
                    if let Some(input) = input {
                        let value =
                            view.with_ref(|r| resolve_aggregate_input(input, r, &input_ctx))?;
                        if !matches!(value, QueryValue::Null) {
                            agg.accumulate(&value)
                                .map_err(aggregate_accumulation_error)?;
                        }
                    } else {
                        let value = if *all_field {
                            Some(view.with_ref(|r| {
                                if ctx.interner_overlay.is_some() {
                                    let mut inner = new_map_wc(0);
                                    r.for_each_field(&mut |key, value| {
                                        inner.insert(key, value);
                                    });
                                    let inner = InnerValue::Map(inner);
                                    ctx.resolve_query_value(&inner)
                                } else {
                                    r.to_query_value(interner)
                                }
                            })?)
                        } else {
                            view.with_ref(|r| fn_value_for_aggregator(r, path, ctx))
                        };
                        if let Some(value) = value {
                            // Preserve legacy field-input error handling.
                            if let Err(error) = agg.accumulate(&value) {
                                if error.code.starts_with("result_too_large: ResultBytes ") {
                                    return Err(aggregate_accumulation_error(error));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    for (key, acc, input) in agg_slots {
        // Literal NULL has no non-NULL contributions. Keep legacy SUM's zero
        // identity unchanged for field inputs and other expression inputs.
        let value = if matches!(input, Some(FilterValue::Null))
            && matches!(&acc.state, AggState::Sum { .. })
        {
            QueryValue::Null
        } else {
            acc.finish(interner)
        };
        obj.insert(key, value);
    }

    for (key, _, _, agg, _) in fn_slots {
        let qv = match agg {
            Some(agg) => match agg.finalize() {
                Ok(v) => v,
                Err(_) => QueryValue::Null,
            },
            None => QueryValue::Null,
        };
        obj.insert(key, qv);
    }

    // Resolve field-projection fallbacks from the first record (rare path).
    for (key, path) in field_slots {
        let val = group_records
            .first()
            .and_then(|(_, bytes)| {
                let keys = intern_field_path_keys_with_overlay(
                    path,
                    interner,
                    ctx.interner_overlay_ids.as_deref(),
                )?;
                match RecordView::new(bytes) {
                    Ok(v) => v
                        .scalar_at(keys.as_slice())
                        .map(scalar_ref_to_query)
                        .or_else(|| {
                            // Container leaf — materialise once.
                            v.materialize_at(keys.as_slice())
                                .and_then(|iv| ctx.resolve_query_value(&iv).ok())
                        }),
                    Err(_) => InnerValue::from_bytes(bytes.as_ref()).ok().and_then(|iv| {
                        iv.scalar_at(keys.as_slice())
                            .map(scalar_ref_to_query)
                            .or_else(|| {
                                iv.materialize_at(keys.as_slice())
                                    .and_then(|v| ctx.resolve_query_value(&v).ok())
                            })
                    }),
                }
            })
            .unwrap_or(QueryValue::Null);
        obj.insert(key, val);
    }

    // Scalar functions in a group SELECT: evaluate against the group's first
    // record (mirrors the field-projection fallback above).
    if !func_slots.is_empty() {
        let mut cond_cache = crate::query::filter::CondCache::new();
        let mut field_path_cache = new_map_wc(0);
        let mut query_ref_cache = new_map_wc(0);
        for (_, fv) in &func_slots {
            prescan_cond_cache(fv, interner, &mut cond_cache);
            prescan_field_path_cache(fv, interner, &mut field_path_cache);
            prescan_query_ref_cache(fv, &mut query_ref_cache);
        }
        let fn_ctx = FilterContext::new(interner, ctx.resolved_refs)
            .with_params(ctx.params)
            .with_scalars(ctx.scalars.clone())
            .with_overlay_snapshot(ctx.interner_overlay.clone())
            .with_overlay_ids_snapshot(ctx.interner_overlay_ids.clone())
            .with_cond_cache(&cond_cache)
            .with_field_path_cache(&field_path_cache)
            .with_query_ref_cache(&query_ref_cache);
        for (key, fv) in func_slots {
            let val = group_records
                .first()
                .and_then(|(_, bytes)| {
                    let view = match RecordView::new(bytes) {
                        Ok(v) => RowView::Lens(v),
                        Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                            Ok(iv) => RowView::Tree(iv),
                            Err(_) => return None,
                        },
                    };
                    view.with_ref(|r| resolve_filter_query(&fv, r, &fn_ctx))
                })
                .unwrap_or(QueryValue::Null);
            obj.insert(key, val);
        }
    }

    Ok(QueryValue::Map(obj))
}

/// Per-row view carrier: a small enum that abstracts over `RecordView` (hot)
/// and `InnerValue` (bare-scalar fallback) so the accumulator loop can be
/// written once. Both arms implement `RecordRef`.
pub(super) enum RowView<'a> {
    Lens(RecordView<'a>),
    Tree(InnerValue),
}

impl<'a> RowView<'a> {
    /// Apply `f` to the underlying `RecordRef`.
    ///
    /// `v: &'a RecordView<'a>` (via match ergonomics on `&'a self`) coerces
    /// to `&'a dyn RecordRef`; `t: &'a InnerValue` coerces likewise.
    /// Both avoid dereferencing to a value type (RecordView is Copy but &T →
    /// &dyn Trait requires a reference; InnerValue is not Copy so `*t` would
    /// move out from behind a reference).
    #[inline]
    pub(super) fn with_ref<R, O>(&'a self, f: R) -> O
    where
        R: FnOnce(&dyn RecordRef) -> O,
    {
        match self {
            RowView::Lens(v) => f(v),
            RowView::Tree(t) => f(t),
        }
    }
}

/// Build the `QueryValue` argument for a funclib aggregator at the funclib
/// boundary only. Scalars are constructed directly from `ScalarRef` (cheap);
/// container leaves (and `AggregateField::All`) materialise ONCE via
/// `materialize_at` → `inner_value_to_query_value`. This is the §5b boundary
/// #1 (funclib ABI takes owned) — there is no whole-record conversion.
#[inline]
fn fn_value_for_aggregator(
    record: &dyn RecordRef,
    path: &Option<Vec<InternerKey>>,
    ctx: &FilterContext<'_>,
) -> Option<QueryValue> {
    let path = path.as_deref()?;
    // Try scalar first (the hot case): no Value materialisation.
    if let Some(s) = record.scalar_at(path) {
        return Some(scalar_ref_to_query(s));
    }
    // Container / Dec / Big leaf — materialise this one field only.
    record
        .materialize_at(path)
        .and_then(|iv| ctx.resolve_query_value(&iv).ok())
}

/// Convert a `ScalarRef` to a `QueryValue` (the only leaf-level conversion;
/// cheap — no interner touch). Used at the funclib feed boundary and for
/// field-projection fallbacks.
#[inline]
pub(super) fn scalar_ref_to_query(s: ScalarRef<'_>) -> QueryValue {
    match s {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(b),
        ScalarRef::Int(i) => QueryValue::Int(i),
        ScalarRef::F64(f) => QueryValue::F64(f),
        ScalarRef::Str(s) => QueryValue::Str(s.into()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.into()),
    }
}
