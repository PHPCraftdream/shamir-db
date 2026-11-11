//! Lowering of the typed builder intrinsics: `batch`, `batch_tx`, `batch_id`,
//! `batch_read`, `batch_insert`, `batch_update`, `batch_delete`, `request`,
//! `db_execute`, `response_rows`, `response_inserted` and `response_affected`.
//!
//! Every argument is lowered, boxed to a cell pointer and spilled into a
//! fresh local (a later argument may open blocks), then reloaded in the
//! block the lowering ends in. The plan values are the `BatchRequest`
//! record itself, so `request` is the identity and `db_execute` encodes the
//! plan as it is.

use crate::api::Span;
use crate::back::batch::guard_cell;
use crate::back::batch::template::BatchStatics;
use crate::back::ir::{cell_tag, InstId, InstKind, IrError, ValueKind};
use crate::back::lower::{lower_expr, result_node, to_slot, LowerCtx};
use crate::front::ExprId;
use crate::sem::{BuiltinId, TypeData, TypeId};

/// True iff `builtin` belongs to the typed builder family.
pub(crate) fn is_batch_builtin(builtin: BuiltinId) -> bool {
    matches!(
        builtin,
        BuiltinId::Batch
            | BuiltinId::BatchTx
            | BuiltinId::BatchId
            | BuiltinId::BatchRead
            | BuiltinId::BatchInsert
            | BuiltinId::BatchUpdate
            | BuiltinId::BatchDelete
            | BuiltinId::Request
            | BuiltinId::DbExecute
            | BuiltinId::ResponseRows
            | BuiltinId::ResponseInserted
            | BuiltinId::ResponseAffected
    )
}

