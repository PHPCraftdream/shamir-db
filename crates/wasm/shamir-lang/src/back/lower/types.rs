use crate::api::Span;
use crate::back::ir::{
    cell_tag, ArithOp, DbOp, EqShape, InstId, InstKind, IrError, RecordField, TypeNode,
    TypeNodeKind, ValueKind,
};
use crate::front::ExprId;
use crate::sem::{BuiltinId, TypeData, TypeId, TypedModule};

use super::ctx::LowerCtx;
use super::expr::lower_expr;

/// The documented compiler cap on accumulated validation errors; appending
/// beyond it raises error kind 2 (never a trap or unbounded growth).
pub(crate) const VALIDATION_ERROR_CAP: u32 = 64;

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

/// Prepares a value for a list element or record field slot: every slot
/// holds a 16-byte cell, so a scalar is boxed by its own type and a cell
/// pointer (aggregate, optional or null) passes through unchanged.
pub(crate) fn to_slot(
    ctx: &mut LowerCtx,
    value_ty: TypeId,
    value: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    if is_scalar(ctx.type_data(value_ty)?) {
        box_value(ctx, value_ty, value, span)
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

/// Lowers one of the seven validator verdict constructors. `args` are the
/// ALREADY-LOWERCED argument values in source order (strings and lists are
/// cell pointers; they are not re-boxed); the checker guarantees arities, so
/// a mismatch is an internal invariant.
pub(crate) fn lower_ctor(
    ctx: &mut LowerCtx,
    builtin: BuiltinId,
    args: &[InstId],
    span: Span,
) -> Result<InstId, IrError> {
    match builtin {
        BuiltinId::Accept => {
            expect_arity(args, 0)?;
            // The empty errors list carries count 0 and a zero payload; the
            // encoder never dereferences a zero-count list, and a later
            // append copies zero cells into a fresh region.
            let empty = ctx.builder.append(
                InstKind::CellAlloc {
                    tag: cell_tag::LIST,
                },
                span,
            )?;
            let zero = ctx.builder.append(InstKind::ConstI32(0), span)?;
            ctx.builder.append(
                InstKind::CellStoreAux {
                    cell: empty,
                    value: zero,
                },
                span,
            )?;
            let zero_payload = ctx.builder.append(InstKind::ConstI64(0), span)?;
            ctx.builder.append(
                InstKind::CellStorePayload {
                    cell: empty,
                    value: zero_payload,
                },
                span,
            )?;
            let list = ctx.builder.append(InstKind::CellPass(empty), span)?;
            let stop = stop_cell(ctx, false, span)?;
            validation_cell(ctx, list, stop, span)
        }
        BuiltinId::Reject => {
            expect_arity(args, 2)?;
            let field = *args.first().ok_or_else(arity_missing)?;
            let code = *args.get(1).ok_or_else(arity_missing)?;
            let path = single_string_list(ctx, field, span)?;
            let error = field_error_cell(ctx, code, path, span)?;
            let errors = errors_list_single(ctx, error, span)?;
            let stop = stop_cell(ctx, false, span)?;
            validation_cell(ctx, errors, stop, span)
        }
        BuiltinId::RejectPath => {
            expect_arity(args, 2)?;
            let path = *args.first().ok_or_else(arity_missing)?;
            let code = *args.get(1).ok_or_else(arity_missing)?;
            let error = field_error_cell(ctx, code, path, span)?;
            let errors = errors_list_single(ctx, error, span)?;
            let stop = stop_cell(ctx, false, span)?;
            validation_cell(ctx, errors, stop, span)
        }
        BuiltinId::RecordError => {
            expect_arity(args, 1)?;
            let code = *args.first().ok_or_else(arity_missing)?;
            let error = code_error_cell(ctx, code, span)?;
            let errors = errors_list_single(ctx, error, span)?;
            let stop = stop_cell(ctx, false, span)?;
            validation_cell(ctx, errors, stop, span)
        }
        BuiltinId::WithError => {
            expect_arity(args, 3)?;
            let value = *args.first().ok_or_else(arity_missing)?;
            let field = *args.get(1).ok_or_else(arity_missing)?;
            let code = *args.get(2).ok_or_else(arity_missing)?;
            let path = single_string_list(ctx, field, span)?;
            let error = field_error_cell(ctx, code, path, span)?;
            append_error(ctx, value, error, span)
        }
        BuiltinId::WithRecordError => {
            expect_arity(args, 2)?;
            let value = *args.first().ok_or_else(arity_missing)?;
            let code = *args.get(1).ok_or_else(arity_missing)?;
            let error = code_error_cell(ctx, code, span)?;
            append_error(ctx, value, error, span)
        }
        BuiltinId::Stop => {
            expect_arity(args, 1)?;
            let value = *args.first().ok_or_else(arity_missing)?;
            let payload = ctx.builder.append(InstKind::CellLoadPayload(value), span)?;
            let region_v = ctx.builder.append(InstKind::I32WrapI64(payload), span)?;
            let old_list = slot(ctx, region_v, 0, span)?;
            let new_stop = stop_cell(ctx, true, span)?;
            validation_cell(ctx, old_list, new_stop, span)
        }
        _ => Err(IrError::Internal("lower: validator constructor arity")),
    }
}

/// Internal-invariant error for an impossible constructor arity.
fn arity_missing() -> IrError {
    IrError::Internal("lower: validator constructor arity")
}

/// Rejects an impossible constructor arity (checker-guaranteed).
fn expect_arity(args: &[InstId], expected: usize) -> Result<(), IrError> {
    if args.len() != expected {
        return Err(IrError::Internal("lower: validator constructor arity"));
    }
    Ok(())
}

/// Unchecked 16-byte slot address inside `base` at the compile-time `index`.
fn slot(ctx: &mut LowerCtx, base: InstId, index: i32, span: Span) -> Result<InstId, IrError> {
    let index_inst = ctx.builder.append(InstKind::ConstI32(index), span)?;
    ctx.builder.append(
        InstKind::CellSlot {
            base,
            index: index_inst,
        },
        span,
    )
}

/// The cached absolute validation-record descriptor (fields `errors`, `stop`).
fn validation_descriptor(ctx: &mut LowerCtx) -> Result<u32, IrError> {
    if let Some(offset) = ctx.validation_descriptor {
        return Ok(offset);
    }
    let offset = ctx
        .builder
        .record_descriptor(&["errors".to_string(), "stop".to_string()])?;
    ctx.validation_descriptor = Some(offset);
    Ok(offset)
}

/// The cached absolute field-error descriptor (fields `code`, `field`).
fn field_error_descriptor(ctx: &mut LowerCtx) -> Result<u32, IrError> {
    if let Some(offset) = ctx.field_error_descriptor {
        return Ok(offset);
    }
    let offset = ctx
        .builder
        .record_descriptor(&["code".to_string(), "field".to_string()])?;
    ctx.field_error_descriptor = Some(offset);
    Ok(offset)
}

/// The cached absolute code-error descriptor (field `code`).
fn code_error_descriptor(ctx: &mut LowerCtx) -> Result<u32, IrError> {
    if let Some(offset) = ctx.code_error_descriptor {
        return Ok(offset);
    }
    let offset = ctx.builder.record_descriptor(&["code".to_string()])?;
    ctx.code_error_descriptor = Some(offset);
    Ok(offset)
}

/// The cached static LIST cell whose aux is `VALIDATION_ERROR_CAP`; used
/// solely as the bounds-check base for error appends.
fn error_cap_cell(ctx: &mut LowerCtx) -> Result<u32, IrError> {
    if let Some(offset) = ctx.error_cap_cell {
        return Ok(offset);
    }
    let offset = ctx.builder.list_cap_cell(VALIDATION_ERROR_CAP)?;
    ctx.error_cap_cell = Some(offset);
    Ok(offset)
}

/// A one-error list cell over a fresh ONE-cell region holding `error_cell`;
/// appends never grow in place, so a single slot suffices.
fn errors_list_single(
    ctx: &mut LowerCtx,
    error_cell: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let one = ctx.builder.append(InstKind::ConstI32(1), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(one), span)?;
    let dest = slot(ctx, region, 0, span)?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest,
            source: error_cell,
        },
        span,
    )?;
    list_cell(ctx, region, 1, span)
}

