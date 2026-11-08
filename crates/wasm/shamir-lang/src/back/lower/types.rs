use crate::api::Span;
use crate::back::ir::{EqShape, InstId, InstKind, IrError, ValueKind};
use crate::sem::{TypeData, TypeId};

use super::ctx::LowerCtx;

/// The Wasm value kind of one interned type: scalars lower to raw values,
/// bool to 0/1, and every aggregate/optional to an I32 cell pointer.
pub(crate) fn value_kind(data: TypeData) -> ValueKind {
    match data {
        TypeData::I32 => ValueKind::I32,
        TypeData::I64 => ValueKind::I64,
        TypeData::F64 => ValueKind::F64,
        _ => ValueKind::I32,
    }
}

/// True iff the type lowers to a raw scalar value (never a cell pointer).
pub(crate) fn is_scalar(data: TypeData) -> bool {
    matches!(
        data,
        TypeData::I32 | TypeData::I64 | TypeData::F64 | TypeData::Bool
    )
}

/// THE representation rule: boxes a value of type `ty` into a 16-byte cell
/// when `ty` is scalar-typed (directly or through an optional), and passes
/// aggregate/optional cell pointers through unchanged.
pub(crate) fn box_value(
    ctx: &mut LowerCtx,
    ty: TypeId,
    value: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    match ctx.type_data(ty)? {
        TypeData::I32 => {
            let wide = ctx.builder.append(InstKind::SExtend64(value), span)?;
            ctx.builder.append(InstKind::CellAllocI64(wide), span)
        }
        TypeData::I64 => ctx.builder.append(InstKind::CellAllocI64(value), span),
        TypeData::Bool => ctx.builder.append(InstKind::CellAllocBool(value), span),
        TypeData::F64 => ctx.builder.append(InstKind::CellAllocF64(value), span),
        TypeData::Optional(inner) => box_value(ctx, inner, value, span),
        _ => Ok(value),
    }
}

/// Converts a lowered value to a declared target type, decided by the VALUE
/// and TARGET static types: scalar->scalar passes through unboxed (the kinds
/// must already match), scalar->aggregate/optional boxes the value, and
/// cell pointers pass through between non-scalar types.
pub(crate) fn convert(
    ctx: &mut LowerCtx,
    target_ty: TypeId,
    value_ty: TypeId,
    value: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let target_data = ctx.type_data(target_ty)?;
    let value_data = ctx.type_data(value_ty)?;
    if is_scalar(value_data) {
        if is_scalar(target_data) {
            if value_kind(target_data) != value_kind(value_data) {
                return Err(IrError::Internal("lower: value kind mismatch"));
            }
            Ok(value)
        } else {
            box_value(ctx, target_ty, value, span)
        }
    } else if is_scalar(target_data) {
        // M3 typing makes a cell pointer for a scalar target impossible.
        Err(IrError::Internal("lower: cell value for a scalar target"))
    } else {
        Ok(value)
    }
}

/// Interns the equality shape of `ty`, bottom-up and depth-guarded.
pub(crate) fn eq_shape_for(ctx: &mut LowerCtx, ty: TypeId, depth: u32) -> Result<u32, IrError> {
    if depth > ctx.limits.max_type_depth as u32 {
        return Err(IrError::Internal(
            "lower: equality shape exceeds type depth",
        ));
    }
    let shape = match ctx.type_data(ty)? {
        TypeData::I32 => EqShape::I32,
        TypeData::I64 => EqShape::I64,
        TypeData::F64 => EqShape::F64,
        TypeData::Bool => EqShape::Bool,
        TypeData::String => EqShape::String,
        TypeData::Bytes => EqShape::Bytes,
        TypeData::Null => EqShape::Null,
        TypeData::Optional(inner) => EqShape::Optional(eq_shape_for(ctx, inner, depth + 1)?),
        TypeData::List(inner) => EqShape::List(eq_shape_for(ctx, inner, depth + 1)?),
        TypeData::Record(index) => {
            let record = ctx
                .typed
                .types
                .record(index)
                .ok_or(IrError::Internal("lower: missing record shape"))?;
            let mut children = Vec::with_capacity(record.fields.len());
            for (_, field_ty) in &record.fields {
                children.push(eq_shape_for(ctx, *field_ty, depth + 1)?);
            }
            EqShape::Record(children)
        }
        _ => return Err(IrError::Internal("lower: type has no equality shape")),
    };
    ctx.builder.eq_shape(shape)
}