/// Lowers one typed builder call; the checker guarantees arities and types.
pub(crate) fn lower_batch_builtin(
    ctx: &mut LowerCtx,
    id: ExprId,
    builtin: BuiltinId,
    arguments: &[ExprId],
    span: Span,
) -> Result<InstId, IrError> {
    if ctx.batch_statics.is_none() {
        let statics = BatchStatics::bake(&mut ctx.builder)?;
        ctx.batch_statics = Some(statics);
    }
    let mut slots = Vec::with_capacity(arguments.len());
    let mut types = Vec::with_capacity(arguments.len());
    for &argument in arguments {
        let value = lower_expr(ctx, argument)?;
        let ty = ctx.expr_type(argument)?;
        let argument_span = ctx.expr_span(argument);
        let boxed = to_slot(ctx, ty, value, argument_span)?;
        // LocalSet reads the immediately preceding instruction: re-yield the
        // boxed cell pointer so it is that instruction.
        ctx.builder
            .append(InstKind::CellPass(boxed), argument_span)?;
        let slot = ctx.builder.declare_local(ValueKind::I32)?;
        ctx.builder
            .append(InstKind::LocalSet(slot), argument_span)?;
        slots.push(slot);
        types.push(ty);
    }
    let mut args = Vec::with_capacity(slots.len());
    for slot in slots {
        args.push(ctx.builder.append(InstKind::LocalGet(slot), span)?);
    }
    let arg = |position: usize| -> Result<InstId, IrError> {
        args.get(position)
            .copied()
            .ok_or(IrError::Internal("lower: builder intrinsic arity mismatch"))
    };
    let markers = ctx
        .typed
        .reference_sinks
        .get(id.0 as usize)
        .cloned()
        .unwrap_or_default();
    if markers.iter().any(|marker| marker.transactional) {
        ctx.builder.mark_transactional();
    }
    match builtin {
        BuiltinId::Batch => {
            let batch_cell = statics(ctx)?.batch_cell;
            ctx.builder.append(InstKind::StaticCell(batch_cell), span)
        }
        BuiltinId::BatchTx => {
            ctx.builder.mark_transactional();
            let statics = ctx
                .batch_statics
                .as_mut()
                .ok_or(IrError::Internal("lower: missing builder templates"))?;
            let batch_cell = statics.transactional_batch_cell(&mut ctx.builder)?;
            ctx.builder.append(InstKind::StaticCell(batch_cell), span)
        }
        BuiltinId::BatchId => with_id(ctx, arg(0)?, arg(1)?, span),
        BuiltinId::Request => ctx.builder.append(InstKind::CellPass(arg(0)?), span),
        BuiltinId::DbExecute => ctx
            .builder
            .append(InstKind::DbExecute { request: arg(0)? }, span),
        BuiltinId::ResponseRows | BuiltinId::ResponseInserted => {
            let result_ty = ctx.expr_type(id)?;
            let result = result_node(ctx, result_ty)?;
            ctx.builder.append(
                InstKind::ResponseGet {
                    response: arg(0)?,
                    alias: arg(1)?,
                    result,
                },
                span,
            )
        }
        BuiltinId::BatchRead => {
            let filter_ty = *types
                .get(3)
                .ok_or(IrError::Internal("lower: builder intrinsic arity mismatch"))?;
            let guard = if markers.is_empty() {
                None
            } else {
                let address = guard_cell(&mut ctx.builder, &markers)?;
                let cell = ctx.builder.append(InstKind::StaticCell(address), span)?;
                ctx.builder.mark_reference_guard();
                Some(cell)
            };
            let transactional = markers.iter().any(|marker| marker.transactional);
            let entry = read_entry(ctx, arg(2)?, arg(3)?, filter_ty, guard, transactional, span)?;
            ctx.builder.append(
                InstKind::BatchAppend {
                    batch: arg(0)?,
                    alias: arg(1)?,
                    entry,
                },
                span,
            )
        }
        BuiltinId::BatchInsert => {
            let guard = if markers.is_empty() {
                None
            } else {
                let address = guard_cell(&mut ctx.builder, &markers)?;
                let cell = ctx.builder.append(InstKind::StaticCell(address), span)?;
                ctx.builder.mark_reference_guard();
                Some(cell)
            };
            let transactional = markers.iter().any(|marker| marker.transactional);
            let entry = insert_entry(ctx, arg(2)?, arg(3)?, guard, transactional, span)?;
            ctx.builder.append(
                InstKind::BatchAppend {
                    batch: arg(0)?,
                    alias: arg(1)?,
                    entry,
                },
                span,
            )
        }
        BuiltinId::BatchUpdate => {
            let filter_ty = *types
                .get(3)
                .ok_or(IrError::Internal("lower: builder intrinsic arity mismatch"))?;
            let filter = read_filter(ctx, arg(3)?, filter_ty, span)?.ok_or(IrError::Internal(
                "lower: mutation predicate unexpectedly has no filter",
            ))?;
            let entry = update_entry(ctx, arg(2)?, filter, arg(4)?, span)?;
            ctx.builder.append(
                InstKind::BatchAppend {
                    batch: arg(0)?,
                    alias: arg(1)?,
                    entry,
                },
                span,
            )
        }
        BuiltinId::BatchDelete => {
            let filter_ty = *types
                .get(3)
                .ok_or(IrError::Internal("lower: builder intrinsic arity mismatch"))?;
            let filter = read_filter(ctx, arg(3)?, filter_ty, span)?.ok_or(IrError::Internal(
                "lower: mutation predicate unexpectedly has no filter",
            ))?;
            let entry = delete_entry(ctx, arg(2)?, filter, span)?;
            ctx.builder.append(
                InstKind::BatchAppend {
                    batch: arg(0)?,
                    alias: arg(1)?,
                    entry,
                },
                span,
            )
        }
        BuiltinId::ResponseAffected => ctx.builder.append(
            InstKind::ResponseAffected {
                response: arg(0)?,
                alias: arg(1)?,
            },
            span,
        ),
        _ => Err(IrError::Internal("lower: not a builder intrinsic")),
    }
}

fn statics<'a>(ctx: &'a LowerCtx) -> Result<&'a BatchStatics, IrError> {
    ctx.batch_statics
        .as_ref()
        .ok_or(IrError::Internal("lower: missing builder templates"))
}

fn constant(ctx: &mut LowerCtx, value: i32, span: Span) -> Result<InstId, IrError> {
    ctx.builder.append(InstKind::ConstI32(value), span)
}

fn region_of(ctx: &mut LowerCtx, count: i32, span: Span) -> Result<InstId, IrError> {
    let count_inst = constant(ctx, count, span)?;
    ctx.builder.append(InstKind::CellsAlloc(count_inst), span)
}

