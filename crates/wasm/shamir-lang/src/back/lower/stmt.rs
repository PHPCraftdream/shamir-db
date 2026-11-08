use crate::api::Span;
use crate::back::ir::{ArithOp, BlockId, CmpOp, InstId, InstKind, IrError, ValueKind};
use crate::front::{ExprId, ForKind, StatementId, StatementKind};
use crate::sem::{TypeData, TypeId};

use super::ctx::{LoopFrame, LowerCtx};
use super::expr::lower_expr;
use super::types::{convert, is_scalar, value_kind};

/// Lowers one statement into the current block.
pub(crate) fn lower_stmt(ctx: &mut LowerCtx, id: StatementId) -> Result<(), IrError> {
    let node = ctx
        .module
        .statement(id)
        .ok_or(IrError::Internal("lower: missing statement node"))?;
    let span = node.span;
    match &node.kind {
        StatementKind::Block(children) => {
            for child in children {
                lower_stmt(ctx, *child)?;
            }
            Ok(())
        }
        StatementKind::Let { value, .. } => {
            let slot = ctx.binding_slot(id)?;
            let slot_ty = ctx.slot_type(slot)?;
            let value_inst = lower_expr(ctx, *value)?;
            // The last appended inst is the (possibly boxed) value; LocalSet
            // takes it from the immediately preceding instruction.
            convert(
                ctx,
                slot_ty,
                ctx.expr_type(*value)?,
                value_inst,
                ctx.expr_span(*value),
            )?;
            ctx.builder.append(InstKind::LocalSet(slot), span)?;
            Ok(())
        }
        StatementKind::Assign { target, value } => {
            let slot = assign_slot(ctx, *target)?;
            let slot_ty = ctx.slot_type(slot)?;
            let value_inst = lower_expr(ctx, *value)?;
            convert(
                ctx,
                slot_ty,
                ctx.expr_type(*value)?,
                value_inst,
                ctx.expr_span(*value),
            )?;
            ctx.builder.append(InstKind::LocalSet(slot), span)?;
            Ok(())
        }
        StatementKind::If {
            condition,
            then_block,
            else_branch,
        } => lower_if(ctx, *condition, *then_block, *else_branch),
        StatementKind::While { condition, body } => lower_while(ctx, *condition, *body),
        StatementKind::For {
            kind,
            start_or_list,
            end,
            body,
            ..
        } => match kind {
            ForKind::Range => {
                let end_expr =
                    (*end).ok_or(IrError::Internal("lower: range for without an end"))?;
                lower_for_range(ctx, id, *start_or_list, end_expr, *body)
            }
            ForKind::List => lower_for_list(ctx, id, *start_or_list, *body),
        },
        StatementKind::Return(value) => lower_return(ctx, *value, span),
        StatementKind::Break => {
            ctx.loop_stack
                .last()
                .ok_or(IrError::Internal("lower: break outside a loop"))?;
            let block = ctx
                .builder
                .current_block()
                .ok_or(IrError::Internal("lower: break outside a block"))?;
            ctx.builder.terminate_pending_branch()?;
            ctx.pending_exits.push(block);
            Ok(())
        }
        StatementKind::Continue => {
            ctx.loop_stack
                .last()
                .ok_or(IrError::Internal("lower: continue outside a loop"))?;
            let block = ctx
                .builder
                .current_block()
                .ok_or(IrError::Internal("lower: continue outside a block"))?;
            ctx.builder.terminate_pending_branch()?;
            ctx.pending_continues.push(block);
            Ok(())
        }
        StatementKind::Throw(value) => {
            let message = lower_expr(ctx, *value)?;
            ctx.builder.terminate_trap_cell(message)
        }
        StatementKind::Expression(value) => {
            let value_inst = lower_expr(ctx, *value)?;
            ctx.builder.append(InstKind::Drop(value_inst), span)?;
            Ok(())
        }
    }
}