/// A bool cell with payload 0/1 (tag 2) holding `value`.
fn stop_cell(ctx: &mut LowerCtx, value: bool, span: Span) -> Result<InstId, IrError> {
    let bit = ctx.builder.append(InstKind::ConstI32(value as i32), span)?;
    ctx.builder.append(InstKind::CellAllocBool(bit), span)
}

/// A list cell over `region` with aux = `count` and payload = region.
fn list_cell(
    ctx: &mut LowerCtx,
    region: InstId,
    count: i32,
    span: Span,
) -> Result<InstId, IrError> {
    let cell = ctx.builder.append(
        InstKind::CellAlloc {
            tag: cell_tag::LIST,
        },
        span,
    )?;
    let aux = ctx.builder.append(InstKind::ConstI32(count), span)?;
    ctx.builder
        .append(InstKind::CellStoreAux { cell, value: aux }, span)?;
    // Addresses are < 2^31, so SExtend64 zero-extends the region offset.
    let payload = ctx.builder.append(InstKind::SExtend64(region), span)?;
    ctx.builder.append(
        InstKind::CellStorePayload {
            cell,
            value: payload,
        },
        span,
    )?;
    // Explicit value id — the stores above are value-less: re-yield the cell.
    ctx.builder.append(InstKind::CellPass(cell), span)
}

