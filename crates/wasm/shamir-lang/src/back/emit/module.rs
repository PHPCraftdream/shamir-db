use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, CustomSection, DataSection, Encode, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction, MemArg, MemorySection, MemoryType, Module, TypeSection, ValType,
};

use shamir_wasm_admission::{
    ABI_SECTION, EXPORT_ALLOC, EXPORT_CALL, EXPORT_MEMORY, ORIGIN_SECTION, SUPPORTED_ABI_VERSION,
};

use super::helpers::{emit_helper_order, HelperGraph, HelperId};
use crate::back::runtime::{
    emit_allocator, emit_append_byte, emit_check_range, emit_compare_bytes, emit_copy_cells,
    emit_decode_root, emit_encode_root, emit_error_raiser, emit_read_be, emit_read_byte,
    emit_store_cell, emit_validate_utf8, Cell, CellTag, DecodeEmit, EncodeEmit, Layout,
    RuntimeCaps, RuntimeError, CONTEXT_CELL_BASE_OFFSET, CONTEXT_FRAME_BASE_OFFSET,
    CONTEXT_HEADER_BYTES, CONTEXT_INPUT_LEN_OFFSET, CONTEXT_INPUT_PTR_OFFSET,
    CONTEXT_OUTPUT_CAP_OFFSET, CONTEXT_OUTPUT_LEN_OFFSET, CONTEXT_OUTPUT_PTR_OFFSET,
    CONTEXT_RESERVED_OFFSET, CONTEXT_SCRATCH_OFFSET, FRAME_BYTES, FRAME_COUNT, MAX_ADDRESS,
    MAX_STATIC_BYTES, PAGE_BYTES, STATIC_START,
};

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
/// Bytes occupied by one runtime cell.
const CELL_SLOT_BYTES: u64 = 16;
/// Bytes occupied by one encoder pair-index slot.
const SORT_INDEX_BYTES: u64 = 4;
/// Bytes of the shared codec reject-message scratch slot.
const SCRATCH_BYTES: u64 = 16;
/// Linear-memory page size in bytes, mirrored from the layout arithmetic.
const WASM_PAGE_BYTES: u32 = 65_536;

type OrderedHelpers = (Vec<(u32, Function)>, u32, u32);

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

fn workspace_address(value: u64, memory_bytes: u64) -> Result<u32, RuntimeError> {
    if value > memory_bytes {
        return Err(RuntimeError::RuntimeTooLarge {
            limit: memory_bytes,
            observed: value,
        });
    }
    match i32::try_from(value) {
        Ok(address) => Ok(address as u32),
        Err(_) => Err(RuntimeError::RuntimeTooLarge {
            limit: memory_bytes,
            observed: value,
        }),
    }
}