/// Assignment target: an identifier resolved to a declared MUTABLE local.
fn assign_slot(ctx: &mut LowerCtx, target: ExprId) -> Result<u32, IrError> {
    let resolved = ctx
        .typed
        .resolved
        .get(target.0 as usize)
        .copied()
        .flatten()
        .ok_or(IrError::Internal("lower: unresolved assignment target"))?;
    match resolved {
        crate::sem::ResolvedTarget::Local(slot) => {
            let params = u32::try_from(ctx.param_types.len())
                .map_err(|_| IrError::Internal("lower: parameter count overflow"))?;
            if slot < params {
                return Err(IrError::Internal("lower: assignment to a parameter"));
            }
            if !ctx.is_mutable_local(slot)? {
                return Err(IrError::Internal("lower: assignment to an immutable local"));
            }
            Ok(slot)
        }
        _ => Err(IrError::Internal("lower: assignment to a non-local")),
    }
}

fn lower_return(ctx: &mut LowerCtx, value: Option<ExprId>, _span: Span) -> Result<(), IrError> {
    let Some(result_ty) = ctx.result_ty else {
        if value.is_some() {
            return Err(IrError::Internal(
                "lower: value returned from a void function",
            ));
        }
        ctx.builder.terminate_return(None)?;
        return Ok(());
    };
    let Some(value) = value else {
        return Err(IrError::Internal(
            "lower: bare return in a value-returning function",
        ));
    };
    let value_inst = lower_expr(ctx, value)?;
    let converted = convert(
        ctx,
        result_ty,
        ctx.expr_type(value)?,
        value_inst,
        ctx.expr_span(value),
    )?;
    ctx.builder.terminate_return(Some(converted))?;
    Ok(())
}

fn lower_if(
    ctx: &mut LowerCtx,
    condition: ExprId,
    then_block: StatementId,
    else_branch: Option<StatementId>,
) -> Result<(), IrError> {
    let cond = lower_expr(ctx, condition)?;
    // The condition's value inst lives in the block the BranchIf terminates;
    // capturing it after the condition leaves the branch block unterminated.
    let cond_block = ctx
        .builder
        .current_block()
        .ok_or(IrError::Internal("lower: if outside a block"))?;
    let region = ctx.block_region();
    // Arms are created BEFORE the BranchIf is emitted, and the join LAST
    // (after the arms are filled), so every branch target is forward and
    // blocks tile in fill order.
    let then_b = ctx.builder.open_block(region)?;
    let else_b = match else_branch {
        Some(_) => Some(ctx.builder.open_block(region)?),
        None => None,
    };
    ctx.builder.set_current(cond_block)?;
    match else_b {
        Some(else_b) => ctx.builder.terminate_branch_if(cond, then_b, else_b)?,
        None => ctx.builder.terminate_branch_if_then(cond, then_b)?,
    }
    ctx.builder.set_current(then_b)?;
    lower_stmt(ctx, then_block)?;
    let mut then_pending = None;
    if let Some(tail) = ctx.builder.current_block() {
        ctx.builder.terminate_pending_branch()?;
        then_pending = Some(tail);
    }
    let mut else_pending = None;
    if let (Some(branch), Some(else_b)) = (else_branch, else_b) {
        ctx.builder.set_current(else_b)?;
        lower_stmt(ctx, branch)?;
        if let Some(tail) = ctx.builder.current_block() {
            ctx.builder.terminate_pending_branch()?;
            else_pending = Some(tail);
        }
    }
    let join = ctx.builder.open_block(region)?;
    if let Some(block) = then_pending {
        ctx.builder.patch_pending_branch(block, join)?;
    }
    if let Some(block) = else_pending {
        ctx.builder.patch_pending_branch(block, join)?;
    }
    // No-else if: the BranchIf's else target was pending (the join did not
    // exist when it was emitted); name it now.
    if else_b.is_none() {
        ctx.builder.patch_branch_if(cond_block, join)?;
    }
    ctx.builder.set_current(join)?;
    Ok(())
}

