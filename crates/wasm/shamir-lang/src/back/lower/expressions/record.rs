use crate::api::Span;
use crate::back::batch::marker_cell;
use crate::back::ir::{InstId, InstKind, IrError, ValueKind};
use crate::front::{ExprId, ValueFieldId};
use crate::sem::TypeData;

use super::super::ctx::LowerCtx;
use super::super::expr::lower_expr;
use super::super::stmt::{opens_blocks, reload, spill};
use super::super::types::to_slot;

pub(in crate::back::lower) fn lower_record(
    ctx: &mut LowerCtx,
    id: ExprId,
    fields: &[ValueFieldId],
    span: Span,
) -> Result<InstId, IrError> {
    let shape_index = match ctx.type_data(ctx.expr_type(id)?)? {
        TypeData::Record(index) => index,
        _ => {
            return Err(IrError::Internal(
                "lower: record literal with non-record type",
            ))
        }
    };
    let shape = ctx
        .typed
        .types
        .record(shape_index)
        .ok_or(IrError::Internal("lower: missing record shape"))?;
    let field_count = u32::try_from(shape.fields.len())
        .map_err(|_| IrError::Internal("lower: record field count overflow"))?;
    let field_count_i32 = i32::try_from(field_count)
        .map_err(|_| IrError::Internal("lower: record field count out of i32 range"))?;
    // The region is allocated once; fields are filled in canonical
    // (sorted-name) order regardless of literal order.
    let count_inst = ctx
        .builder
        .append(InstKind::ConstI32(field_count_i32), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(count_inst), span)?;
    // Resolve each shape field to its literal value expression first, so a
    // block-opening field (`?:`, `&&`, `||`) can be detected before the loop.
    let field_values: Vec<ExprId> = shape
        .fields
        .iter()
        .map(|(name, _)| {
            fields
                .iter()
                .find_map(|field_id| {
                    let node = ctx.module.value_field(*field_id)?;
                    if &node.name == name {
                        Some(node.value)
                    } else {
                        None
                    }
                })
                .ok_or(IrError::Internal("lower: record literal misses a field"))
        })
        .collect::<Result<Vec<ExprId>, IrError>>()?;
    // A field may open new blocks; the CellsAlloc region must then be
    // re-loaded in the block of each consuming CellSlot, and for the final
    // RecordCell (verifier: same-block operands).
    let any_opens = field_values.iter().any(|value| opens_blocks(ctx, *value));
    let region_slot = if any_opens {
        Some(spill(ctx, region, ValueKind::I32, span)?)
    } else {
        None
    };
    for (position, value_id) in field_values.iter().enumerate() {
        let boxed = if let Some(marker) = ctx
            .typed
            .reference_markers
            .get(value_id.0 as usize)
            .and_then(Option::as_ref)
        {
            let address = marker_cell(&mut ctx.builder, &marker.alias, &marker.path)?;
            ctx.builder
                .append(InstKind::StaticCell(address), ctx.expr_span(*value_id))?
        } else {
            let value = lower_expr(ctx, *value_id)?;
            to_slot(
                ctx,
                ctx.expr_type(*value_id)?,
                value,
                ctx.expr_span(*value_id),
            )?
        };
        let region = match region_slot {
            Some(slot) => reload(ctx, slot, span)?,
            None => region,
        };
        let index = u32::try_from(position)
            .map_err(|_| IrError::Internal("lower: field index overflow"))?;
        let index_i32 = i32::try_from(index)
            .map_err(|_| IrError::Internal("lower: field index out of i32 range"))?;
        let index_inst = ctx.builder.append(InstKind::ConstI32(index_i32), span)?;
        let dest = ctx.builder.append(
            InstKind::CellSlot {
                base: region,
                index: index_inst,
            },
            span,
        )?;
        ctx.builder.append(
            InstKind::CellCopyTo {
                dest,
                source: boxed,
            },
            span,
        )?;
    }
    let descriptor = match ctx.record_descriptors.get(&shape_index) {
        Some(offset) => *offset,
        None => {
            let names: Vec<String> = shape.fields.iter().map(|(name, _)| name.clone()).collect();
            let offset = ctx.builder.record_descriptor(&names)?;
            ctx.record_descriptors.insert(shape_index, offset);
            offset
        }
    };
    let region = match region_slot {
        Some(slot) => reload(ctx, slot, span)?,
        None => region,
    };
    ctx.builder.append(
        InstKind::RecordCell {
            region,
            descriptor,
            field_count,
        },
        span,
    )
}