/// Registers each codec primitive plus the iterative decoder as a `HelperGraph`
/// node with its real dependencies, orders them deterministically (BTree-backed),
/// and returns the emitted bodies with their type indices in dependency-first
/// order together with the decoder's module function index.
fn ordered_helper_functions(
    scratch: u32,
    sort_base: u32,
    caps: RuntimeCaps,
) -> Result<OrderedHelpers, RuntimeError> {
    const TYPE_RANGE: u32 = 0;
    const TYPE_SMALL: u32 = 1;
    const TYPE_COMPARE: u32 = 2;
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
    let index_of = |helper: HelperId| {
        let position = order
            .iter()
            .position(|&candidate| candidate == helper)
            .expect("registered helper is in order");
        1 + u32::try_from(position).expect("helper count fits u32")
    };
    let decode_params = DecodeEmit {
        scratch,
        raise_index: error_raiser_index,
        alloc_index,
        check_range_index: index_of(range),
        read_byte_index: index_of(read_byte),
        read_be_index: index_of(read_be),
        compare_index: index_of(compare),
        utf8_index: index_of(utf8),
        store_cell_index: index_of(store),
        caps,
    };
    let encode_params = EncodeEmit {
        scratch,
        sort_base,
        raise_index: error_raiser_index,
        append_index: index_of(append),
        compare_index: index_of(compare),
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
    let decode_index = index_of(decode);
    let encode_index = index_of(encode);
    Ok((functions, decode_index, encode_index))
}

/// Assembles the complete M1 runtime module: custom ABI/origin sections, type,
/// import (error raiser at index 0), function, memory, global, export and code
/// sections, plus one active data segment carrying the static bytes.
pub fn emit_runtime_module(
    static_bytes: &[u8],
    caps: RuntimeCaps,
) -> Result<Vec<u8>, RuntimeError> {
    validate_caps(&caps)?;
    let null_cell = validate_cell_codec()?;

    let static_len = u64::try_from(static_bytes.len()).unwrap_or(u64::MAX);
    if static_len > MAX_STATIC_BYTES {
        return Err(RuntimeError::RuntimeTooLarge {
            limit: MAX_STATIC_BYTES,
            observed: static_len,
        });
    }
    // Workspace budget above the 64 KiB frame stack: decoder cell slots, the
    // encoder pair-index window and the reject scratch, all below the heap.
    let cell_slot_total = caps
        .cells
        .checked_mul(CELL_SLOT_BYTES)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let sort_slot_total = caps
        .entries
        .checked_mul(SORT_INDEX_BYTES)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let cell_bytes = cell_slot_total
        .checked_add(sort_slot_total)
        .and_then(|total| total.checked_add(SCRATCH_BYTES))
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let layout = Layout::calculate(static_len, cell_bytes)?;

    const TYPE_VOID_THREE_I32: u32 = 4;
    const TYPE_ALLOC: u32 = 7;
    const TYPE_CALL: u32 = 8;

    let memory_bytes = layout.maximum_pages * PAGE_BYTES;
    let cell_base = layout
        .workspace_start
        .checked_add(u64::from(FRAME_COUNT * FRAME_BYTES))
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let sort_base = cell_base
        .checked_add(cell_slot_total)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let scratch = sort_base
        .checked_add(sort_slot_total)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let context_base = workspace_address(layout.workspace_start, memory_bytes)?;
    // frame 0 sits one FRAME_BYTES below this base, above the context header
    let frame_base = context_base + CONTEXT_HEADER_BYTES;
    let cell_base = workspace_address(cell_base, memory_bytes)?;
    let sort_base = workspace_address(sort_base, memory_bytes)?;
    let scratch = workspace_address(scratch, memory_bytes)?;

    let (helper_functions, decode_index, encode_index) =
        ordered_helper_functions(scratch, sort_base, caps)?;
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

    let mut types = TypeSection::new();
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I32]);
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I64]);
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32], []);
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32, ValType::I64], []);
    types.ty().function([ValType::I32, ValType::I32], []);
    types.ty().function([ValType::I32], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I64]);
    types.ty().function([ValType::I32], []);

    let mut imports = ImportSection::new();
    imports.import(
        "shamir_host",
        "raise_error",
        EntityType::Function(TYPE_VOID_THREE_I32),
    );

    let mut functions = FunctionSection::new();
    for (type_index, _) in &helper_functions {
        functions.function(*type_index);
    }
    functions.function(TYPE_VOID_THREE_I32);
    functions.function(TYPE_ALLOC);
    functions.function(TYPE_CALL);

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: layout.minimum_pages,
        maximum: Some(layout.maximum_pages),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut globals = GlobalSection::new();
    // two mutable i32 globals per design §5: heap_next at index 0 (the
    // allocator's heap pointer, initialized to the heap base) and call_state
    // at index 1 (the wrapper re-entry guard, initialized to zero)
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(i32::try_from(layout.heap_base).expect("heap base fits i32")),
    );
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
    exports.export(EXPORT_ALLOC, ExportKind::Func, alloc_index);
    exports.export(EXPORT_CALL, ExportKind::Func, call_index);

    let mut code = CodeSection::new();
    for (_, function) in &helper_functions {
        code.function(function);
    }
    code.function(&emit_error_raiser(0, context_base));
    code.function(&emit_allocator(layout, 0, 0));
    code.function(&emit_call_wrapper(
        decode_index,
        encode_index,
        alloc_index,
        error_raiser_index,
        WrapperBases {
            context: context_base,
            frame: frame_base,
            cell: cell_base,
            scratch,
            output_cap: i32::try_from(caps.output_bytes)
                .expect("validated caps fit the address space") as u32,
        },
        NullCell {
            tag: null_cell_tag,
            aux: null_cell_aux,
            payload: null_cell_payload,
        },
    ));

    let mut data = DataSection::new();
    data.active(
        0,
        &ConstExpr::i32_const(i32::try_from(STATIC_START).expect("static start fits i32")),
        static_bytes.to_vec(),
    );

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
    // code-section budget: encode the fully built section and enforce the
    // design's 64 KiB runtime-code cap (static data keeps its own 4 MiB cap
    // above; the old whole-module check is replaced by this one)
    let mut code_bytes = Vec::new();
    code.encode(&mut code_bytes);
    let code_observed = u64::try_from(code_bytes.len()).unwrap_or(u64::MAX);
    if code_observed > CODE_BUDGET_BYTES {
        return Err(RuntimeError::RuntimeTooLarge {
            limit: CODE_BUDGET_BYTES,
            observed: code_observed,
        });
    }
    Ok(bytes)
}