/// Patches the pending branches recorded since `exit_mark`/`cont_mark` (the
/// loop body just lowered): breaks with `exit`, continues/fallthroughs with
/// `cont`. Inner loops drained their own ranges when they closed.
fn patch_loop_branches(
    ctx: &mut LowerCtx,
    exit_mark: usize,
    cont_mark: usize,
    exit: BlockId,
    cont: BlockId,
) -> Result<(), IrError> {
    for block in ctx.pending_exits.drain(exit_mark..) {
        ctx.builder.patch_pending_branch(block, exit)?;
    }
    for block in ctx.pending_continues.drain(cont_mark..) {
        ctx.builder.patch_pending_branch(block, cont)?;
    }
    Ok(())
}

fn lower_while(ctx: &mut LowerCtx, condition: ExprId, body: StatementId) -> Result<(), IrError> {
    let entry = ctx
        .builder
        .current_block()
        .ok_or(IrError::Internal("lower: loop outside a block"))?;
    let region = ctx.loop_region()?;
    // Open the loop header FIRST and make it current so the condition is
    // re-evaluated every iteration; push the frame before lowering the
    // condition so any blocks it opens stay inside the region (rule 5).
    let header = ctx.builder.open_loop(region)?;
    ctx.loop_stack.push(LoopFrame { region });
    let exit_mark = ctx.pending_exits.len();
    let cont_mark = ctx.pending_continues.len();
    ctx.builder.set_current(header)?;
    let cond = lower_expr(ctx, condition)?;
    let body_b = ctx.builder.open_block(Some(region))?;
    // The entry falls into the header (forward: the header was created after
    // the entry stopped appending); the header tests and branches forward.
    ctx.builder.set_current(entry)?;
    ctx.builder.terminate_branch(header)?;
    ctx.builder.set_current(body_b)?;
    lower_stmt(ctx, body)?;
    // The body fallthrough and every `continue` reach the header test; the
    // exit block is created after the body so every `break` stays forward.
    if let Some(tail) = ctx.builder.current_block() {
        ctx.builder.terminate_pending_branch()?;
        ctx.pending_continues.push(tail);
    }
    ctx.loop_stack.pop();
    let exit_b = ctx.builder.open_block(Some(region))?;
    patch_loop_branches(ctx, exit_mark, cont_mark, exit_b, header)?;
    ctx.builder.set_current(header)?;
    ctx.builder.terminate_branch_if(cond, body_b, exit_b)?;
    ctx.builder.set_current(exit_b)?;
    Ok(())
}

