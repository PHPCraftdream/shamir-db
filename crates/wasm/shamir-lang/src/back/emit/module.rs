use wasm_encoder::{Encode, Function, ValType};

use thiserror::Error;

use super::abi::{emit_call_wrapper, emit_scalar_wrapper, NullCell, WrapperBases, WrapperInputs};
use super::helpers::ordered_helper_functions;
use super::scalar::emit_user_function;
use super::support::assemble::{assemble, intern_type, val_type, ModuleImage};
use super::support::db::checked::emit_checked_call_helper;
use super::support::db::{
    append_guard_keys, append_tx_check_strings, emit_batch_append_helper, emit_db_call_helper,
    emit_db_execute_helper, emit_dynmap_find_helper, emit_response_affected_helper,
    emit_response_get_helper, emit_response_skip_check_helper, emit_response_tx_check_helper,
    host_import_name, TxCheckPlan,
};
use super::support::eq::{emit_bytes_compare, emit_eq_helper};
use super::support::projection::{
    append_raise_messages, append_record_descriptors, collect_db_result_nodes,
    collect_projection_nodes, emit_projection_helper, projection_signature, reachable_nodes,
    ProjectionSpec,
};
use super::support::workspace::{compute_workspace, validate_caps, validate_cell_codec};
use crate::api::{CompileLimits, Span};
use crate::back::ir::{inst_result_kind, DbOp, IrModule, ParamProjection};
use crate::back::runtime::{
    emit_allocator, emit_error_raiser, RuntimeCaps, RuntimeError, STATIC_START,
};
use crate::front::CompileBudget;

/// Emitted code-section budget (design's runtime-code cap; static data has
/// its own 4 MiB cap below).
const CODE_BUDGET_BYTES: u64 = 64 * 1_024;
/// Hard ceiling on defined functions (import excluded) per the design.
const MAX_DEFINED_FUNCTIONS: u64 = 384;
/// Hard ceiling on wasm locals (params + locals + fixed scratch tail + dense
/// instruction value locals) per user function.
const MAX_LOCALS: u64 = 4096;
/// Hard ceiling on signature arity (params + result) per user function.
const MAX_ARITY: u64 = 33;
/// Parameter/field names are unrolled byte-by-byte into wrapper/projection
/// code, so their length is bounded before generation.
pub(crate) const MAX_UNROLLED_NAME_BYTES: usize = 64;

