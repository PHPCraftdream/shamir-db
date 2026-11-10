use crate::api::Span;
use crate::back::ir::{InstId, InstKind, IrError};
use crate::front::ExprId;

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use super::types::node_for;

/// Lowers a checked stored call with per-call static name/signature cells and typed argument map.
pub(crate) fn lower_checked_call(
    ctx: &mut LowerCtx,
    id: ExprId,
    args: &[ExprId],
    span: Span,
) -> Result<InstId, IrError> {
    let metadata = ctx
        .typed
        .checked_calls
        .get(id.0 as usize)
        .and_then(Clone::clone)
        .ok_or(IrError::Internal("lower: missing checked-call metadata"))?;
    if args.len() != 2 {
        return Err(IrError::Internal("lower: checked-call arity mismatch"));
    }
    let arguments = lower_expr(ctx, args[1])?;
    let name_offset = ctx.builder.string_literal(metadata.name.into_bytes())?;
    let signature_offset = ctx.builder.bytes_literal(metadata.signature)?;
    let name = ctx
        .builder
        .append(InstKind::StaticCell(name_offset), span)?;
    let signature = ctx
        .builder
        .append(InstKind::StaticCell(signature_offset), span)?;
    let result_ty = ctx.expr_type(id)?;
    let result_node = node_for(
        ctx.typed,
        result_ty,
        &mut ctx.type_nodes,
        ctx.limits.max_type_depth,
    )?;
    let root = ctx.builder.append(
        InstKind::CallChecked {
            name,
            signature,
            args: arguments,
            result: result_node,
        },
        span,
    )?;
    if super::types::is_scalar(ctx.type_data(result_ty)?) {
        super::expr::load_element(ctx, root, result_ty, span)
    } else {
        Ok(root)
    }
}