/// Range for: the header only tests `cur >= end`; the body publishes the
/// item and the increment block steps the counter, so `continue` and the
/// body tail both reach the test through the increment. The body/increment/
/// exit blocks are created in that order (all forward from their branchers)
/// and the test-before-increment order makes `0..1` run exactly once while
/// `MAX..MAX` runs zero times (the increment's checked add cannot trap
/// because it only runs while cur < end <= MAX).
fn lower_for_range(
    ctx: &mut LowerCtx,
    id: StatementId,
    start: ExprId,
    end: ExprId,
    body: StatementId,
) -> Result<(), IrError> {
    let span = ctx.stmt_span(id);
    let entry = ctx
        .builder
        .current_block()
        .ok_or(IrError::Internal("lower: loop outside a block"))?;
    let start_data = ctx.type_data(ctx.expr_type(start)?)?;
    let end_data = ctx.type_data(ctx.expr_type(end)?)?;
    let width = match (start_data, end_data) {
        (TypeData::I32, TypeData::I32) => ValueKind::I32,
        (TypeData::I64, TypeData::I64) => ValueKind::I64,
        _ => {
            return Err(IrError::Internal(
                "lower: range bounds are not one integer width",
            ))
        }
    };
    let item_slot = ctx.binding_slot(id)?;
    let item_data = ctx.type_data(ctx.slot_type(item_slot)?)?;
    if is_scalar(item_data) && value_kind(item_data) != width {
        return Err(IrError::Internal("lower: for item width mismatch"));
    }
    let cur = ctx.builder.declare_local(width)?;
    let end_t = ctx.builder.declare_local(width)?;
    // Preheader: the bounds land in temps (region-free, before the loop).
    lower_expr(ctx, start)?;
    ctx.builder
        .append(InstKind::LocalSet(cur), ctx.expr_span(start))?;
    lower_expr(ctx, end)?;
    ctx.builder
        .append(InstKind::LocalSet(end_t), ctx.expr_span(end))?;
    let region = ctx.loop_region()?;
    let header = ctx.builder.open_loop(region)?;
    ctx.loop_stack.push(LoopFrame { region });
    let exit_mark = ctx.pending_exits.len();
    let cont_mark = ctx.pending_continues.len();
    // Header: the exit test only.
    ctx.builder.set_current(header)?;
    let cur_get = ctx.builder.append(InstKind::LocalGet(cur), span)?;
    let end_get = ctx.builder.append(InstKind::LocalGet(end_t), span)?;
    let done = ctx.builder.append(
        InstKind::Compare {
            op: CmpOp::Ge,
            width,
            left: cur_get,
            right: end_get,
        },
        span,
    )?;
    let body_b = ctx.builder.open_block(Some(region))?;
    // The entry falls into the header unconditionally (forward target).
    ctx.builder.set_current(entry)?;
    ctx.builder.terminate_branch(header)?;
    ctx.builder.set_current(body_b)?;
    // Body: publish the item, run the body; the fallthrough and every
    // `continue` reach the increment block (created after the body, so all
    // such branches stay forward).
    ctx.builder.append(InstKind::LocalGet(cur), span)?;
    ctx.builder.append(InstKind::LocalSet(item_slot), span)?;
    lower_stmt(ctx, body)?;
    if let Some(tail) = ctx.builder.current_block() {
        ctx.builder.terminate_pending_branch()?;
        ctx.pending_continues.push(tail);
    }
    ctx.loop_stack.pop();
    let incr_b = ctx.builder.open_block(Some(region))?;
    let exit_b = ctx.builder.open_block(Some(region))?;
    patch_loop_branches(ctx, exit_mark, cont_mark, exit_b, incr_b)?;
    ctx.builder.set_current(incr_b)?;
    let one = match width {
        ValueKind::I64 => InstKind::ConstI64(1),
        _ => InstKind::ConstI32(1),
    };
    let one_inst = ctx.builder.append(one, span)?;
    let cur_get = ctx.builder.append(InstKind::LocalGet(cur), span)?;
    ctx.builder.append(
        InstKind::CheckedArith {
            op: ArithOp::Add,
            width,
            left: cur_get,
            right: one_inst,
        },
        span,
    )?;
    ctx.builder.append(InstKind::LocalSet(cur), span)?;
    // Back edge to the header re-runs the exit test.
    ctx.builder.terminate_branch(header)?;
    ctx.builder.set_current(header)?;
    ctx.builder.terminate_branch_if(done, exit_b, body_b)?;
    ctx.builder.set_current(exit_b)?;
    Ok(())
}

