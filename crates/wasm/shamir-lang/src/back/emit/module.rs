use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, DataSection, Encode, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    MemorySection, MemoryType, Module, TypeSection, ValType,
};

use shamir_wasm_admission::{
    ABI_SECTION, EXPORT_ALLOC, EXPORT_CALL, EXPORT_MEMORY, ORIGIN_SECTION, SUPPORTED_ABI_VERSION,
};

use thiserror::Error;

use super::abi::{emit_call_wrapper, emit_scalar_wrapper, NullCell, WrapperBases, WrapperInputs};
use super::helpers::{emit_helper_order, HelperGraph, HelperId};
use super::scalar::emit_user_function;
use super::support::eq::{emit_bytes_compare, emit_eq_helper};
use super::support::projection::{
    append_raise_messages, append_record_descriptors, collect_projection_nodes,
    emit_projection_helper, projection_signature, ProjectionSpec,
};
use super::support::workspace::compute_workspace;
use crate::api::{CompileLimits, Span};
use crate::back::ir::{inst_result_kind, IrModule, ParamProjection, ValueKind};
use crate::back::runtime::{
    emit_allocator, emit_append_byte, emit_check_range, emit_compare_bytes, emit_copy_cells,
    emit_decode_root, emit_encode_root, emit_error_raiser, emit_read_be, emit_read_byte,
    emit_store_cell, emit_validate_utf8, Cell, CellTag, DecodeEmit, EncodeEmit, Layout,
    RuntimeCaps, RuntimeError, MAX_ADDRESS, STATIC_START,
};
use crate::front::CompileBudget;

/// Design hard ceilings; exceeding any of them is a caller bug (the cap
/// values themselves are the supported envelope), rejected before layout.
const MAX_INPUT_BYTES: u64 = 8 * 1_024 * 1_024;
const MAX_OUTPUT_BYTES: u64 = 8 * 1_024 * 1_024;
const MAX_STRING_BYTES: u64 = 1_024 * 1_024;
const MAX_KEY_BYTES: u64 = 65_536;
const MAX_CONTAINERS: u64 = 100_000;
const MAX_ENTRIES: u64 = 100_000;
const MAX_CELLS: u64 = 1_000_000;
const MAX_DEPTH: u64 = 32;
/// Emitted code-section budget (design's runtime-code cap; static data has
/// its own 4 MiB cap below).
const CODE_BUDGET_BYTES: u64 = 64 * 1_024;
/// Hard ceiling on defined functions (import excluded) per the design.
const MAX_DEFINED_FUNCTIONS: u64 = 384;
/// Hard ceiling on function types per the design.
const MAX_TYPES: u64 = 512;
/// Hard ceiling on wasm locals (params + locals + fixed scratch tail + dense
/// instruction value locals) per user function.
const MAX_LOCALS: u64 = 4096;
/// Hard ceiling on signature arity (params + result) per user function.
const MAX_ARITY: u64 = 33;
/// Parameter/field names are unrolled byte-by-byte into wrapper/projection
/// code, so their length is bounded before generation.
pub(crate) const MAX_UNROLLED_NAME_BYTES: usize = 64;

type OrderedHelpers = (Vec<(u32, Function)>, u32, u32, u32);

/// Every cell tag must survive an encode/decode round trip before the wrapper
/// embeds the null-cell constants, keeping the cell codec exercised in
/// production.
const ALL_CELL_TAGS: [CellTag; 15] = [
    CellTag::Unused,
    CellTag::Null,
    CellTag::Bool,
    CellTag::I32,
    CellTag::I64,
    CellTag::F64,
    CellTag::String,
    CellTag::Bytes,
    CellTag::List,
    CellTag::Record,
    CellTag::Validation,
    CellTag::Batch,
    CellTag::Request,
    CellTag::Response,
    CellTag::DynamicMap,
];
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
    /// Type index shared by every equality helper (`[i32,i32] -> [i32]`).
    pub eq_type_index: u32,
    /// Type index shared by projection helpers (`[i32] -> [i32]`).
    pub projection_type_index: u32,
    /// Type index per user function, in IR order.
    pub user_type_indices: Vec<u32>,
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
}