/// A typed record cell over `region` with `field_count` slots and the cached
/// `descriptor`; re-yielded through `CellPass`.
fn record_cell(
    ctx: &mut LowerCtx,
    region: InstId,
    descriptor: u32,
    field_count: u32,
    span: Span,
) -> Result<InstId, IrError> {
    let cell = ctx.builder.append(
        InstKind::RecordCell {
            region,
            descriptor,
            field_count,
        },
        span,
    )?;
    ctx.builder.append(InstKind::CellPass(cell), span)
}

/// The 2-slot `Validation` record: slot 0 errors list, slot 1 stop bool.
fn validation_cell(
    ctx: &mut LowerCtx,
    errors: InstId,
    stop: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let two = ctx.builder.append(InstKind::ConstI32(2), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(two), span)?;
    let errors_dest = slot(ctx, region, 0, span)?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest: errors_dest,
            source: errors,
        },
        span,
    )?;
    let stop_dest = slot(ctx, region, 1, span)?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest: stop_dest,
            source: stop,
        },
        span,
    )?;
    let descriptor = validation_descriptor(ctx)?;
    record_cell(ctx, region, descriptor, 2, span)
}

/// The 2-slot field-error record: slot 0 code string, slot 1 field list.
fn field_error_cell(
    ctx: &mut LowerCtx,
    code: InstId,
    field: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let two = ctx.builder.append(InstKind::ConstI32(2), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(two), span)?;
    let code_dest = slot(ctx, region, 0, span)?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest: code_dest,
            source: code,
        },
        span,
    )?;
    let field_dest = slot(ctx, region, 1, span)?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest: field_dest,
            source: field,
        },
        span,
    )?;
    let descriptor = field_error_descriptor(ctx)?;
    record_cell(ctx, region, descriptor, 2, span)
}

/// The 1-slot record-level error record: slot 0 code string.
fn code_error_cell(ctx: &mut LowerCtx, code: InstId, span: Span) -> Result<InstId, IrError> {
    let one = ctx.builder.append(InstKind::ConstI32(1), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(one), span)?;
    let code_dest = slot(ctx, region, 0, span)?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest: code_dest,
            source: code,
        },
        span,
    )?;
    let descriptor = code_error_descriptor(ctx)?;
    record_cell(ctx, region, descriptor, 1, span)
}

