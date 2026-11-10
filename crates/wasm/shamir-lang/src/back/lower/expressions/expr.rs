use crate::back::ir::{
    cell_tag, ArithOp, CastOp, CmpOp, FloatOp, InstId, InstKind, IrError, ValueKind,
};
use crate::front::{BinaryOp, ExprId, ExprKind, UnaryOp};
use crate::sem::{BuiltinId, ResolvedTarget, TypeData, TypeId};

use super::ctx::LowerCtx;
use super::stmt::{lower_conditional, opens_blocks, reload, short_circuit, spill};
use super::types::{convert, eq_shape_for, is_scalar, to_slot, value_kind};

/// Magnitude of 2^63: the `negative_min_candidate` literal for i64::MIN.
const MAGNITUDE_I64_MIN: u64 = 9_223_372_036_854_775_808;
/// Magnitude of 2^31: the `negative_min_candidate` literal for i32::MIN.
const MAGNITUDE_I32_MIN: u64 = 2_147_483_648;

/// Lowers one expression into the current block; returns the value inst.
pub(crate) fn lower_expr(ctx: &mut LowerCtx, id: ExprId) -> Result<InstId, IrError> {
    let node = ctx
        .module
        .expression(id)
        .ok_or(IrError::Internal("lower: missing expression node"))?;
    let span = node.span;
    match &node.kind {
        ExprKind::Identifier(_) => lower_identifier(ctx, id, span),
        ExprKind::Integer {
            magnitude,
            negative_min_candidate,
            ..
        } => lower_integer(ctx, id, *magnitude, *negative_min_candidate, span),
        ExprKind::Float(value) => ctx
            .builder
            .append(InstKind::ConstF64(value.to_bits()), span),
        ExprKind::String(text) => {
            let offset = ctx.builder.string_literal(text.clone().into_bytes())?;
            ctx.builder.append(InstKind::StaticCell(offset), span)
        }
        ExprKind::Bytes(bytes) => {
            let offset = ctx.builder.bytes_literal(bytes.clone())?;
            ctx.builder.append(InstKind::StaticCell(offset), span)
        }
        ExprKind::Boolean(value) => ctx.builder.append(InstKind::ConstI32(*value as i32), span),
        ExprKind::Null => {
            let offset = ctx.builder.null_cell()?;
            ctx.builder.append(InstKind::StaticCell(offset), span)
        }
        ExprKind::Group(inner) => lower_expr(ctx, *inner),
        ExprKind::List(items) => lower_list(ctx, id, items, span),
        ExprKind::Record(fields) => lower_record(ctx, id, fields, span),
        ExprKind::Unary { op, operand } => lower_unary(ctx, *op, *operand, span),
        ExprKind::Binary { op, left, right } => lower_binary(ctx, id, *op, *left, *right, span),
        ExprKind::Conditional {
            condition,
            then_value,
            else_value,
        } => lower_conditional(ctx, id, *condition, *then_value, *else_value, span),
        ExprKind::Call { arguments, .. } => lower_call(ctx, id, arguments, span),
        ExprKind::Field { target, name } => {
            let base = lower_expr(ctx, *target)?;
            let ty = ctx.expr_type(*target)?;
            field_value(ctx, base, ty, name, span)
        }
        ExprKind::Index { target, index } => lower_index(ctx, *target, *index, span),
    }
}

fn lower_identifier(
    ctx: &mut LowerCtx,
    id: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    match ctx.typed.resolved.get(id.0 as usize).copied().flatten() {
        Some(ResolvedTarget::Parameter(slot)) | Some(ResolvedTarget::Local(slot)) => {
            ctx.builder.append(InstKind::LocalGet(slot), span)
        }
        // Bare function/builtin names are only legal as call targets.
        _ => Err(IrError::Internal("lower: bare identifier is not a value")),
    }
}