/// List for: the header only tests `idx >= len` (an empty list never
/// touches an element slot); the body publishes the item and the increment
/// block steps the index.
fn lower_for_list(
    ctx: &mut LowerCtx,
    id: StatementId,
    list: ExprId,
    body: StatementId,
) -> Result<(), IrError> {
    let span = ctx.stmt_span(id);
    let entry = ctx
        .builder
        .current_block()
        .ok_or(IrError::Internal("lower: loop outside a block"))?;
    let item_slot = ctx.binding_slot(id)?;
    let list_t = ctx.builder.declare_local(ValueKind::I32)?;
    let len_t = ctx.builder.declare_local(ValueKind::I32)?;
    let idx = ctx.builder.declare_local(ValueKind::I32)?;
    // Preheader: the list, its length and the index start.
    lower_expr(ctx, list)?;
    ctx.builder
        .append(InstKind::LocalSet(list_t), ctx.expr_span(list))?;
    let list_get = ctx.builder.append(InstKind::LocalGet(list_t), span)?;
    ctx.builder.append(InstKind::CellLen(list_get), span)?;
    ctx.builder.append(InstKind::LocalSet(len_t), span)?;
    ctx.builder.append(InstKind::ConstI32(0), span)?;
    ctx.builder.append(InstKind::LocalSet(idx), span)?;
    let region = ctx.loop_region()?;
    let header = ctx.builder.open_loop(region)?;
    ctx.loop_stack.push(LoopFrame { region });
    let exit_mark = ctx.pending_exits.len();
    let cont_mark = ctx.pending_continues.len();
    // Header: exit when the index reaches the length.
    let idx_get = ctx.builder.append(InstKind::LocalGet(idx), span)?;
    let len_get = ctx.builder.append(InstKind::LocalGet(len_t), span)?;
    let done = ctx.builder.append(
        InstKind::Compare {
            op: CmpOp::Ge,
            width: ValueKind::I32,
            left: idx_get,
            right: len_get,
        },
        span,
    )?;
    let body_b = ctx.builder.open_block(Some(region))?;
    // The entry falls into the header unconditionally (forward target), so
    // an empty list never touches an element slot.
    ctx.builder.set_current(entry)?;
    ctx.builder.terminate_branch(header)?;
    ctx.builder.set_current(body_b)?;
    // Body: load the element, publish the item, run the body; the
    // fallthrough and every `continue` reach the increment block (created
    // after the body, so all such branches stay forward).
    let list_get = ctx.builder.append(InstKind::LocalGet(list_t), span)?;
    let idx_get = ctx.builder.append(InstKind::LocalGet(idx), span)?;
    let elem = ctx.builder.append(
        InstKind::CellElemPtr {
            base: list_get,
            index: idx_get,
        },
        span,
    )?;
    load_item(ctx, elem, ctx.slot_type(item_slot)?, span)?;
    ctx.builder.append(InstKind::LocalSet(item_slot), span)?;
    lower_stmt(ctx, body)?;
    if let Some(tail) = ctx.builder.current_block() {
        ctx.builder.terminate_pending_branch()?;
        ctx.pending_continues.push(tail);
    }
    ctx.loop_stack.pop();
    let incr_b = ctx.builder.open_block(Some(region))?;
    let exit_b = ctx.builder.open_block(Some(region))?;
    patch_loop_branches(ctx, exit_mark, cont_mark, exit_b, incr_b)?;
    ctx.builder.set_current(incr_b)?;
    // Increment: idx + 1 cannot trap because idx < len when it runs; the
    // back edge to the header re-runs the exit test.
    let one = ctx.builder.append(InstKind::ConstI32(1), span)?;
    let idx_get = ctx.builder.append(InstKind::LocalGet(idx), span)?;
    ctx.builder.append(
        InstKind::CheckedArith {
            op: ArithOp::Add,
            width: ValueKind::I32,
            left: idx_get,
            right: one,
        },
        span,
    )?;
    ctx.builder.append(InstKind::LocalSet(idx), span)?;
    ctx.builder.terminate_branch(header)?;
    ctx.builder.set_current(header)?;
    ctx.builder.terminate_branch_if(done, exit_b, body_b)?;
    ctx.builder.set_current(exit_b)?;
    Ok(())
}

/// Appends the load of one for-list item slot value by its declared type.
fn load_item(ctx: &mut LowerCtx, elem: InstId, item_ty: TypeId, span: Span) -> Result<(), IrError> {
    match ctx.type_data(item_ty)? {
        TypeData::I64 => {
            ctx.builder.append(InstKind::CellLoadPayload(elem), span)?;
        }
        TypeData::I32 | TypeData::Bool => {
            let payload = ctx.builder.append(InstKind::CellLoadPayload(elem), span)?;
            ctx.builder.append(InstKind::I32WrapI64(payload), span)?;
        }
        TypeData::F64 => {
            let payload = ctx.builder.append(InstKind::CellLoadPayload(elem), span)?;
            ctx.builder.append(InstKind::F64FromBits(payload), span)?;
        }
        // Aggregate/optional items are cell pointers already.
        _ => {}
    }
    Ok(())
}