/// Emission failure for the compiled (IR-driven) module path.
#[derive(Debug, Error)]
pub enum EmitError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("helper dependency order rejected")]
    HelperOrder(#[from] super::helpers::HelperOrderError),
    #[error("limit {code} exceeded: limit {limit}, observed {observed}")]
    LimitExceeded {
        code: &'static str,
        limit: u64,
        observed: u64,
    },
    #[error("internal invariant: {0}")]
    Internal(&'static str),
}

/// Fixed message addresses (inside the static image) plus the assembled
/// image, for raise sites emitted by `scalar`.
pub(crate) struct EmitContext {
    pub overflow_msg_addr: u32,
    pub div0_msg_addr: u32,
    pub range_msg_addr: u32,
    pub bound_msg_addr: u32,
    pub unwrap_msg_addr: u32,
}

/// Resolved function indices and type indices of one compiled module; also
/// consumed by the scalar call-bridge wrapper (abi extension).
pub(crate) struct EmissionPlan {
    pub helper_count: u32,
    pub raise_index: u32,
    pub alloc_index: u32,
    pub decode_index: u32,
    pub encode_index: u32,
    /// The `shamir_call` export (call wrapper).
    pub wrapper_index: u32,
    /// First user-function index (IR call order).
    pub users_base: u32,
    /// First generated equality helper (bytes compare at `eq_base + 0`).
    pub eq_base: u32,
    pub user_count: u32,
    pub eq_count: u32,
    /// Runtime byte-compare helper index.
    pub compare_index: u32,
    /// Runtime cell-copy helper index (M1 graph root), called by `CellsCopy`.
    pub copy_cells_index: u32,
    /// Type index shared by every equality helper (`[i32,i32] -> [i32]`).
    pub eq_type_index: u32,
    /// Type index shared by projection helpers (`[i32] -> [i32]`).
    pub projection_type_index: u32,
    /// Type index per user function, in IR order.
    pub user_type_indices: Vec<u32>,
    pub checked_call_index: Option<u32>,
    /// Stage-2 database operation helpers.
    pub db_helpers: Vec<(DbOp, u32)>,

    /// Decode-class projection helper per distinct database result node:
    /// `(node index, fn index)`.
    pub db_projection_indices: Vec<(u32, u32)>,
    /// Typed builder helper function indices (`None` when unused).
    pub batch_append_index: Option<u32>,
    pub db_execute_index: Option<u32>,
    pub response_get_index: Option<u32>,
    pub response_affected_index: Option<u32>,
    pub skip_check_index: Option<u32>,
    /// Emits the transaction outcome check, when this module reaches `batch_tx`.
    pub tx_check_plan: Option<TxCheckPlan>,
    pub dynmap_find_index: Option<u32>,
    /// Static message/key addresses the builder helpers raise or look up
    /// (0 when the module has no builder instructions).
    pub msg_alias: u32,
    pub msg_bound: u32,
    pub msg_arg: u32,
    pub key_results: u32,
    pub key_records: u32,
    pub key_stats: u32,
    pub key_records_scanned: u32,
    pub key_skipped: u32,
    pub msg_skipped: u32,
    /// First generated projection helper (`eq_base + 1 + eq_count`).
    /// Projection helper per distinct type node: `(node index, fn index)` in
    /// the deterministic first-use order of the nodes.
    pub projection_indices: Vec<(u32, u32)>,
    bases: WrapperBases,
}
impl EmissionPlan {
    pub(crate) fn wrapper_bases(&self) -> WrapperBases {
        self.bases
    }
    pub(crate) fn checked_call_index(&self) -> Option<u32> {
        self.checked_call_index
    }
}

/// Assembles the M1 runtime module: no user functions, no generated equality
/// helpers, no extra types.
pub fn emit_runtime_module(
    static_bytes: &[u8],
    caps: RuntimeCaps,
) -> Result<Vec<u8>, RuntimeError> {
    validate_caps(&caps)?;
    let null_cell = validate_cell_codec()?;

    let static_len = u64::try_from(static_bytes.len()).unwrap_or(u64::MAX);
    let workspace = compute_workspace(static_len, &caps)?;

    let (helper_functions, decode_index, encode_index, _compare_index, _copy_cells_index) =
        ordered_helper_functions(workspace.scratch, workspace.sort_base, caps, false, 1)?;
    let helper_count = helper_functions.len() as u32;
    let error_raiser_index = 1 + helper_count;
    let alloc_index = error_raiser_index + 1;
    let call_index = alloc_index + 1;

    let mut tag_bytes = [0; 4];
    tag_bytes.copy_from_slice(&null_cell[0..4]);
    let mut aux_bytes = [0; 4];
    aux_bytes.copy_from_slice(&null_cell[4..8]);
    let mut payload_bytes = [0; 8];
    payload_bytes.copy_from_slice(&null_cell[8..16]);
    let null_cell_tag = u32::from_le_bytes(tag_bytes) as i32;
    let null_cell_aux = u32::from_le_bytes(aux_bytes) as i32;
    let null_cell_payload = i64::from_le_bytes(payload_bytes);

    let output_cap =
        i32::try_from(caps.output_bytes).map_err(|_| RuntimeError::RuntimeTooLarge {
            limit: i32::MAX as u64,
            observed: caps.output_bytes,
        })? as u32;
    let image = ModuleImage {
        static_bytes: static_bytes.to_vec(),
        helper_functions,
        error_raiser: emit_error_raiser(0, workspace.context_base),
        allocator: emit_allocator(workspace.layout, 0, 0),
        wrapper: emit_call_wrapper(
            decode_index,
            encode_index,
            alloc_index,
            error_raiser_index,
            WrapperBases {
                context: workspace.context_base,
                frame: workspace.frame_base,
                cell: workspace.cell_base,
                null: workspace.cell_base,
                scratch: workspace.scratch,
                output_cap,
            },
            NullCell {
                tag: null_cell_tag,
                aux: null_cell_aux,
                payload: null_cell_payload,
            },
        ),
        user_functions: Vec::new(),
        eq_functions: Vec::new(),
        projection_functions: Vec::new(),
        alloc_index,
        call_index,
        extra_types: Vec::new(),
        heap_base: workspace.layout.heap_base,
        layout: workspace.layout,
        db_imports: Vec::new(),
        db_projection_functions: Vec::new(),
        db_call_functions: Vec::new(),
        checked_call_functions: Vec::new(),
    };
    let (bytes, code_len) = assemble(&image)?;
    if code_len > CODE_BUDGET_BYTES {
        return Err(RuntimeError::RuntimeTooLarge {
            limit: CODE_BUDGET_BYTES,
            observed: code_len,
        });
    }
    Ok(bytes)
}

/// Encoded body size of one function (its single-function code section
/// entry, without the section header).
fn body_bytes(function: &Function) -> u64 {
    let mut bytes = Vec::new();
    function.encode(&mut bytes);
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// Charges work against the shared pipeline budget, mapping a fuel overflow
/// onto the emission limit error (finding 19).
fn charge_work(budget: &mut CompileBudget, amount: u64) -> Result<(), EmitError> {
    if budget.charge(amount, Span::source_start()).is_err() {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: budget.limit(),
            observed: budget.spent().saturating_add(amount),
        });
    }
    Ok(())
}

