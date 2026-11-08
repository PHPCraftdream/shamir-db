use std::collections::BTreeMap;

use crate::api::CompileLimits;
use crate::back::ir::{
    IrBuilder, IrError, IrLimits, IrModule, ParamProjection, RecordField, TypeNode, TypeNodeKind,
};
use crate::front::budget::CompileBudget;
use crate::front::EntryKind;
use crate::front::Module;
use crate::sem::{TypeData, TypeId, TypedFunction, TypedModule};

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
        max_ir_nodes_per_function: limits.max_ir_nodes_per_function,
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
        validation_descriptor: None,
        field_error_descriptor: None,
        code_error_descriptor: None,
        error_cap_cell: None,
    };
    for fid in &typed.call_order {
        let function = typed
            .function(*fid)
            .ok_or(IrError::Internal("lower: missing typed function"))?;
        lower_function(&mut ctx, function)?;
    }
    let mut ir = ctx.builder.finish()?;
    // Validator entries share the scalar emission path; the wrapper selects
    // the validator contract when this flag is set.
    ir.entry_is_validator = typed.entry_kind == EntryKind::Validator;
    for (index, function) in ir.functions.iter_mut().enumerate() {
        if let Some((params, result)) = ctx.projections.get(index) {
            function.param_projections = params.clone();
            function.result_projection = *result;
        }
    }
    // Structural parameter type nodes (finding 15): interned per parameter in
    // call order, patched with the same indexing as the projections patch.
    let (type_nodes, param_node_lists) = build_type_nodes(typed, limits.max_type_depth)?;
    for (index, nodes) in param_node_lists.into_iter().enumerate() {
        let function = ir.functions.get_mut(index).ok_or(IrError::Internal(
            "lower: call order exceeds emitted functions",
        ))?;
        function.param_type_nodes = nodes;
    }
    ir.type_nodes = type_nodes;
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
fn node_for(
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

/// Structural type nodes for every function's parameters (call order).
fn build_type_nodes(
    typed: &TypedModule,
    max_depth: usize,
) -> Result<(Vec<TypeNode>, Vec<Vec<u32>>), IrError> {
    let mut nodes = Vec::new();
    let mut lists = Vec::with_capacity(typed.call_order.len());
    for fid in &typed.call_order {
        let function = typed
            .function(*fid)
            .ok_or(IrError::Internal("lower: missing typed function"))?;
        let mut list = Vec::with_capacity(function.parameters.len());
        for parameter in &function.parameters {
            list.push(node_for(typed, parameter.ty, &mut nodes, max_depth)?);
        }
        lists.push(list);
    }
    Ok((nodes, lists))
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
    // the lowering context is reused across functions; reset the per-function
    // loop/branch state before lowering this one
    ctx.loop_stack.clear();
    ctx.pending_exits.clear();
    ctx.pending_continues.clear();
    ctx.region_counter = 0;
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