fn lower_integer(
    ctx: &mut LowerCtx,
    id: ExprId,
    magnitude: u64,
    negative_min_candidate: bool,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    // The parser guarantees magnitude <= i64::MAX (2^63 for the MIN
    // candidate), so `as i64` is lossless and wrapping_neg of 2^63 is MIN.
    let wide = if negative_min_candidate {
        (magnitude as i64).wrapping_neg()
    } else {
        magnitude as i64
    };
    match ctx.type_data(ctx.expr_type(id)?)? {
        TypeData::I64 => ctx.builder.append(InstKind::ConstI64(wide), span),
        TypeData::I32 => {
            let value = i32::try_from(wide)
                .map_err(|_| IrError::Internal("lower: integer literal out of i32 range"))?;
            ctx.builder.append(InstKind::ConstI32(value), span)
        }
        _ => Err(IrError::Internal(
            "lower: integer literal with non-integer type",
        )),
    }
}

fn lower_list(
    ctx: &mut LowerCtx,
    id: ExprId,
    items: &[ExprId],
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    if !matches!(ctx.type_data(ctx.expr_type(id)?)?, TypeData::List(_)) {
        return Err(IrError::Internal("lower: list literal with non-list type"));
    }
    let count =
        u32::try_from(items.len()).map_err(|_| IrError::Internal("lower: list length overflow"))?;
    let count_i32 = i32::try_from(count)
        .map_err(|_| IrError::Internal("lower: list length out of i32 range"))?;
    let count_inst = ctx.builder.append(InstKind::ConstI32(count_i32), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(count_inst), span)?;
    // An item may open new blocks (`?:`, `&&`, `||`); the CellsAlloc region
    // must then be re-loaded in the block of each consuming CellSlot, and
    // for the final SExtend64 (verifier: same-block operands).
    let any_opens = items.iter().any(|item| opens_blocks(ctx, *item));
    let region_slot = if any_opens {
        Some(spill(ctx, region, ValueKind::I32, span)?)
    } else {
        None
    };
    for (position, item) in items.iter().enumerate() {
        let index = u32::try_from(position)
            .map_err(|_| IrError::Internal("lower: element index overflow"))?;
        let index_i32 = i32::try_from(index)
            .map_err(|_| IrError::Internal("lower: element index out of i32 range"))?;
        let value = lower_expr(ctx, *item)?;
        let boxed = to_slot(ctx, ctx.expr_type(*item)?, value, ctx.expr_span(*item))?;
        let region = match region_slot {
            Some(slot) => reload(ctx, slot, span)?,
            None => region,
        };
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
    let cell = ctx.builder.append(
        InstKind::CellAlloc {
            tag: cell_tag::LIST,
        },
        span,
    )?;
    let aux = ctx.builder.append(InstKind::ConstI32(count_i32), span)?;
    ctx.builder
        .append(InstKind::CellStoreAux { cell, value: aux }, span)?;
    // Addresses are < 2^31, so SExtend64 zero-extends the region offset.
    let region = match region_slot {
        Some(slot) => reload(ctx, slot, span)?,
        None => region,
    };
    let payload = ctx.builder.append(InstKind::SExtend64(region), span)?;
    ctx.builder.append(
        InstKind::CellStorePayload {
            cell,
            value: payload,
        },
        span,
    )?;
    // Explicit value id — LocalSet reads the immediately preceding
    // instruction, and the stores above are value-less: re-yield the cell.
    let yielded = ctx.builder.append(InstKind::CellPass(cell), span)?;
    Ok(yielded)
}

fn lower_record(
    ctx: &mut LowerCtx,
    id: ExprId,
    fields: &[crate::front::ValueFieldId],
    span: crate::api::Span,
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
        let value = lower_expr(ctx, *value_id)?;
        let boxed = to_slot(
            ctx,
            ctx.expr_type(*value_id)?,
            value,
            ctx.expr_span(*value_id),
        )?;
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

fn lower_unary(
    ctx: &mut LowerCtx,
    op: UnaryOp,
    operand: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    match op {
        UnaryOp::Not => {
            let value = lower_expr(ctx, operand)?;
            ctx.builder.append(InstKind::BoolNot(value), span)
        }
        UnaryOp::Positive => lower_expr(ctx, operand),
        UnaryOp::Negate => lower_negate(ctx, operand, span),
    }
}

fn lower_negate(
    ctx: &mut LowerCtx,
    operand: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    let value = lower_expr(ctx, operand)?;
    let data = ctx.type_data(ctx.expr_type(operand)?)?;
    // -MIN literal: constant-fold, the runtime negation would trap.
    let magnitude = ctx.module.expression(operand).and_then(min_candidate);
    match (data, magnitude) {
        (TypeData::I64, Some(MAGNITUDE_I64_MIN)) => {
            ctx.builder.append(InstKind::ConstI64(i64::MIN), span)
        }
        (TypeData::I32, Some(MAGNITUDE_I32_MIN)) => {
            ctx.builder.append(InstKind::ConstI32(i32::MIN), span)
        }
        (TypeData::F64, _) => ctx.builder.append(InstKind::FloatNeg(value), span),
        (TypeData::I32, _) => {
            let zero = ctx.builder.append(InstKind::ConstI32(0), span)?;
            ctx.builder.append(
                InstKind::CheckedArith {
                    op: ArithOp::Sub,
                    width: ValueKind::I32,
                    left: zero,
                    right: value,
                },
                span,
            )
        }
        (TypeData::I64, _) => {
            let zero = ctx.builder.append(InstKind::ConstI64(0), span)?;
            ctx.builder.append(
                InstKind::CheckedArith {
                    op: ArithOp::Sub,
                    width: ValueKind::I64,
                    left: zero,
                    right: value,
                },
                span,
            )
        }
        _ => Err(IrError::Internal("lower: negation of a non-numeric value")),
    }
}

fn min_candidate(node: &crate::front::Expr) -> Option<u64> {
    match &node.kind {
        ExprKind::Integer {
            magnitude,
            negative_min_candidate: true,
            ..
        } => Some(*magnitude),
        _ => None,
    }
}

fn lower_binary(
    ctx: &mut LowerCtx,
    id: ExprId,
    op: BinaryOp,
    left: ExprId,
    right: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    match op {
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Remainder => lower_arith(ctx, op, left, right, span),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
            lower_order(ctx, op, left, right, span)
        }
        BinaryOp::Equal | BinaryOp::NotEqual => lower_equality(ctx, op, id, left, right, span),
        BinaryOp::And => short_circuit(ctx, true, left, right, span),
        BinaryOp::Or => short_circuit(ctx, false, left, right, span),
    }
}

fn lower_arith(
    ctx: &mut LowerCtx,
    op: BinaryOp,
    left: ExprId,
    right: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    let left_inst = lower_expr(ctx, left)?;
    // The right operand may open new blocks; spill the left value so both
    // operands of the consuming instruction share its block.
    let left_slot = if opens_blocks(ctx, right) {
        Some(spill(
            ctx,
            left_inst,
            value_kind(ctx.type_data(ctx.expr_type(left)?)?),
            ctx.expr_span(left),
        )?)
    } else {
        None
    };
    let right_inst = lower_expr(ctx, right)?;
    let left_inst = match left_slot {
        Some(slot) => reload(ctx, slot, span)?,
        None => left_inst,
    };
    let data = ctx.type_data(ctx.expr_type(left)?)?;
    let right_data = ctx.type_data(ctx.expr_type(right)?)?;
    if data != right_data {
        return Err(IrError::Internal("lower: mixed arithmetic operand types"));
    }
    match (data, op) {
        (TypeData::I32, _) => {
            let arith = arith_op(op)?;
            ctx.builder.append(
                InstKind::CheckedArith {
                    op: arith,
                    width: ValueKind::I32,
                    left: left_inst,
                    right: right_inst,
                },
                span,
            )
        }
        (TypeData::I64, _) => {
            let arith = arith_op(op)?;
            ctx.builder.append(
                InstKind::CheckedArith {
                    op: arith,
                    width: ValueKind::I64,
                    left: left_inst,
                    right: right_inst,
                },
                span,
            )
        }
        (TypeData::F64, BinaryOp::Remainder) => {
            Err(IrError::Internal("lower: f64 remainder is not defined"))
        }
        (TypeData::F64, _) => ctx.builder.append(
            InstKind::FloatArith {
                op: float_op(op)?,
                left: left_inst,
                right: right_inst,
            },
            span,
        ),
        _ => Err(IrError::Internal("lower: arithmetic on a non-numeric type")),
    }
}

fn arith_op(op: BinaryOp) -> Result<ArithOp, IrError> {
    match op {
        BinaryOp::Add => Ok(ArithOp::Add),
        BinaryOp::Subtract => Ok(ArithOp::Sub),
        BinaryOp::Multiply => Ok(ArithOp::Mul),
        BinaryOp::Divide => Ok(ArithOp::Div),
        BinaryOp::Remainder => Ok(ArithOp::Rem),
        _ => Err(IrError::Internal("lower: not an arithmetic operator")),
    }
}

fn float_op(op: BinaryOp) -> Result<FloatOp, IrError> {
    match op {
        BinaryOp::Add => Ok(FloatOp::Add),
        BinaryOp::Subtract => Ok(FloatOp::Sub),
        BinaryOp::Multiply => Ok(FloatOp::Mul),
        BinaryOp::Divide => Ok(FloatOp::Div),
        _ => Err(IrError::Internal("lower: not a float operator")),
    }
}

fn cmp_op(op: BinaryOp) -> Result<CmpOp, IrError> {
    match op {
        BinaryOp::Equal => Ok(CmpOp::Eq),
        BinaryOp::NotEqual => Ok(CmpOp::Ne),
        BinaryOp::Less => Ok(CmpOp::Lt),
        BinaryOp::LessEqual => Ok(CmpOp::Le),
        BinaryOp::Greater => Ok(CmpOp::Gt),
        BinaryOp::GreaterEqual => Ok(CmpOp::Ge),
        _ => Err(IrError::Internal("lower: not a comparison operator")),
    }
}

fn lower_equality(
    ctx: &mut LowerCtx,
    op: BinaryOp,
    _id: ExprId,
    left: ExprId,
    right: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    let cmp = cmp_op(op)?;
    let left_data = ctx.type_data(ctx.expr_type(left)?)?;
    let right_data = ctx.type_data(ctx.expr_type(right)?)?;
    if left_data == TypeData::Null && right_data == TypeData::Null {
        // Null == Null is a compile-time truth.
        let value = if cmp == CmpOp::Eq { 1 } else { 0 };
        return ctx.builder.append(InstKind::ConstI32(value), span);
    }
    // Null vs Optional(T): equality means the non-null side is null.
    if left_data == TypeData::Null || right_data == TypeData::Null {
        let side = if left_data == TypeData::Null {
            right
        } else {
            left
        };
        let value = lower_expr(ctx, side)?;
        let is_null = ctx.builder.append(InstKind::CellIsNull(value), span)?;
        // CellIsNull is 1 when equal (null), so flip Eq/Ne.
        return compare_against_zero(ctx, cmp, is_null, span, true);
    }
    if ctx.expr_type(left)? != ctx.expr_type(right)? {
        return Err(IrError::Internal(
            "lower: equality between mismatched types",
        ));
    }
    let left_inst = lower_expr(ctx, left)?;
    // The right operand may open new blocks; spill the left value so both
    // operands of the consuming comparison share its block.
    let left_slot = if opens_blocks(ctx, right) {
        Some(spill(
            ctx,
            left_inst,
            value_kind(left_data),
            ctx.expr_span(left),
        )?)
    } else {
        None
    };
    let right_inst = lower_expr(ctx, right)?;
    let left_inst = match left_slot {
        Some(slot) => reload(ctx, slot, span)?,
        None => left_inst,
    };
    match left_data {
        TypeData::I32 | TypeData::Bool => ctx.builder.append(
            InstKind::Compare {
                op: cmp,
                width: ValueKind::I32,
                left: left_inst,
                right: right_inst,
            },
            span,
        ),
        TypeData::I64 => ctx.builder.append(
            InstKind::Compare {
                op: cmp,
                width: ValueKind::I64,
                left: left_inst,
                right: right_inst,
            },
            span,
        ),
        TypeData::F64 => ctx.builder.append(
            InstKind::Compare {
                op: cmp,
                width: ValueKind::F64,
                left: left_inst,
                right: right_inst,
            },
            span,
        ),
        TypeData::String | TypeData::Bytes => {
            let order = ctx.builder.append(
                InstKind::BytesCompare {
                    left: left_inst,
                    right: right_inst,
                },
                span,
            )?;
            // BytesCompare equality: 0 means equal (keep Eq/Ne unflipped).
            compare_against_zero(ctx, cmp, order, span, false)
        }
        TypeData::Optional(_) | TypeData::List(_) | TypeData::Record(_) => {
            let shape = eq_shape_for(ctx, ctx.expr_type(left)?, 0)?;
            let equal = ctx.builder.append(
                InstKind::StructEq {
                    shape,
                    left: left_inst,
                    right: right_inst,
                },
                span,
            )?;
            // StructEq is 1 when equal (flip Eq/Ne).
            compare_against_zero(ctx, cmp, equal, span, true)
        }
        _ => Err(IrError::Internal("lower: equality on an unsupported type")),
    }
}

/// `Compare { op, I32, value, 0 }` used to normalise -1/0/1 and 0/1 results.
/// `one_means_equal` marks results where 1 already means "equal" (StructEq,
/// CellIsNull): the original Eq/Ne must be flipped. Results where 0 means
/// equal (BytesCompare equality) and ordering results keep the original op.
fn compare_against_zero(
    ctx: &mut LowerCtx,
    cmp: CmpOp,
    value: InstId,
    span: crate::api::Span,
    one_means_equal: bool,
) -> Result<InstId, IrError> {
    let op = match (cmp, one_means_equal) {
        (CmpOp::Eq, true) => CmpOp::Ne,
        (CmpOp::Ne, true) => CmpOp::Eq,
        (op, _) => op,
    };
    let zero = ctx.builder.append(InstKind::ConstI32(0), span)?;
    ctx.builder.append(
        InstKind::Compare {
            op,
            width: ValueKind::I32,
            left: value,
            right: zero,
        },
        span,
    )
}

fn lower_order(
    ctx: &mut LowerCtx,
    op: BinaryOp,
    left: ExprId,
    right: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    let cmp = cmp_op(op)?;
    let data = ctx.type_data(ctx.expr_type(left)?)?;
    if ctx.expr_type(left)? != ctx.expr_type(right)? {
        return Err(IrError::Internal(
            "lower: ordering between mismatched types",
        ));
    }
    let left_inst = lower_expr(ctx, left)?;
    // The right operand may open new blocks; spill the left value so both
    // operands of the Compare share its block.
    let left_slot = if opens_blocks(ctx, right) {
        Some(spill(
            ctx,
            left_inst,
            value_kind(data),
            ctx.expr_span(left),
        )?)
    } else {
        None
    };
    let right_inst = lower_expr(ctx, right)?;
    let left_inst = match left_slot {
        Some(slot) => reload(ctx, slot, span)?,
        None => left_inst,
    };
    let width = match data {
        TypeData::I32 => ValueKind::I32,
        TypeData::I64 => ValueKind::I64,
        TypeData::F64 => ValueKind::F64,
        TypeData::String => {
            let order = ctx.builder.append(
                InstKind::BytesCompare {
                    left: left_inst,
                    right: right_inst,
                },
                span,
            )?;
            return compare_against_zero(ctx, cmp, order, span, false);
        }
        _ => return Err(IrError::Internal("lower: ordering on an unsupported type")),
    };
    ctx.builder.append(
        InstKind::Compare {
            op: cmp,
            width,
            left: left_inst,
            right: right_inst,
        },
        span,
    )
}

fn lower_call(
    ctx: &mut LowerCtx,
    id: ExprId,
    arguments: &[ExprId],
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    let resolved = ctx
        .typed
        .calls
        .get(id.0 as usize)
        .copied()
        .flatten()
        .ok_or(IrError::Internal("lower: missing call resolution"))?;
    match resolved.callee {
        crate::sem::Callee::Function(fid) => {
            let callee = ctx
                .function_map
                .get(&fid.0)
                .copied()
                .ok_or(IrError::Internal(
                    "lower: call to a function outside call order",
                ))?;
            let function = ctx
                .typed
                .function(fid)
                .ok_or(IrError::Internal("lower: missing typed callee"))?;
            if arguments.len() != function.parameters.len() {
                return Err(IrError::Internal("lower: call arity mismatch"));
            }
            let mut args = Vec::with_capacity(arguments.len());
            // Argument k needs a spill iff some LATER argument opens new
            // blocks (`?:`, `&&`, `||`): the verifier requires the Call's
            // operands to live in the block the argument lowering ends in.
            let mut opens_after = Vec::with_capacity(arguments.len());
            let mut later_opens = false;
            for argument in arguments.iter().rev() {
                opens_after.push(later_opens);
                later_opens = later_opens || opens_blocks(ctx, *argument);
            }
            opens_after.reverse();
            // Spill in phase 1 directly after `lower_expr` (because `convert`
            // may pass the id through without appending, a `LocalSet` after
            // `convert` would read the wrong instruction); reload in phase 2
            // because the `Call` and all its operands must share the final
            // block.
            let mut lowered: Vec<(ExprId, TypeId, InstId, Option<u32>)> =
                Vec::with_capacity(arguments.len());
            for (position, argument) in arguments.iter().enumerate() {
                let parameter = function
                    .parameters
                    .get(position)
                    .ok_or(IrError::Internal("lower: missing parameter type"))?;
                let value = lower_expr(ctx, *argument)?;
                let value_type = ctx.expr_type(*argument)?;
                let value_data = ctx.type_data(value_type)?;
                let parameter_data = ctx.type_data(parameter.ty)?;
                if is_scalar(parameter_data) && value_kind(parameter_data) != value_kind(value_data)
                {
                    return Err(IrError::Internal("lower: argument kind mismatch"));
                }
                let slot = if opens_after[position] {
                    Some(spill(
                        ctx,
                        value,
                        value_kind(value_data),
                        ctx.expr_span(*argument),
                    )?)
                } else {
                    None
                };
                lowered.push((*argument, parameter.ty, value, slot));
            }
            for (argument, parameter_ty, raw, slot) in lowered {
                let raw = match slot {
                    Some(s) => reload(ctx, s, span)?,
                    None => raw,
                };
                let converted = convert(
                    ctx,
                    parameter_ty,
                    ctx.expr_type(argument)?,
                    raw,
                    ctx.expr_span(argument),
                )?;
                args.push(converted);
            }
            ctx.builder.append(InstKind::Call { callee, args }, span)
        }
        crate::sem::Callee::Builtin(BuiltinId::CallChecked) => {
            super::call::lower_checked_call(ctx, id, arguments, span)
        }
        crate::sem::Callee::Builtin(builtin) => lower_builtin(ctx, id, builtin, arguments, span),
        _ => Err(IrError::Internal("lower: unresolved callee")),
    }
}

fn lower_builtin(
    ctx: &mut LowerCtx,
    id: ExprId,
    builtin: BuiltinId,
    arguments: &[ExprId],
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    if crate::back::batch::is_batch_builtin(builtin) {
        return crate::back::batch::lower_batch_builtin(ctx, id, builtin, arguments, span);
    }
    if let Some(op) = super::types::db_op_of(builtin) {
        return super::types::lower_db_builtin(ctx, id, op, arguments, span);
    }
    // Validator verdict constructors lower outside the single-argument path.
    // Each argument is spilled to a fresh local right after it is lowered: a
    // later argument may open new blocks (`?:`, `&&`, `||`), and the
    // constructor's operands must live in the block the lowering ends in
    // (the verifier requires same-block, strictly-earlier operands).
    if is_validator_ctor(builtin) {
        let mut slots = Vec::with_capacity(arguments.len());
        for argument in arguments {
            lower_expr(ctx, *argument)?;
            let tmp = ctx.builder.declare_local(ValueKind::I32)?;
            ctx.builder
                .append(InstKind::LocalSet(tmp), ctx.expr_span(*argument))?;
            slots.push(tmp);
        }
        let mut lowered = Vec::with_capacity(slots.len());
        for slot in slots {
            lowered.push(ctx.builder.append(InstKind::LocalGet(slot), span)?);
        }
        return super::types::lower_ctor(ctx, builtin, &lowered, span);
    }
    if arguments.len() != 1 {
        return Err(IrError::Internal("lower: builtin arity mismatch"));
    }
    let argument = arguments
        .first()
        .copied()
        .ok_or(IrError::Internal("lower: builtin arity mismatch"))?;
    let value = lower_expr(ctx, argument)?;
    if builtin == BuiltinId::Unwrap {
        // Unwrap raises on null and yields the cell; load the value out of it
        // per the underlying static type (aggregates keep the pointer).
        let inner = match ctx.type_data(ctx.expr_type(argument)?)? {
            TypeData::Optional(inner) => inner,
            _ => {
                return Err(IrError::Internal("lower: unwrap of a non-optional"));
            }
        };
        let unwrapped = ctx.builder.append(InstKind::CellUnwrap(value), span)?;
        return load_element(ctx, unwrapped, inner, span);
    }
    let kind = match builtin {
        BuiltinId::Len => InstKind::CellLen(value),
        BuiltinId::IsNull => InstKind::CellIsNull(value),
        BuiltinId::ToI64 => InstKind::Cast {
            op: CastOp::ToI64FromI32,
            value,
        },
        BuiltinId::ToF64I32 => InstKind::Cast {
            op: CastOp::ToF64FromI32,
            value,
        },
        BuiltinId::ToF64I64 => InstKind::Cast {
            op: CastOp::ToF64FromI64,
            value,
        },
        BuiltinId::ToI32 => InstKind::Cast {
            op: CastOp::ToI32FromI64,
            value,
        },
        BuiltinId::ToI32F64 => InstKind::Cast {
            op: CastOp::ToI32FromF64,
            value,
        },
        BuiltinId::ToI64F64 => InstKind::Cast {
            op: CastOp::ToI64FromF64,
            value,
        },
        _ => {
            return Err(IrError::Internal(
                "lower: validator/stage-2 builtins are outside the scalar milestone",
            ))
        }
    };
    ctx.builder.append(kind, span)
}

/// True iff `builtin` is one of the seven validator verdict constructors.
fn is_validator_ctor(builtin: BuiltinId) -> bool {
    matches!(
        builtin,
        BuiltinId::Accept
            | BuiltinId::Reject
            | BuiltinId::RejectPath
            | BuiltinId::RecordError
            | BuiltinId::WithError
            | BuiltinId::WithRecordError
            | BuiltinId::Stop
    )
}

/// Field access: `base` is the record (or optional-record) cell pointer.
fn field_value(
    ctx: &mut LowerCtx,
    base: InstId,
    ty: TypeId,
    name: &str,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    match ctx.type_data(ty)? {
        TypeData::Record(index) => {
            let shape = ctx
                .typed
                .types
                .record(index)
                .ok_or(IrError::Internal("lower: missing record shape"))?;
            let field_ty = shape
                .field(name)
                .ok_or(IrError::Internal("lower: unknown record field"))?;
            let position = shape
                .fields
                .iter()
                .position(|(field_name, _)| field_name == name)
                .ok_or(IrError::Internal("lower: unknown record field"))?;
            let index_u32 = u32::try_from(position)
                .map_err(|_| IrError::Internal("lower: field index overflow"))?;
            let index_i32 = i32::try_from(index_u32)
                .map_err(|_| IrError::Internal("lower: field index out of i32 range"))?;
            let index_inst = ctx.builder.append(InstKind::ConstI32(index_i32), span)?;
            let elem = ctx.builder.append(
                InstKind::CellElemPtr {
                    base,
                    index: index_inst,
                },
                span,
            )?;
            load_element(ctx, elem, field_ty, span)
        }
        // Field of an optional record: unwrap raises kind 2 on null.
        TypeData::Optional(inner) => {
            let unwrapped = ctx.builder.append(InstKind::CellUnwrap(base), span)?;
            field_value(ctx, unwrapped, inner, name, span)
        }
        _ => Err(IrError::Internal("lower: field access on a non-record")),
    }
}

/// Loads one container slot as a value of type `ty`: scalars come out of the
/// payload, aggregate slots are used as cell pointers directly.
pub(super) fn load_element(
    ctx: &mut LowerCtx,
    elem: InstId,
    ty: TypeId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    match ctx.type_data(ty)? {
        TypeData::I64 => ctx.builder.append(InstKind::CellLoadPayload(elem), span),
        TypeData::I32 | TypeData::Bool => {
            let payload = ctx.builder.append(InstKind::CellLoadPayload(elem), span)?;
            ctx.builder.append(InstKind::I32WrapI64(payload), span)
        }
        TypeData::F64 => {
            let payload = ctx.builder.append(InstKind::CellLoadPayload(elem), span)?;
            ctx.builder.append(InstKind::F64FromBits(payload), span)
        }
        _ => Ok(elem),
    }
}

fn lower_index(
    ctx: &mut LowerCtx,
    target: ExprId,
    index: ExprId,
    span: crate::api::Span,
) -> Result<InstId, IrError> {
    let mut base = lower_expr(ctx, target)?;
    let mut ty = ctx.expr_type(target)?;
    let elem_ty = loop {
        match ctx.type_data(ty)? {
            TypeData::Optional(inner) => {
                let unwrapped = ctx.builder.append(InstKind::CellUnwrap(base), span)?;
                base = unwrapped;
                ty = inner;
            }
            TypeData::List(elem) => break elem,
            _ => return Err(IrError::Internal("lower: index into a non-list")),
        }
    };
    let index_data = ctx.type_data(ctx.expr_type(index)?)?;
    if index_data != TypeData::I32 {
        return Err(IrError::Internal("lower: list index is not i32"));
    }
    // The index may open new blocks; spill the base cell pointer so both
    // operands of the CellElemPtr share its block.
    let base_slot = if opens_blocks(ctx, index) {
        Some(spill(ctx, base, ValueKind::I32, span)?)
    } else {
        None
    };
    let index_inst = lower_expr(ctx, index)?;
    let base = match base_slot {
        Some(slot) => reload(ctx, slot, span)?,
        None => base,
    };
    let elem = ctx.builder.append(
        InstKind::CellElemPtr {
            base,
            index: index_inst,
        },
        span,
    )?;
    load_element(ctx, elem, elem_ty, span)
}