/// Emits the `shamir_call (i32,i32) -> i64` wrapper: it bounds-checks the input
/// range, initializes the workspace context (offsets per `codec/frames.rs`),
/// stores the null cell at the cell base, decodes the input envelope (codec
/// rejections surface through the error bridge), allocates the output buffer,
/// runs the iterative encoder over the decoded root cell and returns the
/// envelope as `(ptr << 32) | len`.
struct WrapperBases {
    context: u32,
    frame: u32,
    cell: u32,
    scratch: u32,
    /// Output-buffer capacity stored at context offset 52; append_byte reads
    /// it as the defensive append bound.
    output_cap: u32,
}

struct NullCell {
    tag: i32,
    aux: i32,
    payload: i64,
}

fn emit_call_wrapper(
    decode_index: u32,
    encode_index: u32,
    alloc_index: u32,
    raise_index: u32,
    bases: WrapperBases,
    null_cell: NullCell,
) -> Function {
    let mut function = Function::new([(4, ValType::I32)]);
    // memory_bytes = memory.size * 64 KiB (local 2)
    function.instruction(&Instruction::MemorySize(0));
    function.instruction(&Instruction::I32Const(WASM_PAGE_BYTES as i32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::LocalSet(2));
    // input pointer/length signed checks
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32LtS);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32LtS);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // wrap-free end = ptr + len (local 3), bounded by memory_bytes
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalTee(3));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // second-call guard: a re-entry with call_state set raises kind 4
    // ("reentry", 7 ASCII bytes packed into one I64Store) and never returns;
    // the flag is set only after every early exit is passed
    function.instruction(&Instruction::GlobalGet(1));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    {
        let message = b"reentry";
        let mut packed = [0u8; 8];
        packed[..message.len()].copy_from_slice(message);
        function.instruction(&Instruction::I32Const(bases.scratch as i32));
        function.instruction(&Instruction::I64Const(i64::from_le_bytes(packed)));
        function.instruction(&Instruction::I64Store(store_offset(0)));
    }
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I32Const(7));
    function.instruction(&Instruction::Call(raise_index));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::GlobalSet(1));
    // context base (local 4) and zeroed header; the frame stack base stored
    // in the context sits CONTEXT_HEADER_BYTES above so frame 0 stays clear
    // of the header (the encoder reads the root cell pointer from it)
    function.instruction(&Instruction::I32Const(bases.context as i32));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(CONTEXT_RESERVED_OFFSET as i32));
    function.instruction(&Instruction::MemoryFill(0));
    // input pointer/length
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Store(store_offset(
        CONTEXT_INPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Store(store_offset(
        CONTEXT_INPUT_LEN_OFFSET,
    )));
    // frame/cell/scratch bases
    for (field_offset, value) in [
        (CONTEXT_FRAME_BASE_OFFSET, bases.frame),
        (CONTEXT_CELL_BASE_OFFSET, bases.cell),
        (CONTEXT_SCRATCH_OFFSET, bases.scratch),
        (CONTEXT_OUTPUT_CAP_OFFSET, bases.output_cap),
    ] {
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::I32Const(value as i32));
        function.instruction(&Instruction::I32Store(store_offset(field_offset)));
    }
    // null cell at the cell base (tag/aux/payload from the runtime codec)
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I32Const(null_cell.tag));
    function.instruction(&Instruction::I32Store(store_offset(0)));
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I32Const(null_cell.aux));
    function.instruction(&Instruction::I32Store(store_offset(4)));
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I64Const(null_cell.payload));
    function.instruction(&Instruction::I64Store(store_offset(8)));
    // decode the root envelope; codec rejections raise kind 3 and never return
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(decode_index));
    // allocate the output buffer sized by the validated output cap; the same
    // byte count is stored as the context capacity the encoder budget reads
    function.instruction(&Instruction::I32Const(
        i32::try_from(bases.output_cap).expect("output cap fits i32"),
    ));
    function.instruction(&Instruction::Call(alloc_index));
    function.instruction(&Instruction::LocalSet(5));
    // record the output pointer and a zero length before encoding
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::I32Store(store_offset(
        CONTEXT_OUTPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Store(store_offset(
        CONTEXT_OUTPUT_LEN_OFFSET,
    )));
    // encode the decoded root cell into the output buffer
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(encode_index));
    // result = (ptr << 32) | len, both read back from the context
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(store_offset(
        CONTEXT_OUTPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Const(32));
    function.instruction(&Instruction::I64Shl);
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(store_offset(
        CONTEXT_OUTPUT_LEN_OFFSET,
    )));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::End);
    function
}

fn store_offset(bytes: u32) -> MemArg {
    MemArg {
        offset: u64::from(bytes),
        align: 2,
        memory_index: 0,
    }
}