fn validate_caps(caps: &RuntimeCaps) -> Result<(), RuntimeError> {
    // design hard ceilings first: a cap above these is a configuration bug
    if caps.input_bytes > MAX_INPUT_BYTES
        || caps.output_bytes > MAX_OUTPUT_BYTES
        || caps.string_bytes > MAX_STRING_BYTES
        || caps.key_bytes > MAX_KEY_BYTES
        || caps.containers > MAX_CONTAINERS
        || caps.entries > MAX_ENTRIES
        || caps.cells > MAX_CELLS
        || u64::from(caps.depth) > MAX_DEPTH
    {
        return Err(RuntimeError::CapAboveCeiling);
    }
    let within_limit = |value: u64| value <= MAX_ADDRESS;
    if !within_limit(caps.input_bytes)
        || !within_limit(caps.output_bytes)
        || !within_limit(caps.string_bytes)
        || !within_limit(caps.key_bytes)
        || !within_limit(caps.containers)
        || !within_limit(caps.entries)
        || !within_limit(caps.cells)
        || !within_limit(u64::from(caps.depth))
    {
        return Err(RuntimeError::CapsBeyondAddressSpace);
    }
    Ok(())
}

fn validate_cell_codec() -> Result<[u8; 16], RuntimeError> {
    for &tag in ALL_CELL_TAGS.iter() {
        let cell = Cell::new(tag, 0, 0);
        let Some(decoded) = Cell::decode(cell.encode()) else {
            return Err(RuntimeError::CellTemplate);
        };
        if decoded.tag() != tag || decoded.auxiliary() != 0 || decoded.payload() != 0 {
            return Err(RuntimeError::CellTemplate);
        }
    }
    Ok(Cell::new(CellTag::Null, 0, 0).encode())
}

/// Registers each codec primitive plus the iterative decoder as a
/// `HelperGraph` node with its real dependencies, orders them deterministically
/// (BTree-backed), and returns the emitted bodies with their type indices in
/// dependency-first order together with the decoder/encoder module indices.
fn ordered_helper_functions(
    scratch: u32,
    sort_base: u32,
    caps: RuntimeCaps,
) -> Result<OrderedHelpers, RuntimeError> {
    const TYPE_RANGE: u32 = 0;
    const TYPE_SMALL: u32 = 1;
    const TYPE_READ_BE: u32 = 3;
    const TYPE_VOID_THREE_I32: u32 = 4;
    const TYPE_STORE_CELL: u32 = 5;
    const TYPE_APPEND: u32 = 6;
    const TYPE_DECODE: u32 = 9;
    const TYPE_ENCODE: u32 = 9;

    let mut graph = HelperGraph::default();
    let range = graph.add(&[]);
    let utf8 = graph.add(&[]);
    let compare = graph.add(&[]);
    let read_byte = graph.add(&[]);
    let read_be = graph.add(&[read_byte]);
    let copy = graph.add(&[]);
    let store = graph.add(&[]);
    let append = graph.add(&[]);
    // the decoder calls range validation, byte/be reads, UTF-8 validation,
    // byte comparison (duplicate keys) and cell stores; it allocates heap
    // copies through the allocator and raises through the error helper, both
    // emitted after the graph order is fixed. The encoder appends to the
    // output buffer and compares map-key bytes for the insertion sort.
    let decode = graph.add(&[range, read_byte, read_be, utf8, compare, store]);
    let encode = graph.add(&[append, compare]);
    let roots = [
        range, utf8, compare, read_byte, read_be, copy, store, append, decode, encode,
    ];
    let order = emit_helper_order(&graph, &roots)?;

    let helper_count = order.len() as u32;
    let error_raiser_index = 1 + helper_count;
    let alloc_index = error_raiser_index + 1;
    let index_of = |helper: HelperId| -> Result<u32, RuntimeError> {
        let position = order
            .iter()
            .position(|&candidate| candidate == helper)
            .ok_or(RuntimeError::UnknownHelper)?;
        u32::try_from(position)
            .map(|position| 1 + position)
            .map_err(|_| RuntimeError::UnknownHelper)
    };
    let decode_params = DecodeEmit {
        scratch,
        raise_index: error_raiser_index,
        alloc_index,
        check_range_index: index_of(range)?,
        read_byte_index: index_of(read_byte)?,
        read_be_index: index_of(read_be)?,
        compare_index: index_of(compare)?,
        utf8_index: index_of(utf8)?,
        store_cell_index: index_of(store)?,
        caps,
    };
    let encode_params = EncodeEmit {
        scratch,
        sort_base,
        raise_index: error_raiser_index,
        append_index: index_of(append)?,
        compare_index: index_of(compare)?,
        caps,
    };

    let mut functions: Vec<(u32, Function)> = Vec::with_capacity(order.len());
    for helper in &order {
        let entry = match *helper {
            id if id == range => (TYPE_RANGE, emit_check_range()),
            id if id == utf8 => (TYPE_SMALL, emit_validate_utf8()),
            id if id == compare => (TYPE_COMPARE, emit_compare_bytes()),
            id if id == read_byte => (TYPE_SMALL, emit_read_byte()),
            id if id == read_be => (TYPE_READ_BE, emit_read_be()),
            id if id == copy => (TYPE_VOID_THREE_I32, emit_copy_cells()),
            id if id == store => (TYPE_STORE_CELL, emit_store_cell()),
            id if id == append => (TYPE_APPEND, emit_append_byte()),
            id if id == decode => (TYPE_DECODE, emit_decode_root(&decode_params)),
            id if id == encode => (TYPE_ENCODE, emit_encode_root(&encode_params)),
            _ => return Err(RuntimeError::UnknownHelper),
        };
        functions.push(entry);
    }
    let decode_index = index_of(decode)?;
    let encode_index = index_of(encode)?;
    let compare_index = index_of(compare)?;
    Ok((functions, decode_index, encode_index, compare_index))
}