/// ABSOLUTE static address of one image offset (`STATIC_START + offset`);
/// shared with the projection module's baking helpers.
pub(crate) fn static_address(offset: u64) -> Result<u32, EmitError> {
    let addr = STATIC_START
        .checked_add(offset)
        .ok_or(EmitError::Internal("static address overflow"))?;
    if addr > i32::MAX as u64 {
        return Err(EmitError::Internal("static address exceeds i32"));
    }
    u32::try_from(addr).map_err(|_| EmitError::Internal("static address exceeds i32"))
}

/// Assembles the compiled module for one lowered IR module: runtime helpers,
/// error bridge, allocator, call wrapper, user functions in IR order, then
/// the generated equality helpers (bytes compare first, then one per shape).
pub(crate) fn emit_compiled_module(
    ir: &IrModule,
    limits: &CompileLimits,
    budget: &mut CompileBudget,
) -> Result<Vec<u8>, EmitError> {
    let caps = RuntimeCaps::default();
    validate_caps(&caps)?;
    let null_cell = validate_cell_codec()?;

    let entry = ir
        .functions
        .get(ir.entry as usize)
        .ok_or(EmitError::Internal("missing entry function"))?;
    let (mut static_bytes, baked) =
        append_raise_messages(&ir.static_bytes, &entry.param_names, limits)?;
    // finding 15: distinct projection nodes for the entry's aggregate
    // parameters, in deterministic first-use order (limits checked inside)
    let distinct = collect_projection_nodes(ir, entry)?;
    // stage-2 database calls: distinct result nodes (decode-class projection
    // helpers) and the host operations the module imports
    let db_nodes = collect_db_result_nodes(ir)?;
    let db_ops = ir.db_ops();
    let batch_uses = ir.batch_uses();
    let transactional = batch_uses.transactional;
    let checked_import = batch_uses.checked_call;
    let import_count = 1
        + u32::try_from(db_ops.len())
            .map_err(|_| EmitError::Internal("database import count overflow"))?
        + u32::from(batch_uses.execute)
        + u32::from(checked_import);
    let static_limit = limits.max_static_bytes as u64;
    // every record node reachable from a projected parameter or a database
    // result gets a descriptor
    let mut descriptor_roots = distinct.clone();
    descriptor_roots.extend_from_slice(&db_nodes);
    let reachable = reachable_nodes(&ir.type_nodes, &descriptor_roots)?;
    let reachable_addrs =
        append_record_descriptors(&mut static_bytes, &ir.type_nodes, &reachable, static_limit)?;
    let mut descriptors: Vec<Option<u32>> = vec![None; ir.type_nodes.len()];
    for (node_index, address) in reachable.iter().zip(reachable_addrs) {
        if let Some(slot) = descriptors.get_mut(*node_index as usize) {
            *slot = address;
        }
    }
    let (msg_alias, key_results, key_records, key_stats, key_records_scanned) =
        super::support::projection::append_builder_messages(
            &mut static_bytes,
            batch_uses.append,
            batch_uses.response_rows || batch_uses.guarded,
            batch_uses.response_affected,
            static_limit,
        )?;
    let (key_skipped, msg_skipped) = if batch_uses.guarded {
        append_guard_keys(&mut static_bytes, static_limit)?
    } else {
        (0, 0)
    };
    let tx_check_strings = if transactional {
        Some(append_tx_check_strings(&mut static_bytes, static_limit)?)
    } else {
        None
    };
    // the shared null cell lives in the static image, outside the decoder's
    // cell slots (the decoder overwrites those)
    let null_cell_offset = u64::try_from(static_bytes.len())
        .map_err(|_| EmitError::Internal("static image length overflow"))?;
    static_bytes.extend_from_slice(&null_cell);
    let null_cell_addr = static_address(null_cell_offset)?;
    let observed = u64::try_from(static_bytes.len())
        .map_err(|_| EmitError::Internal("static image length overflow"))?;
    if observed > static_limit {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: static_limit,
            observed,
        });
    }
    let ctx = EmitContext {
        overflow_msg_addr: baked.scalar[0],
        div0_msg_addr: baked.scalar[1],
        range_msg_addr: baked.scalar[2],
        bound_msg_addr: baked.scalar[3],
        unwrap_msg_addr: baked.scalar[4],
    };

    let workspace = compute_workspace(observed, &caps)?;
    let (helper_functions, decode_index, encode_index, compare_index, copy_cells_index) =
        ordered_helper_functions(
            workspace.scratch,
            workspace.sort_base,
            caps,
            true,
            import_count,
        )?;
    // finding 20: the ten ordered runtime helpers are not controlled by user
    // code, so their bytes count against `max_runtime_bytes`; user code is
    // bounded by `max_function_body_bytes` per body and the module by the
    // output cap.
    let helper_bytes: u64 = helper_functions
        .iter()
        .map(|(_, function)| body_bytes(function))
        .sum();
    let runtime_limit = limits.max_runtime_bytes as u64;
    if helper_bytes > runtime_limit {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: runtime_limit,
            observed: helper_bytes,
        });
    }

    // user types appended after the ten runtime types; eq helpers share the
    // dedicated equality signature
    let mut extra_types: Vec<(Vec<ValType>, Option<ValType>)> = Vec::new();
    let mut user_type_indices = Vec::with_capacity(ir.functions.len());
    for func in &ir.functions {
        let arity = u64::try_from(func.params.len())
            .map_err(|_| EmitError::Internal("param count overflow"))?
            .checked_add(u64::from(func.result.is_some()))
            .ok_or(EmitError::Internal("arity overflow"))?;
        if arity > MAX_ARITY {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: MAX_ARITY,
                observed: arity,
            });
        }
        // params + ir locals + fixed scratch + one dense local per
        // value-producing instruction (the operand-id contract)
        let value_insts = func
            .insts
            .iter()
            .filter(|inst| inst_result_kind(&inst.kind, func, &ir.functions).is_some())
            .count();
        let locals = u64::try_from(func.params.len())
            .map_err(|_| EmitError::Internal("param count overflow"))?
            .checked_add(u64::try_from(func.locals.len()).unwrap_or(u64::MAX))
            .and_then(|slots| slots.checked_add(6))
            .and_then(|slots| slots.checked_add(u64::try_from(value_insts).unwrap_or(u64::MAX)))
            .ok_or(EmitError::Internal("local count overflow"))?;
        if locals > MAX_LOCALS {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: MAX_LOCALS,
                observed: locals,
            });
        }
        let params: Vec<ValType> = func.params.iter().map(|kind| val_type(*kind)).collect();
        let result = func.result.map(val_type);
        user_type_indices.push(intern_type(&mut extra_types, params, result)?);
    }
    // eq helpers take two i32 scalars and return an i32
    let eq_type_index = intern_type(
        &mut extra_types,
        vec![ValType::I32, ValType::I32],
        Some(ValType::I32),
    )?;
    // projection helpers take one i32 and return an i32
    let (proj_params, proj_result) = projection_signature();
    let projection_type_index = intern_type(&mut extra_types, proj_params, proj_result)?;
    // host database imports: `(table ptr, table len, arg ptr, arg len) -> i64`
    let mut db_imports: Vec<(&'static str, u32)> = Vec::with_capacity(db_ops.len());
    if !db_ops.is_empty() {
        let import_type = intern_type(
            &mut extra_types,
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            Some(ValType::I64),
        )?;
        for op in &db_ops {
            db_imports.push((host_import_name(*op), import_type));
        }
    }
    if batch_uses.execute {
        db_imports.push(("db_execute", 8));
    }
    if checked_import {
        let checked_type =
            intern_type(&mut extra_types, vec![ValType::I32; 6], Some(ValType::I64))?;
        db_imports.push(("call_checked", checked_type));
    }
    let output_cap = i32::try_from(caps.output_bytes).map_err(|_| EmitError::LimitExceeded {
        code: "B001",
        limit: i32::MAX as u64,
        observed: caps.output_bytes,
    })? as u32;
    let bases = WrapperBases {
        context: workspace.context_base,
        frame: workspace.frame_base,
        cell: workspace.cell_base,
        null: null_cell_addr,
        scratch: workspace.scratch,
        output_cap,
    };
    let helper_count = helper_functions.len() as u32;
    let users_base = import_count + 3 + helper_count;
    let eq_base = users_base
        .checked_add(
            u32::try_from(ir.functions.len())
                .map_err(|_| EmitError::Internal("user function count overflow"))?,
        )
        .ok_or(EmitError::Internal("function index overflow"))?;
    let eq_count = u32::try_from(ir.eq_shapes.len())
        .map_err(|_| EmitError::Internal("shape count overflow"))?;
    // projection helpers follow the eq helpers (bytes compare first, then
    // one per shape); helper indices follow the distinct-node order
    let proj_base = eq_base
        .checked_add(
            1u32.checked_add(eq_count)
                .ok_or(EmitError::Internal("function index overflow"))?,
        )
        .ok_or(EmitError::Internal("function index overflow"))?;
    let mut projection_indices: Vec<(u32, u32)> = Vec::with_capacity(distinct.len());
    for (position, node_index) in distinct.iter().enumerate() {
        let fn_index = proj_base
            .checked_add(
                u32::try_from(position)
                    .map_err(|_| EmitError::Internal("projection index overflow"))?,
            )
            .ok_or(EmitError::Internal("function index overflow"))?;
        projection_indices.push((*node_index, fn_index));
    }
    // database projection helpers, then the database call helpers, follow the
    // entry projection helpers
    let db_proj_base = proj_base
        .checked_add(
            u32::try_from(distinct.len())
                .map_err(|_| EmitError::Internal("projection count overflow"))?,
        )
        .ok_or(EmitError::Internal("function index overflow"))?;
    let mut db_projection_indices: Vec<(u32, u32)> = Vec::with_capacity(db_nodes.len());
    for (position, node_index) in db_nodes.iter().enumerate() {
        let fn_index = db_proj_base
            .checked_add(
                u32::try_from(position)
                    .map_err(|_| EmitError::Internal("projection index overflow"))?,
            )
            .ok_or(EmitError::Internal("function index overflow"))?;
        db_projection_indices.push((*node_index, fn_index));
    }
    let db_call_base = db_proj_base
        .checked_add(
            u32::try_from(db_nodes.len())
                .map_err(|_| EmitError::Internal("projection count overflow"))?,
        )
        .ok_or(EmitError::Internal("function index overflow"))?;
    let mut db_helpers: Vec<(DbOp, u32)> = Vec::with_capacity(db_ops.len());
    for (position, op) in db_ops.iter().enumerate() {
        let fn_index = db_call_base
            .checked_add(
                u32::try_from(position)
                    .map_err(|_| EmitError::Internal("database helper index overflow"))?,
            )
            .ok_or(EmitError::Internal("function index overflow"))?;
        db_helpers.push((*op, fn_index));
    }
    let mut next_batch_fn = db_call_base
        .checked_add(
            u32::try_from(db_ops.len())
                .map_err(|_| EmitError::Internal("database helper count overflow"))?,
        )
        .ok_or(EmitError::Internal("function index overflow"))?;
    let mut take_index = |needed: bool| {
        if needed {
            let index = next_batch_fn;
            next_batch_fn += 1;
            Some(index)
        } else {
            None
        }
    };
    let dynmap_find_index = take_index(
        batch_uses.response_rows
            || batch_uses.response_affected
            || batch_uses.guarded
            || transactional,
    );
    let batch_append_index = take_index(batch_uses.append);
    let db_execute_index = take_index(batch_uses.execute);
    let response_get_index = take_index(batch_uses.response_rows);
    let response_affected_index = take_index(batch_uses.response_affected);
    let tx_check_index = take_index(transactional);
    let skip_check_index = take_index(batch_uses.guarded);
    let tx_check_plan = match (tx_check_index, tx_check_strings) {
        (Some(helper_index), Some(strings)) => Some(TxCheckPlan {
            helper_index,
            strings,
        }),
        (None, None) => None,
        _ => {
            return Err(EmitError::Internal(
                "transaction check registration mismatch",
            ))
        }
    };
    let checked_call_index = if checked_import {
        Some(next_batch_fn)
    } else {
        None
    };
    let plan = EmissionPlan {
        helper_count,
        raise_index: import_count + helper_count,
        alloc_index: import_count + 1 + helper_count,
        decode_index,
        encode_index,
        wrapper_index: import_count + 2 + helper_count,
        users_base,
        eq_base,
        user_count: u32::try_from(ir.functions.len())
            .map_err(|_| EmitError::Internal("user function count overflow"))?,
        eq_count,
        compare_index,
        copy_cells_index,
        eq_type_index,
        projection_type_index,
        user_type_indices,
        checked_call_index,
        db_helpers,
        db_projection_indices,
        batch_append_index,
        db_execute_index,
        response_get_index,
        response_affected_index,
        skip_check_index,
        tx_check_plan,
        dynmap_find_index,
        msg_alias,
        msg_bound: baked.scalar[3],
        msg_arg: baked.wrapper.arg,
        key_results,
        key_records,
        key_stats,
        key_records_scanned,
        key_skipped,
        msg_skipped,
        projection_indices,
        bases,
    };

    // function-count ceiling before any body emission
    let runtime_count = u64::from(plan.helper_count) + 3; // raise, alloc, wrapper
    let projection_count = u64::try_from(plan.projection_indices.len())
        .map_err(|_| EmitError::Internal("projection count overflow"))?;
    let total_functions = runtime_count
        .checked_add(u64::from(plan.user_count))
        .and_then(|total| total.checked_add(u64::from(plan.eq_count)))
        .and_then(|total| total.checked_add(projection_count))
        .and_then(|total| total.checked_add(plan.db_projection_indices.len() as u64))
        .and_then(|total| total.checked_add(plan.db_helpers.len() as u64))
        .and_then(|total| total.checked_add(u64::from(plan.checked_call_index.is_some())))
        .and_then(|total| {
            total.checked_add(
                u64::from(plan.dynmap_find_index.is_some())
                    + u64::from(plan.batch_append_index.is_some())
                    + u64::from(plan.db_execute_index.is_some())
                    + u64::from(plan.response_get_index.is_some())
                    + u64::from(plan.response_affected_index.is_some())
                    + u64::from(plan.skip_check_index.is_some())
                    + u64::from(plan.tx_check_plan.is_some()),
            )
        })
        .ok_or(EmitError::Internal("function count overflow"))?;
    if total_functions > MAX_DEFINED_FUNCTIONS {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: MAX_DEFINED_FUNCTIONS,
            observed: total_functions,
        });
    }

    let body_limit = limits.max_function_body_bytes as u64;
    let mut user_functions = Vec::with_capacity(plan.user_count as usize);
    for (index, func) in ir.functions.iter().enumerate() {
        let fn_index = plan.users_base as usize + index;
        charge_work(budget, 1)?;
        let function = emit_user_function(&plan, &ctx, fn_index, func, &ir.functions)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        user_functions.push((plan.user_type_indices[index], function));
    }

    let mut eq_functions = Vec::with_capacity(plan.eq_count as usize + 1);
    charge_work(budget, 1)?;
    let bytes_compare = emit_bytes_compare(&plan);
    charge_work(budget, body_bytes(&bytes_compare))?;
    eq_functions.push((plan.eq_type_index, bytes_compare));
    for shape_index in 0..plan.eq_count as usize {
        charge_work(budget, 1)?;
        let function = emit_eq_helper(&plan, &ir.eq_shapes, shape_index)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        eq_functions.push((plan.eq_type_index, function));
    }

    // projection helpers: one per distinct aggregate parameter type (finding
    // 15); record descriptors were baked above in the same node order
    let mut projection_functions = Vec::with_capacity(plan.projection_indices.len());
    let spec = ProjectionSpec {
        nodes: &ir.type_nodes,
        descriptors: &descriptors,
        alloc_index: plan.alloc_index,
        raise_index: plan.raise_index,
        null_cell: null_cell_addr,
        arg_msg_addr: baked.wrapper.arg,
        i32_msg_addr: baked.wrapper.i32,
        field_msg_addr: baked.wrapper.field,
        raise_kind: 2,
    };
    for &(node_index, _fn_index) in plan.projection_indices.iter() {
        charge_work(budget, 1)?;
        let function = emit_projection_helper(&spec, &ir.type_nodes, node_index)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        projection_functions.push((plan.projection_type_index, function));
    }

    // decode-class (kind 3) projection helpers of the database results and
    // the database call helpers themselves
    let db_spec = ProjectionSpec {
        nodes: &ir.type_nodes,
        descriptors: &descriptors,
        alloc_index: plan.alloc_index,
        raise_index: plan.raise_index,
        null_cell: null_cell_addr,
        arg_msg_addr: baked.wrapper.arg,
        i32_msg_addr: baked.wrapper.i32,
        field_msg_addr: baked.wrapper.field,
        raise_kind: 3,
    };
    let mut db_projection_functions = Vec::with_capacity(plan.db_projection_indices.len());
    for &(node_index, _fn_index) in plan.db_projection_indices.iter() {
        charge_work(budget, 1)?;
        let function = emit_projection_helper(&db_spec, &ir.type_nodes, node_index)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        db_projection_functions.push((plan.projection_type_index, function));
    }
    let mut db_call_functions = Vec::with_capacity(plan.db_helpers.len());
    for (position, &(op, _fn_index)) in plan.db_helpers.iter().enumerate() {
        charge_work(budget, 1)?;
        // host import i sits after `raise_error` (import 0)
        let import_index = 1 + u32::try_from(position)
            .map_err(|_| EmitError::Internal("database import index overflow"))?;
        let function = emit_db_call_helper(&plan, op, import_index)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        // `(table cell, argument cell) -> cell` shares the runtime type 1
        db_call_functions.push((1u32, function));
    }
    // typed builder helpers (types: 0 = `(i32,i32,i32) -> i32`, 1 = `(i32,i32)
    // -> i32`, 7 = `(i32) -> i32`)
    let mut push_helper = |type_index: u32,
                           function: wasm_encoder::Function,
                           budget: &mut CompileBudget|
     -> Result<(), EmitError> {
        charge_work(budget, 1)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        db_call_functions.push((type_index, function));
        Ok(())
    };
    if plan.dynmap_find_index.is_some() {
        push_helper(0, emit_dynmap_find_helper(&plan)?, budget)?;
    }
    if plan.batch_append_index.is_some() {
        push_helper(0, emit_batch_append_helper(&plan)?, budget)?;
    }
    if plan.db_execute_index.is_some() {
        let import_index = 1 + u32::try_from(db_ops.len())
            .map_err(|_| EmitError::Internal("database import index overflow"))?;
        push_helper(7, emit_db_execute_helper(&plan, import_index)?, budget)?;
    }
    if let (Some(find), Some(_response)) = (plan.dynmap_find_index, plan.response_get_index) {
        push_helper(1, emit_response_get_helper(&plan, find)?, budget)?;
    }
    if let (Some(find), Some(_affected)) = (plan.dynmap_find_index, plan.response_affected_index) {
        push_helper(1, emit_response_affected_helper(&plan, find)?, budget)?;
    }
    if let Some(tx_plan) = plan.tx_check_plan {
        let find = plan
            .dynmap_find_index
            .ok_or(EmitError::Internal("missing transaction map finder"))?;
        push_helper(
            1,
            emit_response_tx_check_helper(&plan, &tx_plan, find)?,
            budget,
        )?;
    }
    if let (Some(find), Some(_skip_check)) = (plan.dynmap_find_index, plan.skip_check_index) {
        push_helper(7, emit_response_skip_check_helper(&plan, find)?, budget)?;
    }

    let mut checked_call_functions = Vec::new();
    if plan.checked_call_index.is_some() {
        let import_index = 1
            + u32::try_from(db_ops.len())
                .map_err(|_| EmitError::Internal("checked import index overflow"))?
            + u32::from(batch_uses.execute);
        let function = emit_checked_call_helper(&plan, import_index)?;
        let observed = body_bytes(&function);
        if observed > body_limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: body_limit,
                observed,
            });
        }
        charge_work(budget, observed)?;
        checked_call_functions.push((
            intern_type(&mut extra_types, vec![ValType::I32; 3], Some(ValType::I32))?,
            function,
        ));
    }
    // per-parameter projection helper indices for the wrapper (None = the
    // inline pass-through for hand-built IR without nodes)
    let param_helper: Vec<Option<u32>> = entry
        .param_projections
        .iter()
        .enumerate()
        .map(|(slot, kind)| {
            if !matches!(kind, ParamProjection::Cell | ParamProjection::OptionalCell) {
                return None;
            }
            let node_index = *entry.param_type_nodes.get(slot)?;
            plan.projection_indices
                .iter()
                .find(|&&(node, _)| node == node_index)
                .map(|&(_, fn_index)| fn_index)
        })
        .collect();

    let mut tag_bytes = [0; 4];
    tag_bytes.copy_from_slice(&null_cell[0..4]);
    let mut aux_bytes = [0; 4];
    aux_bytes.copy_from_slice(&null_cell[4..8]);
    let mut payload_bytes = [0; 8];
    payload_bytes.copy_from_slice(&null_cell[8..16]);

    let error_raiser = emit_error_raiser(0, workspace.context_base);
    let allocator = emit_allocator(workspace.layout, 0, 0);
    // Scalar and validator entries both compile; the wrapper choice is the
    // only difference (the validator wrapper adds the Validation-contract
    // checks on top of the shared scalar envelope). Procedures stay rejected
    // by the compile pipeline's entry gate.
    charge_work(budget, 1)?;
    let wrapper = {
        let entry_index = plan
            .users_base
            .checked_add(ir.entry)
            .ok_or(EmitError::Internal("entry index overflow"))?;
        let inputs = WrapperInputs {
            decode_index: plan.decode_index,
            encode_index: plan.encode_index,
            alloc_index: plan.alloc_index,
            raise_index: plan.raise_index,
            entry_index,
            bases: plan.wrapper_bases(),
            messages: baked.wrapper,
            param_kinds: &entry.param_projections,
            result_projection: entry.result_projection,
            param_names: &baked.names,
            param_helper: &param_helper,
            reinit_context: !plan.db_helpers.is_empty()
                || batch_uses.execute
                || batch_uses.response_rows
                || batch_uses.response_affected
                || batch_uses.checked_call,
        };
        if ir.entry_is_validator {
            super::support::validator::emit_validator_wrapper(inputs)?
        } else {
            emit_scalar_wrapper(inputs)?
        }
    };
    charge_work(budget, body_bytes(&wrapper))?;
    // finding 20: raiser + allocator + wrapper complete the runtime portion
    // (helpers + wrapper count against `max_runtime_bytes`; user code against
    // `max_function_body_bytes` and the output cap).
    let runtime_total = helper_bytes
        .checked_add(body_bytes(&error_raiser))
        .and_then(|total| total.checked_add(body_bytes(&allocator)))
        .and_then(|total| total.checked_add(body_bytes(&wrapper)))
        .ok_or(EmitError::Internal("runtime byte count overflow"))?;
    if runtime_total > runtime_limit {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: runtime_limit,
            observed: runtime_total,
        });
    }

    let image = ModuleImage {
        static_bytes,
        helper_functions,
        error_raiser,
        allocator,
        wrapper,
        user_functions,
        eq_functions,
        projection_functions,
        alloc_index: plan.alloc_index,
        call_index: plan.wrapper_index,
        extra_types,
        heap_base: workspace.layout.heap_base,
        layout: workspace.layout,
        db_imports,
        db_projection_functions,
        db_call_functions,
        checked_call_functions,
    };
    let (bytes, code_len) = assemble(&image)?;
    if code_len > CODE_BUDGET_BYTES {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: CODE_BUDGET_BYTES,
            observed: code_len,
        });
    }
    let output_limit = limits.max_output_bytes as u64;
    let final_len =
        u64::try_from(bytes.len()).map_err(|_| EmitError::Internal("module size overflow"))?;
    if final_len > output_limit {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: output_limit,
            observed: final_len,
        });
    }
    Ok(bytes)
}