/// A one-element list of strings holding `item`.
fn single_string_list(ctx: &mut LowerCtx, item: InstId, span: Span) -> Result<InstId, IrError> {
    let one = ctx.builder.append(InstKind::ConstI32(1), span)?;
    let region = ctx.builder.append(InstKind::CellsAlloc(one), span)?;
    let dest = slot(ctx, region, 0, span)?;
    ctx.builder
        .append(InstKind::CellCopyTo { dest, source: item }, span)?;
    list_cell(ctx, region, 1, span)
}

/// Returns `old_validation` with `error_cell` appended. A FRESH region of
/// `count + 1` cells is allocated, the old cells are copied with one
/// `CellsCopy` (the M1 runtime copy helper), and the new error is written at
/// index `count`; the input value is never mutated.
fn append_error(
    ctx: &mut LowerCtx,
    old_validation: InstId,
    error_cell: InstId,
    span: Span,
) -> Result<InstId, IrError> {
    let payload = ctx
        .builder
        .append(InstKind::CellLoadPayload(old_validation), span)?;
    let region_v = ctx.builder.append(InstKind::I32WrapI64(payload), span)?;
    let old_list = slot(ctx, region_v, 0, span)?;
    let old_stop = slot(ctx, region_v, 1, span)?;
    let count = ctx.builder.append(InstKind::CellLen(old_list), span)?;
    let list_payload = ctx
        .builder
        .append(InstKind::CellLoadPayload(old_list), span)?;
    let region_list = ctx
        .builder
        .append(InstKind::I32WrapI64(list_payload), span)?;
    // Bounds check via the static cap cell: `CellElemPtr` raises kind 2
    // ("bound") exactly when count >= VALIDATION_ERROR_CAP, before the
    // (meaningless) element pointer is computed and dropped.
    let cap_addr = error_cap_cell(ctx)?;
    let cap = ctx.builder.append(InstKind::StaticCell(cap_addr), span)?;
    let elem = ctx.builder.append(
        InstKind::CellElemPtr {
            base: cap,
            index: count,
        },
        span,
    )?;
    ctx.builder.append(InstKind::Drop(elem), span)?;
    let one = ctx.builder.append(InstKind::ConstI32(1), span)?;
    let new_count = ctx.builder.append(
        InstKind::CheckedArith {
            op: ArithOp::Add,
            width: ValueKind::I32,
            left: count,
            right: one,
        },
        span,
    )?;
    // The fresh region holds exactly the grown list (`count + 1` cells).
    let new_region = ctx.builder.append(InstKind::CellsAlloc(new_count), span)?;
    // Copy-on-append: the runtime copy helper (memory.copy of count * 16
    // bytes) duplicates the old cells, so the input Validation value is never
    // mutated (design: records/lists and all aggregate descendants are
    // immutable). A zero count is a zero-length memory.copy — a no-op — which
    // is how appends onto the empty `accept()` list (payload 0) work.
    ctx.builder.append(
        InstKind::CellsCopy {
            dest: new_region,
            source: region_list,
            count,
        },
        span,
    )?;
    // Runtime index `count` is proven in range: the cap check above raised
    // kind 2 when count >= VALIDATION_ERROR_CAP, and the fresh region has
    // `count + 1 >= count + 1` slots, so index `count` is in range.
    let dest = ctx.builder.append(
        InstKind::CellSlot {
            base: new_region,
            index: count,
        },
        span,
    )?;
    ctx.builder.append(
        InstKind::CellCopyTo {
            dest,
            source: error_cell,
        },
        span,
    )?;
    // A NEW list cell over the fresh region (built inline: `list_cell`'s
    // count is a compile-time constant, this one is `new_count`).
    let new_list = ctx.builder.append(
        InstKind::CellAlloc {
            tag: cell_tag::LIST,
        },
        span,
    )?;
    ctx.builder.append(
        InstKind::CellStoreAux {
            cell: new_list,
            value: new_count,
        },
        span,
    )?;
    // Addresses are < 2^31, so SExtend64 zero-extends the region offset.
    let p = ctx.builder.append(InstKind::SExtend64(new_region), span)?;
    ctx.builder.append(
        InstKind::CellStorePayload {
            cell: new_list,
            value: p,
        },
        span,
    )?;
    let list = ctx.builder.append(InstKind::CellPass(new_list), span)?;
    validation_cell(ctx, list, old_stop, span)
}