const TYPE_VOID_THREE_I32: u32 = 4;
const TYPE_ALLOC: u32 = 7;
const TYPE_CALL: u32 = 8;
/// Runtime byte-compare helper type (`[i32,i32] -> [i32]`, position 2).
const TYPE_COMPARE: u32 = 2;

/// One fully prepared module ready for section assembly.
struct ModuleImage {
    static_bytes: Vec<u8>,
    helper_functions: Vec<(u32, Function)>,
    error_raiser: Function,
    allocator: Function,
    wrapper: Function,
    user_functions: Vec<(u32, Function)>,
    eq_functions: Vec<(u32, Function)>,
    /// Generated projection helpers, appended after the eq helpers.
    projection_functions: Vec<(u32, Function)>,
    alloc_index: u32,
    call_index: u32,
    /// Extra function types beyond the ten runtime ones (user + eq).
    extra_types: Vec<(Vec<ValType>, Option<ValType>)>,
    heap_base: u64,
    layout: Layout,
}

/// Assembles the complete module: custom ABI/origin sections, type, import
/// (error raiser at index 0), function, memory, global, export and code
/// sections, plus one active data segment carrying the static bytes. Returns
/// the module bytes and the encoded code-section length.
fn assemble(image: &ModuleImage) -> Result<(Vec<u8>, u64), RuntimeError> {
    let mut types = TypeSection::new();
    let i = ValType::I32;
    let l = ValType::I64;
    for (params, result) in [
        (vec![i, i, i], vec![i]),
        (vec![i, i], vec![i]),
        (vec![i, i, i, i], vec![i]),
        (vec![i, i, i], vec![l]),
        (vec![i, i, i], vec![]),
        (vec![i, i, i, l], vec![]),
        (vec![i, i], vec![]),
        (vec![i], vec![i]),
        (vec![i, i], vec![l]),
        (vec![i], vec![]),
    ] {
        types.ty().function(params, result);
    }
    for (params, result) in &image.extra_types {
        let result: &[ValType] = match result {
            Some(kind) => std::slice::from_ref(kind),
            None => &[],
        };
        types
            .ty()
            .function(params.iter().copied(), result.iter().copied());
    }

    let mut imports = ImportSection::new();
    imports.import(
        "shamir_host",
        "raise_error",
        EntityType::Function(TYPE_VOID_THREE_I32),
    );

    let mut functions = FunctionSection::new();
    for (type_index, _) in &image.helper_functions {
        functions.function(*type_index);
    }
    functions.function(TYPE_VOID_THREE_I32);
    functions.function(TYPE_ALLOC);
    functions.function(TYPE_CALL);
    for (type_index, _) in &image.user_functions {
        functions.function(*type_index);
    }
    for (type_index, _) in &image.eq_functions {
        functions.function(*type_index);
    }
    for (type_index, _) in &image.projection_functions {
        functions.function(*type_index);
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: image.layout.minimum_pages,
        maximum: Some(image.layout.maximum_pages),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut globals = GlobalSection::new();
    // two mutable i32 globals per design §5: heap_next (index 0) and
    // call_state (index 1, the wrapper re-entry guard)
    let heap_const = ConstExpr::i32_const(i32::try_from(image.heap_base).map_err(|_| {
        RuntimeError::RuntimeTooLarge {
            limit: i32::MAX as u64,
            observed: image.heap_base,
        }
    })?);
    let global_type = GlobalType {
        val_type: ValType::I32,
        mutable: true,
        shared: false,
    };
    globals.global(global_type, &heap_const);
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(0),
    );

    let mut exports = ExportSection::new();
    exports.export(EXPORT_MEMORY, ExportKind::Memory, 0);
    exports.export(EXPORT_ALLOC, ExportKind::Func, image.alloc_index);
    exports.export(EXPORT_CALL, ExportKind::Func, image.call_index);

    let mut code = CodeSection::new();
    for (_, function) in &image.helper_functions {
        code.function(function);
    }
    code.function(&image.error_raiser);
    code.function(&image.allocator);
    code.function(&image.wrapper);
    for (_, function) in &image.user_functions {
        code.function(function);
    }
    for (_, function) in &image.eq_functions {
        code.function(function);
    }
    for (_, function) in &image.projection_functions {
        code.function(function);
    }

    let mut data = DataSection::new();
    let static_const = ConstExpr::i32_const(i32::try_from(STATIC_START).map_err(|_| {
        RuntimeError::RuntimeTooLarge {
            limit: i32::MAX as u64,
            observed: STATIC_START,
        }
    })?);
    data.active(0, &static_const, image.static_bytes.to_vec());

    let mut module = Module::new();
    module.section(&CustomSection {
        name: ABI_SECTION.into(),
        data: SUPPORTED_ABI_VERSION.to_le_bytes().to_vec().into(),
    });
    module.section(&CustomSection {
        name: ORIGIN_SECTION.into(),
        data: b"shamir-lang".to_vec().into(),
    });
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&memories);
    module.section(&globals);
    module.section(&exports);
    module.section(&code);
    module.section(&data);

    let bytes = module.finish();
    let mut code_bytes = Vec::new();
    code.encode(&mut code_bytes);
    let code_len = u64::try_from(code_bytes.len()).unwrap_or(u64::MAX);
    Ok((bytes, code_len))
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

    let (helper_functions, decode_index, encode_index, _compare_index) =
        ordered_helper_functions(workspace.scratch, workspace.sort_base, caps)?;
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