/// Copies the cell at `source` into slot `index` of `region`.
fn set_slot(
    ctx: &mut LowerCtx,
    region: InstId,
    index: i32,
    source: InstId,
    span: Span,
) -> Result<(), IrError> {
    let index_inst = constant(ctx, index, span)?;
    let dest = ctx.builder.append(
        InstKind::CellSlot {
            base: region,
            index: index_inst,
        },
        span,
    )?;
    ctx.builder
        .append(InstKind::CellCopyTo { dest, source }, span)?;
    Ok(())
}

fn static_cell(ctx: &mut LowerCtx, address: u32, span: Span) -> Result<InstId, IrError> {
    ctx.builder.append(InstKind::StaticCell(address), span)
}

/// A record cell over `region` with the static `descriptor`.
fn record(
    ctx: &mut LowerCtx,
    region: InstId,
    descriptor: u32,
    fields: u32,
    span: Span,
) -> Result<InstId, IrError> {
    let cell = ctx.builder.append(
        InstKind::RecordCell {
            region,
            descriptor,
            field_count: fields,
        },
        span,
    )?;
    ctx.builder.append(InstKind::CellPass(cell), span)
}

/// A list cell over `region` with `count` elements.
fn list(ctx: &mut LowerCtx, region: InstId, count: i32, span: Span) -> Result<InstId, IrError> {
    let cell = ctx.builder.append(
        InstKind::CellAlloc {
            tag: cell_tag::LIST,
        },
        span,
    )?;
    let aux = constant(ctx, count, span)?;
    ctx.builder
        .append(InstKind::CellStoreAux { cell, value: aux }, span)?;
    let payload = ctx.builder.append(InstKind::SExtend64(region), span)?;
    ctx.builder.append(
        InstKind::CellStorePayload {
            cell,
            value: payload,
        },
        span,
    )?;
    ctx.builder.append(InstKind::CellPass(cell), span)
}

/// `batch_id`: a copy of the six plan cells with slot 0 (`id`) replaced.
fn with_id(ctx: &mut LowerCtx, batch: InstId, id: InstId, span: Span) -> Result<InstId, IrError> {
    let descriptor = statics(ctx)?.request_descriptor;
    let payload = ctx.builder.append(InstKind::CellLoadPayload(batch), span)?;
    let old_region = ctx.builder.append(InstKind::I32WrapI64(payload), span)?;
    let six = constant(ctx, 6, span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(six), span)?;
    ctx.builder.append(
        InstKind::CellsCopy {
            dest: region,
            source: old_region,
            count: six,
        },
        span,
    )?;
    set_slot(ctx, region, 0, id, span)?;
    record(ctx, region, descriptor, 6, span)
}

/// One `eq` filter record over the static field path of `name`.
fn eq_filter(ctx: &mut LowerCtx, name: &str, value: InstId, span: Span) -> Result<InstId, IrError> {
    let eq_op = statics(ctx)?.eq_op;
    let eq_descriptor = statics(ctx)?.eq_descriptor;
    let path = {
        let st = ctx
            .batch_statics
            .as_mut()
            .ok_or(IrError::Internal("lower: missing builder templates"))?;
        st.path(&mut ctx.builder, name)?
    };
    let region = region_of(ctx, 3, span)?;
    let op = static_cell(ctx, eq_op, span)?;
    set_slot(ctx, region, 0, op, span)?;
    let path_cell = static_cell(ctx, path, span)?;
    set_slot(ctx, region, 1, path_cell, span)?;
    set_slot(ctx, region, 2, value, span)?;
    record(ctx, region, eq_descriptor, 3, span)
}