/// Interns one structural type node by linear scan (first-use order,
/// deterministic; `TypeNode` derives `PartialEq` through its field vectors).
fn intern_node(nodes: &mut Vec<TypeNode>, kind: TypeNodeKind) -> Result<u32, IrError> {
    if let Some(index) = nodes.iter().position(|node| node.kind == kind) {
        return u32::try_from(index).map_err(|_| IrError::Internal("lower: node index overflow"));
    }
    let index = u32::try_from(nodes.len())
        .map_err(|_| IrError::Internal("lower: type node count overflow"))?;
    nodes.push(TypeNode { kind });
    Ok(index)
}

/// Interns one parameter's structural type into `nodes`, returning the node
/// index. Iterative post-order walk over an explicit frame stack (no
/// recursion); depth is capped by `max_type_depth`, which M3 already enforces
/// at parse time, so exceeding it here is an internal invariant.
pub(super) fn node_for(
    typed: &TypedModule,
    ty: TypeId,
    nodes: &mut Vec<TypeNode>,
    max_depth: usize,
) -> Result<u32, IrError> {
    enum Frame {
        Type(TypeId, usize),
        Assemble(TypeData, usize),
    }
    let mut stack = vec![Frame::Type(ty, 0)];
    let mut children: Vec<u32> = Vec::new();
    let missing = || IrError::Internal("lower: missing interned type");
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Assemble(data, count) => {
                let taken = children
                    .len()
                    .checked_sub(count)
                    .ok_or(IrError::Internal("lower: type node child underflow"))?;
                let kind = match data {
                    TypeData::Optional(_) => {
                        TypeNodeKind::Optional(*children.get(taken).ok_or(missing())?)
                    }
                    TypeData::List(_) => TypeNodeKind::List(*children.get(taken).ok_or(missing())?),
                    TypeData::Record(index) => {
                        let shape = typed
                            .types
                            .record(index)
                            .ok_or(IrError::Internal("lower: missing record shape"))?;
                        // Fields are stored (and emitted here) in name-sorted
                        // order: the canonical order the lowering uses for the
                        // record's cell layout.
                        let fields = shape
                            .fields
                            .iter()
                            .enumerate()
                            .map(|(field_index, (name, _))| {
                                let child =
                                    *children.get(taken + field_index).ok_or_else(missing)?;
                                Ok(RecordField {
                                    name: name.clone(),
                                    ty: child,
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        TypeNodeKind::Record(fields)
                    }
                    _ => {
                        return Err(IrError::Internal(
                            "lower: container type node without children",
                        ))
                    }
                };
                children.truncate(taken);
                let index = intern_node(nodes, kind)?;
                children.push(index);
            }
            Frame::Type(id, depth) => {
                if depth > max_depth {
                    return Err(IrError::Internal(
                        "lower: parameter type depth exceeds max_type_depth",
                    ));
                }
                let data = typed.types.data(id).ok_or_else(missing)?;
                match data {
                    TypeData::I32 => children.push(intern_node(nodes, TypeNodeKind::I32)?),
                    TypeData::I64 => children.push(intern_node(nodes, TypeNodeKind::I64)?),
                    TypeData::F64 => children.push(intern_node(nodes, TypeNodeKind::F64)?),
                    TypeData::Bool => children.push(intern_node(nodes, TypeNodeKind::Bool)?),
                    TypeData::String => children.push(intern_node(nodes, TypeNodeKind::String)?),
                    TypeData::Bytes => children.push(intern_node(nodes, TypeNodeKind::Bytes)?),
                    TypeData::Null => children.push(intern_node(nodes, TypeNodeKind::Null)?),
                    TypeData::Optional(inner) => {
                        stack.push(Frame::Assemble(data, 1));
                        stack.push(Frame::Type(inner, depth.saturating_add(1)));
                    }
                    TypeData::List(inner) => {
                        stack.push(Frame::Assemble(data, 1));
                        stack.push(Frame::Type(inner, depth.saturating_add(1)));
                    }
                    TypeData::Record(index) => {
                        let shape = typed
                            .types
                            .record(index)
                            .ok_or(IrError::Internal("lower: missing record shape"))?;
                        stack.push(Frame::Assemble(data, shape.fields.len()));
                        for (_, field_ty) in shape.fields.iter().rev() {
                            stack.push(Frame::Type(*field_ty, depth.saturating_add(1)));
                        }
                    }
                    TypeData::Error
                    | TypeData::Validation
                    | TypeData::Batch
                    | TypeData::Request
                    | TypeData::Response => {
                        return Err(IrError::Internal(
                            "lower: unsupported type in a parameter type node",
                        ))
                    }
                }
            }
        }
    }
    children.pop().ok_or_else(missing)
}

/// The stage-2 database operation of a direct DB builtin, `None` otherwise.
pub(crate) fn db_op_of(builtin: BuiltinId) -> Option<DbOp> {
    match builtin {
        BuiltinId::DbGet => Some(DbOp::Get),
        BuiltinId::DbInsert => Some(DbOp::Insert),
        BuiltinId::DbQuery => Some(DbOp::Query),
        _ => None,
    }
}

/// Lowers a direct database builtin call: the table cell and the boxed
/// second argument are spilled like the validator constructor arguments (a
/// later argument may open blocks), then reloaded in the block the lowering
/// ends in.
pub(crate) fn lower_db_builtin(
    ctx: &mut LowerCtx,
    id: ExprId,
    op: DbOp,
    arguments: &[ExprId],
    span: Span,
) -> Result<InstId, IrError> {
    let (Some(&table), Some(&second)) = (arguments.first(), arguments.get(1)) else {
        return Err(IrError::Internal("lower: database builtin arity mismatch"));
    };
    lower_expr(ctx, table)?;
    let table_slot = ctx.builder.declare_local(ValueKind::I32)?;
    ctx.builder
        .append(InstKind::LocalSet(table_slot), ctx.expr_span(table))?;
    let value = lower_expr(ctx, second)?;
    let second_ty = ctx.expr_type(second)?;
    let boxed = to_slot(ctx, second_ty, value, ctx.expr_span(second))?;
    // LocalSet reads the immediately preceding instruction: re-yield the
    // boxed cell pointer so it is that instruction.
    ctx.builder
        .append(InstKind::CellPass(boxed), ctx.expr_span(second))?;
    let arg_slot = ctx.builder.declare_local(ValueKind::I32)?;
    ctx.builder
        .append(InstKind::LocalSet(arg_slot), ctx.expr_span(second))?;
    let table_cell = ctx.builder.append(InstKind::LocalGet(table_slot), span)?;
    let arg_cell = ctx.builder.append(InstKind::LocalGet(arg_slot), span)?;
    let result_ty = ctx.expr_type(id)?;
    lower_db_call(ctx, op, table_cell, arg_cell, result_ty, span)
}

/// Lowers one direct database call. `table` and `arg` are cell pointers that
/// live in the block the lowering ends in; the result node is the structural
/// type of the call expression (the contextual type the checker resolved).
pub(crate) fn lower_db_call(
    ctx: &mut LowerCtx,
    op: DbOp,
    table: InstId,
    arg: InstId,
    result_ty: TypeId,
    span: Span,
) -> Result<InstId, IrError> {
    let max_depth = ctx.limits.max_type_depth;
    let result = node_for(ctx.typed, result_ty, &mut ctx.type_nodes, max_depth)?;
    ctx.builder.append(
        InstKind::DbCall {
            op,
            table,
            arg,
            result,
        },
        span,
    )
}