/// Deduplicates one signature by linear scan over the extra types
/// (first-use order, deterministic); the runtime occupies types 0..=9.
fn intern_type(
    extra_types: &mut Vec<(Vec<ValType>, Option<ValType>)>,
    params: Vec<ValType>,
    result: Option<ValType>,
) -> Result<u32, EmitError> {
    if let Some(position) = extra_types
        .iter()
        .position(|(existing, existing_result)| *existing == params && *existing_result == result)
    {
        let index =
            u32::try_from(position).map_err(|_| EmitError::Internal("type index overflow"))?;
        return 10u32
            .checked_add(index)
            .ok_or(EmitError::Internal("type index overflow"));
    }
    let position =
        u32::try_from(extra_types.len()).map_err(|_| EmitError::Internal("type index overflow"))?;
    extra_types.push((params, result));
    let observed = 10u64
        .checked_add(u64::from(position) + 1)
        .ok_or(EmitError::Internal("type count overflow"))?;
    if observed > MAX_TYPES {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: MAX_TYPES,
            observed,
        });
    }
    10u32
        .checked_add(position)
        .ok_or(EmitError::Internal("type index overflow"))
}

fn val_type(kind: ValueKind) -> ValType {
    match kind {
        ValueKind::I32 => ValType::I32,
        ValueKind::I64 => ValType::I64,
        ValueKind::F64 => ValType::F64,
    }
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
    let static_limit = limits.max_static_bytes as u64;
    let descriptor_addrs =
        append_record_descriptors(&mut static_bytes, &ir.type_nodes, &distinct, static_limit)?;
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
    let (helper_functions, decode_index, encode_index, compare_index) =
        ordered_helper_functions(workspace.scratch, workspace.sort_base, caps)?;
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
    let output_cap = i32::try_from(caps.output_bytes).map_err(|_| EmitError::LimitExceeded {
        code: "B001",
        limit: i32::MAX as u64,
        observed: caps.output_bytes,
    })? as u32;
    let bases = WrapperBases {
        context: workspace.context_base,
        frame: workspace.frame_base,
        cell: workspace.cell_base,
        scratch: workspace.scratch,
        output_cap,
    };
    let helper_count = helper_functions.len() as u32;
    let users_base = 4 + helper_count;
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
    let plan = EmissionPlan {
        helper_count,
        raise_index: 1 + helper_count,
        alloc_index: 2 + helper_count,
        decode_index,
        encode_index,
        wrapper_index: 3 + helper_count,
        users_base,
        eq_base,
        user_count: u32::try_from(ir.functions.len())
            .map_err(|_| EmitError::Internal("user function count overflow"))?,
        eq_count,
        compare_index,
        eq_type_index,
        projection_type_index,
        user_type_indices,
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
        alloc_index: plan.alloc_index,
        raise_index: plan.raise_index,
        null_cell: workspace.cell_base,
        arg_msg_addr: baked.wrapper.arg,
        i32_msg_addr: baked.wrapper.i32,
        field_msg_addr: baked.wrapper.field,
    };
    for (position, &(node_index, _fn_index)) in plan.projection_indices.iter().enumerate() {
        // None for non-record nodes (the helper ignores it); Some for records
        let descriptor_addr = descriptor_addrs.get(position).copied().flatten();
        charge_work(budget, 1)?;
        let function = emit_projection_helper(&spec, &ir.type_nodes, node_index, descriptor_addr)?;
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
    // the compiled path always carries a scalar entry (the compile
    // pipeline's entry gate rejects every other entry kind), so the
    // M1 pass-through wrapper is replaced by the scalar wrapper
    charge_work(budget, 1)?;
    let wrapper = {
        let entry_index = plan
            .users_base
            .checked_add(ir.entry)
            .ok_or(EmitError::Internal("entry index overflow"))?;
        emit_scalar_wrapper(WrapperInputs {
            decode_index: plan.decode_index,
            encode_index: plan.encode_index,
            alloc_index: plan.alloc_index,
            raise_index: plan.raise_index,
            entry_index,
            bases: plan.wrapper_bases(),
            null_cell: NullCell {
                tag: u32::from_le_bytes(tag_bytes) as i32,
                aux: u32::from_le_bytes(aux_bytes) as i32,
                payload: i64::from_le_bytes(payload_bytes),
            },
            messages: baked.wrapper,
            param_kinds: &entry.param_projections,
            result_projection: entry.result_projection,
            param_names: &baked.names,
            param_helper: &param_helper,
        })?
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