/// The `where` filter of one read: nothing for `null` or an empty record,
/// `eq` on `id` for a scalar, an `eq` (or an `and` of `eq`s) over the sorted
/// fields of a scalar record.
fn read_filter(
    ctx: &mut LowerCtx,
    filter: InstId,
    filter_ty: TypeId,
    span: Span,
) -> Result<Option<InstId>, IrError> {
    match ctx.type_data(filter_ty)? {
        TypeData::Null => Ok(None),
        TypeData::Record(index) => {
            let names: Vec<String> = ctx
                .typed
                .types
                .record(index)
                .ok_or(IrError::Internal("lower: missing record shape"))?
                .fields
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
            if names.is_empty() {
                return Ok(None);
            }
            let payload = ctx
                .builder
                .append(InstKind::CellLoadPayload(filter), span)?;
            let fields = ctx.builder.append(InstKind::I32WrapI64(payload), span)?;
            let mut eqs = Vec::with_capacity(names.len());
            for (position, name) in names.iter().enumerate() {
                let index = i32::try_from(position)
                    .map_err(|_| IrError::Internal("lower: filter field index overflow"))?;
                let index_inst = constant(ctx, index, span)?;
                let value = ctx.builder.append(
                    InstKind::CellSlot {
                        base: fields,
                        index: index_inst,
                    },
                    span,
                )?;
                eqs.push(eq_filter(ctx, name, value, span)?);
            }
            if let [single] = eqs.as_slice() {
                return Ok(Some(*single));
            }
            let count = i32::try_from(eqs.len())
                .map_err(|_| IrError::Internal("lower: filter field count overflow"))?;
            let region = region_of(ctx, count, span)?;
            for (position, eq) in eqs.iter().enumerate() {
                let index = i32::try_from(position)
                    .map_err(|_| IrError::Internal("lower: filter field index overflow"))?;
                set_slot(ctx, region, index, *eq, span)?;
            }
            let filters = list(ctx, region, count, span)?;
            let and_op = statics(ctx)?.and_op;
            let and_descriptor = statics(ctx)?.and_descriptor;
            let and_region = region_of(ctx, 2, span)?;
            let op = static_cell(ctx, and_op, span)?;
            set_slot(ctx, and_region, 0, op, span)?;
            set_slot(ctx, and_region, 1, filters, span)?;
            Ok(Some(record(ctx, and_region, and_descriptor, 2, span)?))
        }
        // a scalar key selects by the default `id` field
        _ => Ok(Some(eq_filter(ctx, "id", filter, span)?)),
    }
}

/// The read entry record: `from`, `select`, optional `where`, `return_result`.
fn read_entry(
    ctx: &mut LowerCtx,
    table: InstId,
    filter: InstId,
    filter_ty: TypeId,
    when: Option<InstId>,
    transactional: bool,
    span: Span,
) -> Result<InstId, IrError> {
    let select_cell = statics(ctx)?.select_cell;
    let true_cell = statics(ctx)?.true_cell;
    let read_descriptor = statics(ctx)?.read_descriptor;
    let read_where_descriptor = statics(ctx)?.read_where_descriptor;
    let where_filter = read_filter(ctx, filter, filter_ty, span)?;
    if let Some(when_cell) = when {
        let where_filter =
            where_filter.ok_or(IrError::Internal("lower: guarded read has no where filter"))?;
        let (descriptor, fields, abort_cell) = if transactional {
            let statics = ctx
                .batch_statics
                .as_mut()
                .ok_or(IrError::Internal("lower: missing builder templates"))?;
            let (descriptor, _) = statics.transactional_guarded_descriptors(&mut ctx.builder)?;
            let abort_cell = statics.transactional_true_cell(&mut ctx.builder)?;
            (descriptor, 6, Some(abort_cell))
        } else {
            let descriptor = ctx
                .batch_statics
                .as_mut()
                .ok_or(IrError::Internal("lower: missing builder templates"))?
                .guarded_descriptors(&mut ctx.builder)?
                .0;
            (descriptor, 5, None)
        };
        let region = region_of(ctx, fields, span)?;
        set_slot(ctx, region, 0, table, span)?;
        let select = static_cell(ctx, select_cell, span)?;
        set_slot(ctx, region, 1, select, span)?;
        set_slot(ctx, region, 2, where_filter, span)?;
        let flag = static_cell(ctx, true_cell, span)?;
        set_slot(ctx, region, 3, flag, span)?;
        set_slot(ctx, region, 4, when_cell, span)?;
        if let Some(abort_cell) = abort_cell {
            let abort_flag = static_cell(ctx, abort_cell, span)?;
            set_slot(ctx, region, 5, abort_flag, span)?;
        }
        return record(ctx, region, descriptor, fields as u32, span);
    }
    let fields = if where_filter.is_some() { 4 } else { 3 };
    let region = region_of(ctx, fields, span)?;
    set_slot(ctx, region, 0, table, span)?;
    let select = static_cell(ctx, select_cell, span)?;
    set_slot(ctx, region, 1, select, span)?;
    let mut next = 2;
    let descriptor = match where_filter {
        Some(filter_cell) => {
            set_slot(ctx, region, next, filter_cell, span)?;
            next += 1;
            read_where_descriptor
        }
        None => read_descriptor,
    };
    let flag = static_cell(ctx, true_cell, span)?;
    set_slot(ctx, region, next, flag, span)?;
    record(ctx, region, descriptor, fields as u32, span)
}

