use std::collections::BTreeMap;

use crate::api::CompileLimits;
use crate::back::ir::{IrBuilder, IrError, IrLimits, IrModule, ParamProjection};
use crate::front::budget::CompileBudget;
use crate::front::Module;
use crate::sem::{TypeData, TypedFunction, TypedModule};

use super::ctx::LowerCtx;
use super::stmt::lower_stmt;
use super::types::value_kind;

/// Lowers a checked module to typed IR: functions are emitted in
/// `call_order` (callees first), dead functions are not emitted.
pub(crate) fn lower(
    module: &Module,
    typed: &TypedModule,
    limits: &CompileLimits,
    budget: &mut CompileBudget,
) -> Result<IrModule, IrError> {
    let ir_limits = IrLimits {
        max_ir_nodes: limits.max_ir_nodes,
        max_operators_per_function: limits.max_operators_per_function,
        max_single_literal_bytes: limits.max_single_literal_bytes,
        max_total_literal_bytes: limits.max_total_literal_bytes,
        max_static_bytes: limits.max_static_bytes,
    };
    let mut function_map = BTreeMap::new();
    for (index, fid) in typed.call_order.iter().enumerate() {
        let index = u32::try_from(index)
            .map_err(|_| IrError::Internal("lower: function index overflow"))?;
        function_map.insert(fid.0, index);
    }
    let mut ctx = LowerCtx {
        module,
        typed,
        limits,
        builder: IrBuilder::new(ir_limits, budget),
        function_map,
        record_descriptors: BTreeMap::new(),
        region_counter: 0,
        loop_stack: Vec::new(),
        pending_exits: Vec::new(),
        pending_continues: Vec::new(),
        param_types: Vec::new(),
        local_types: Vec::new(),
        local_mutable: Vec::new(),
        result_ty: None,
        projections: Vec::new(),
    };
    for fid in &typed.call_order {
        let function = typed
            .function(*fid)
            .ok_or(IrError::Internal("lower: missing typed function"))?;
        lower_function(&mut ctx, function)?;
    }
    let mut ir = ctx.builder.finish()?;
    for (index, function) in ir.functions.iter_mut().enumerate() {
        if let Some((params, result)) = ctx.projections.get(index) {
            function.param_projections = params.clone();
            function.result_projection = *result;
        }
    }
    // Declared parameter names back into IR order (the entry wrapper reads
    // the entry function's names for the named-form envelope).
    for (index, fid) in typed.call_order.iter().enumerate() {
        let function = ir.functions.get_mut(index).ok_or(IrError::Internal(
            "lower: call order exceeds emitted functions",
        ))?;
        let typed_function = typed
            .function(*fid)
            .ok_or(IrError::Internal("lower: missing typed function"))?;
        function.param_names = typed_function
            .parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
    }
    let entry_position = typed
        .call_order
        .iter()
        .position(|fid| *fid == typed.entry)
        .ok_or(IrError::Internal("lower: entry not in call order"))?;
    ir.entry = u32::try_from(entry_position)
        .map_err(|_| IrError::Internal("lower: entry index overflow"))?;
    Ok(ir)
}

fn projection(data: TypeData, as_param: bool) -> ParamProjection {
    match data {
        TypeData::I32 => ParamProjection::I32,
        TypeData::I64 => ParamProjection::I64,
        TypeData::F64 => ParamProjection::F64,
        TypeData::Bool => ParamProjection::Bool,
        // Optional parameters stay omittable in named mode; a result cannot
        // be omitted and projects as a plain cell.
        TypeData::Optional(_) if as_param => ParamProjection::OptionalCell,
        _ => ParamProjection::Cell,
    }
}

fn lower_function(ctx: &mut LowerCtx, function: &TypedFunction) -> Result<(), IrError> {
    let mut params = Vec::with_capacity(function.parameters.len());
    let mut param_types = Vec::with_capacity(function.parameters.len());
    for parameter in &function.parameters {
        let data = ctx.type_data(parameter.ty)?;
        if data == TypeData::Error {
            return Err(IrError::Internal("lower: error type in a parameter"));
        }
        param_types.push(parameter.ty);
        params.push(value_kind(data));
    }
    let return_data = ctx.type_data(function.return_type)?;
    if return_data == TypeData::Error {
        return Err(IrError::Internal("lower: error type in a result"));
    }
    let (result, result_ty) = match return_data {
        // `-> null` functions are void.
        TypeData::Null => (None, None),
        data => (Some(value_kind(data)), Some(function.return_type)),
    };
    let param_count = u32::try_from(function.parameters.len())
        .map_err(|_| IrError::Internal("lower: parameter count overflow"))?;
    let mut locals = Vec::with_capacity(function.locals.len());
    let mut local_types = Vec::with_capacity(function.locals.len());
    let mut local_mutable = Vec::with_capacity(function.locals.len());
    for local in &function.locals {
        let data = ctx.type_data(local.ty)?;
        if data == TypeData::Error {
            return Err(IrError::Internal("lower: error type in a local"));
        }
        let index = local
            .slot
            .checked_sub(param_count)
            .ok_or(IrError::Internal(
                "lower: local slot inside the parameter range",
            ))?;
        if index as usize != locals.len() {
            return Err(IrError::Internal("lower: local slots are not contiguous"));
        }
        locals.push(value_kind(data));
        local_types.push(local.ty);
        local_mutable.push(local.mutable);
    }
    ctx.param_types = param_types;
    ctx.local_types = local_types;
    ctx.local_mutable = local_mutable;
    ctx.result_ty = result_ty;
    let mut param_projections = Vec::with_capacity(function.parameters.len());
    for parameter in &function.parameters {
        let data = ctx.type_data(parameter.ty)?;
        param_projections.push(projection(data, true));
    }
    let result_projection = match return_data {
        TypeData::Null => None,
        data => Some(projection(data, false)),
    };
    ctx.projections.push((param_projections, result_projection));
    ctx.builder.begin_function(params, result, locals)?;
    ctx.builder.open_block(None)?;
    lower_stmt(ctx, function.body)?;
    // Fallthrough guard: M3 proved every path returns; a still-open block is
    // terminated defensively. Void functions may fall through with `return`.
    if ctx.builder.current_block().is_some() {
        if result.is_none() {
            ctx.builder.terminate_return(None)?;
        } else {
            ctx.builder.terminate_unreachable()?;
        }
    }
    ctx.builder.finish_function()?;
    ctx.param_types = Vec::new();
    ctx.local_types = Vec::new();
    ctx.local_mutable = Vec::new();
    ctx.result_ty = None;
    Ok(())
}