/// Update entry: `update`, `where`, `set`, `return_result`.
fn update_entry(
    ctx: &mut LowerCtx,
    table: InstId,
    filter: InstId,
    set: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let descriptor = ctx
        .builder
        .record_descriptor(&["update", "where", "set", "return_result"].map(str::to_string))?;
    let region = region_of(ctx, 4, span)?;
    set_slot(ctx, region, 0, table, span)?;
    set_slot(ctx, region, 1, filter, span)?;
    set_slot(ctx, region, 2, set, span)?;
    let flag = static_cell(ctx, statics(ctx)?.true_cell, span)?;
    set_slot(ctx, region, 3, flag, span)?;
    record(ctx, region, descriptor, 4, span)
}

fn delete_entry(
    ctx: &mut LowerCtx,
    table: InstId,
    filter: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let descriptor = ctx
        .builder
        .record_descriptor(&["delete_from", "where", "return_result"].map(str::to_string))?;
    let region = region_of(ctx, 3, span)?;
    set_slot(ctx, region, 0, table, span)?;
    set_slot(ctx, region, 1, filter, span)?;
    let flag = static_cell(ctx, statics(ctx)?.true_cell, span)?;
    set_slot(ctx, region, 2, flag, span)?;
    record(ctx, region, descriptor, 3, span)
}

/// The insert entry record: `insert_into`, a one-element `values`,
/// `return_result`.
fn insert_entry(
    ctx: &mut LowerCtx,
    table: InstId,
    document: InstId,
    when: Option<InstId>,
    transactional: bool,
    span: Span,
) -> Result<InstId, IrError> {
    let true_cell = statics(ctx)?.true_cell;
    let insert_descriptor = statics(ctx)?.insert_descriptor;
    if let Some(when_cell) = when {
        let (descriptor, fields, abort_cell) = if transactional {
            let statics = ctx
                .batch_statics
                .as_mut()
                .ok_or(IrError::Internal("lower: missing builder templates"))?;
            let (_, descriptor) = statics.transactional_guarded_descriptors(&mut ctx.builder)?;
            let abort_cell = statics.transactional_true_cell(&mut ctx.builder)?;
            (descriptor, 5, Some(abort_cell))
        } else {
            let descriptor = ctx
                .batch_statics
                .as_mut()
                .ok_or(IrError::Internal("lower: missing builder templates"))?
                .guarded_descriptors(&mut ctx.builder)?
                .1;
            (descriptor, 4, None)
        };
        let abort_flag = match abort_cell {
            Some(cell) => Some(static_cell(ctx, cell, span)?),
            None => None,
        };
        let values_region = region_of(ctx, 1, span)?;
        set_slot(ctx, values_region, 0, document, span)?;
        let values = list(ctx, values_region, 1, span)?;
        let region = region_of(ctx, fields, span)?;
        set_slot(ctx, region, 0, table, span)?;
        set_slot(ctx, region, 1, values, span)?;
        let flag = static_cell(ctx, true_cell, span)?;
        set_slot(ctx, region, 2, flag, span)?;
        set_slot(ctx, region, 3, when_cell, span)?;
        if let Some(abort_flag) = abort_flag {
            set_slot(ctx, region, 4, abort_flag, span)?;
        }
        return record(ctx, region, descriptor, fields as u32, span);
    }
    let values_region = region_of(ctx, 1, span)?;
    set_slot(ctx, values_region, 0, document, span)?;
    let values = list(ctx, values_region, 1, span)?;
    let region = region_of(ctx, 3, span)?;
    set_slot(ctx, region, 0, table, span)?;
    set_slot(ctx, region, 1, values, span)?;
    let flag = static_cell(ctx, true_cell, span)?;
    set_slot(ctx, region, 2, flag, span)?;
    record(ctx, region, insert_descriptor, 3, span)
}
