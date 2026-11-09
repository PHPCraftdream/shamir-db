//! Stage-2 direct database call helpers: one generated Wasm function per
//! host operation (`db_get`, `db_insert`, `db_query`).
//!
//! Signature `(table cell: i32, argument cell: i32) -> i32`; the result is a
//! fresh cell holding the DECODED root value of the host's answer (the typed
//! projection runs afterwards in the call site's projection helper). Steps:
//!
//! 1. read the table name pointer/length from the string cell;
//! 2. unless the argument is the null cell of a `db_query` (zero filter
//!    length means no filter), encode the argument cell into a lazily
//!    allocated per-execution buffer (context slot) through the shared encoder;
//! 3. call the host import; a zero answer is the null cell;
//! 4. decode the packed answer with the shared decoder into the context
//!    scratch cell and copy it into a fresh heap cell (the decoder's cell
//!    slots are reused by the next call).
//!
//! Both the encoder and the decoder keep their state in the runtime context,
//! so every phase re-initialises it exactly like the call wrapper does.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::emit::abi::WrapperBases;
use crate::back::emit::module::{EmissionPlan, EmitError};
use crate::back::ir::DbOp;
use crate::back::runtime::{
    CONTEXT_CELL_BASE_OFFSET, CONTEXT_FRAME_BASE_OFFSET, CONTEXT_INPUT_LEN_OFFSET,
    CONTEXT_INPUT_PTR_OFFSET, CONTEXT_KEEP_ORDER_OFFSET, CONTEXT_OUTPUT_CAP_OFFSET,
    CONTEXT_OUTPUT_LEN_OFFSET, CONTEXT_OUTPUT_PTR_OFFSET, CONTEXT_RESERVED_OFFSET,
    CONTEXT_SCRATCH_OFFSET,
};

/// Capacity of the per-execution argument buffer: the encoder raises its
/// output-cap error (kind 3) when a document or filter does not fit.
pub(crate) const DB_BUFFER_BYTES: u32 = 1_024 * 1_024;

/// Context offset of the lazily allocated argument buffer pointer. It sits
/// above the header bytes the context re-initialisation zeroes, so it
/// survives across calls within one execution (module admission allows only
/// two globals, so the pointer lives in the context instead).
pub(crate) const CONTEXT_DB_BUFFER_OFFSET: u32 = 56;

/// Stable cell tag of the null cell (mirrors `runtime::CellTag::Null`).
const TAG_NULL: i32 = 1;

/// Host import name of one operation (module `shamir_host`).
pub(crate) fn host_import_name(op: DbOp) -> &'static str {
    match op {
        DbOp::Get => "db_get",
        DbOp::Insert => "db_insert",
        DbOp::Query => "db_query",
    }
}

fn mem32(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

fn mem64(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 3,
        memory_index: 0,
    }
}

fn store_i32(function: &mut Function, ctx: u32, offset: u32, value: u32) {
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Const(value as i32));
    function.instruction(&Instruction::I32Store(mem32(offset)));
}

/// Re-initialises the runtime context like the call wrapper's prologue: the
/// header is zeroed, then the frame/cell/scratch bases and the output cap
/// are stored. Shared by the helpers and the wrapper's pre-encode reset.
pub(crate) fn emit_context_init(function: &mut Function, bases: &WrapperBases, output_cap: u32) {
    function.instruction(&Instruction::I32Const(bases.context as i32));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(CONTEXT_RESERVED_OFFSET as i32));
    function.instruction(&Instruction::MemoryFill(0));
    store_i32(
        function,
        bases.context,
        CONTEXT_FRAME_BASE_OFFSET,
        bases.frame,
    );
    store_i32(
        function,
        bases.context,
        CONTEXT_CELL_BASE_OFFSET,
        bases.cell,
    );
    store_i32(
        function,
        bases.context,
        CONTEXT_SCRATCH_OFFSET,
        bases.scratch,
    );
    store_i32(
        function,
        bases.context,
        CONTEXT_OUTPUT_CAP_OFFSET,
        output_cap,
    );
}

/// Copies the 16-byte cell at `from_local` into the 16 bytes at the absolute
/// address `to_addr`.
fn copy_cell_to_address(function: &mut Function, to_addr: u32, from_local: u32) {
    for offset in [0u32, 4] {
        function.instruction(&Instruction::I32Const(to_addr as i32));
        function.instruction(&Instruction::LocalGet(from_local));
        function.instruction(&Instruction::I32Load(mem32(offset)));
        function.instruction(&Instruction::I32Store(mem32(offset)));
    }
    function.instruction(&Instruction::I32Const(to_addr as i32));
    function.instruction(&Instruction::LocalGet(from_local));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Store(mem64(8)));
}

/// Emits the helper of one operation. `import_index` is the host import's
/// function index.
pub(crate) fn emit_db_call_helper(
    plan: &EmissionPlan,
    op: DbOp,
    import_index: u32,
) -> Result<Function, EmitError> {
    let bases = plan.wrapper_bases();
    let ctx = bases.context;
    // params: 0 table cell, 1 argument cell; locals follow.
    let table_ptr: u32 = 2;
    let table_len: u32 = 3;
    let arg_ptr: u32 = 4;
    let arg_len: u32 = 5;
    let buffer: u32 = 6;
    let cell: u32 = 7;
    let packed: u32 = 8;
    let mut function = Function::new([(6, ValType::I32), (1, ValType::I64)]);

    // table name pointer (payload low) and length (aux)
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(table_ptr));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(table_len));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(arg_ptr));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(arg_len));

    // encode the argument (a null query filter sends zero bytes)
    let conditional = op == DbOp::Query;
    if conditional {
        function.instruction(&Instruction::LocalGet(1));
        function.instruction(&Instruction::I32Load(mem32(0)));
        function.instruction(&Instruction::I32Const(TAG_NULL));
        function.instruction(&Instruction::I32Ne);
        function.instruction(&Instruction::If(BlockType::Empty));
    }
    emit_context_init(&mut function, &bases, DB_BUFFER_BYTES);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_DB_BUFFER_OFFSET)));
    function.instruction(&Instruction::LocalTee(buffer));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(DB_BUFFER_BYTES as i32));
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(buffer));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_DB_BUFFER_OFFSET)));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_OUTPUT_PTR_OFFSET)));
    store_i32(&mut function, ctx, CONTEXT_OUTPUT_LEN_OFFSET, 0);
    copy_cell_to_address(&mut function, bases.scratch, 1);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::Call(plan.encode_index));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::LocalSet(arg_ptr));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_OUTPUT_LEN_OFFSET)));
    function.instruction(&Instruction::LocalSet(arg_len));
    if conditional {
        function.instruction(&Instruction::End);
    }

    // host call; zero is "absent" and reads as the null cell
    function.instruction(&Instruction::LocalGet(table_ptr));
    function.instruction(&Instruction::LocalGet(table_len));
    function.instruction(&Instruction::LocalGet(arg_ptr));
    function.instruction(&Instruction::LocalGet(arg_len));
    function.instruction(&Instruction::Call(import_index));
    function.instruction(&Instruction::LocalTee(packed));
    function.instruction(&Instruction::I64Eqz);
    function.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    function.instruction(&Instruction::I32Const(bases.null as i32));
    function.instruction(&Instruction::Else);
    // decode the packed answer into the context scratch cell
    emit_context_init(&mut function, &bases, bases.output_cap);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(packed));
    function.instruction(&Instruction::I64Const(32));
    function.instruction(&Instruction::I64ShrU);
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_INPUT_PTR_OFFSET)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(packed));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_INPUT_LEN_OFFSET)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::Call(plan.decode_index));
    // fresh cell holding the decoded root (the scratch slot is reused)
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(cell));
    for offset in [0u32, 4] {
        function.instruction(&Instruction::LocalGet(cell));
        function.instruction(&Instruction::I32Const(bases.scratch as i32));
        function.instruction(&Instruction::I32Load(mem32(offset)));
        function.instruction(&Instruction::I32Store(mem32(offset)));
    }
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    Ok(function)
}

// ---------------------------------------------------------------------------
// Typed builder helpers (`batch_read`/`batch_insert`, `db_execute`,
// `response_rows`/`response_inserted`)
// ---------------------------------------------------------------------------

/// Cap on operations per batch plan (the design `max_queries`).
const MAX_BATCH_OPERATIONS: i32 = 50;
/// Longest alias accepted by `batch_read`/`batch_insert`.
const MAX_ALIAS_BYTES: i32 = 64;
/// Cell tags the helpers read (mirror `runtime::CellTag`).
const TAG_STRING: i32 = 6;
const TAG_DYNAMIC_MAP: i32 = 14;
/// Stable cell tag of the opaque response cell (mirrors `CellTag::Response`).
const RESPONSE_TAG: i32 = 13;
/// Byte length of the response keys baked into the static image.
const RESULTS_KEY_LEN: u32 = 7;
const RECORDS_KEY_LEN: u32 = 7;
/// Number of cells in the BatchRequest record (`id` .. `result_encoding`).
const REQUEST_FIELDS: u32 = 6;
/// Field position of `queries` in the BatchRequest record.
const QUERIES_FIELD: u32 = 2;

fn raise_with(function: &mut Function, plan: &EmissionPlan, kind: i32, message: u32, len: u32) {
    function.instruction(&Instruction::I32Const(kind));
    function.instruction(&Instruction::I32Const(message as i32));
    function.instruction(&Instruction::I32Const(len as i32));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
}

/// `(batch cell, alias cell, entry cell) -> new batch cell`: validates the
/// alias (1..=64 ASCII identifier bytes, unique) and the operation cap, then
/// returns a copy of the plan whose `queries` map has the new `(alias,
/// entry)` pair appended; the input plan is never mutated.
pub(crate) fn emit_batch_append_helper(plan: &EmissionPlan) -> Result<Function, EmitError> {
    let aptr: u32 = 3;
    let alen: u32 = 4;
    let region_old: u32 = 5;
    let queries: u32 = 6;
    let count: u32 = 7;
    let pairs_old: u32 = 8;
    let index: u32 = 9;
    let pairs_new: u32 = 10;
    let region_new: u32 = 11;
    let newcell: u32 = 12;
    let ch: u32 = 13;
    let key: u32 = 14;
    let mut function = Function::new([(12, ValType::I32)]);
    let alias_len = 5u32;

    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(aptr));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(alen));
    // length 1..=64
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::I32Const(MAX_ALIAS_BYTES));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::I32Or);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 2, plan.msg_alias, alias_len);
    function.instruction(&Instruction::End);
    // every byte: letter, underscore, or (not first) digit
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(aptr));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(ch));
    // lower: ch - 'a' <=u 25
    function.instruction(&Instruction::LocalGet(ch));
    function.instruction(&Instruction::I32Const(97));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Const(25));
    function.instruction(&Instruction::I32LeU);
    // upper: ch - 'A' <=u 25
    function.instruction(&Instruction::LocalGet(ch));
    function.instruction(&Instruction::I32Const(65));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Const(25));
    function.instruction(&Instruction::I32LeU);
    function.instruction(&Instruction::I32Or);
    // underscore
    function.instruction(&Instruction::LocalGet(ch));
    function.instruction(&Instruction::I32Const(95));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::I32Or);
    // digit when not the first byte
    function.instruction(&Instruction::LocalGet(ch));
    function.instruction(&Instruction::I32Const(48));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Const(9));
    function.instruction(&Instruction::I32LeU);
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::I32Or);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 2, plan.msg_alias, alias_len);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    // the existing queries map: region slot 2 of the plan record
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(region_old));
    function.instruction(&Instruction::LocalGet(region_old));
    function.instruction(&Instruction::I32Const((QUERIES_FIELD * 16) as i32));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(queries));
    function.instruction(&Instruction::LocalGet(queries));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(count));
    function.instruction(&Instruction::LocalGet(queries));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(pairs_old));
    // operation cap
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Const(MAX_BATCH_OPERATIONS));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 2, plan.msg_bound, 5);
    function.instruction(&Instruction::End);
    // duplicate alias
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(pairs_old));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(key));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(aptr));
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::Call(plan.compare_index));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 2, plan.msg_alias, alias_len);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    // new pairs region: old pairs, then the new (alias, entry) pair
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Const(5));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(pairs_new));
    function.instruction(&Instruction::LocalGet(pairs_new));
    function.instruction(&Instruction::LocalGet(pairs_old));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::Call(plan.copy_cells_index));
    function.instruction(&Instruction::LocalGet(pairs_new));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Const(5));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(key));
    for (from_local, to_offset) in [(1u32, 0u32), (2, 16)] {
        for offset in [0u32, 4] {
            function.instruction(&Instruction::LocalGet(key));
            function.instruction(&Instruction::LocalGet(from_local));
            function.instruction(&Instruction::I32Load(mem32(offset)));
            function.instruction(&Instruction::I32Store(mem32(to_offset + offset)));
        }
        function.instruction(&Instruction::LocalGet(key));
        function.instruction(&Instruction::LocalGet(from_local));
        function.instruction(&Instruction::I64Load(mem64(8)));
        function.instruction(&Instruction::I64Store(mem64(to_offset + 8)));
    }
    // new plan region: a copy of the six cells with `queries` replaced
    function.instruction(&Instruction::I32Const((REQUEST_FIELDS * 16) as i32));
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(region_new));
    function.instruction(&Instruction::LocalGet(region_new));
    function.instruction(&Instruction::LocalGet(region_old));
    function.instruction(&Instruction::I32Const(REQUEST_FIELDS as i32));
    function.instruction(&Instruction::Call(plan.copy_cells_index));
    function.instruction(&Instruction::LocalGet(region_new));
    function.instruction(&Instruction::I32Const((QUERIES_FIELD * 16) as i32));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(key));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I32Const(TAG_DYNAMIC_MAP));
    function.instruction(&Instruction::I32Store(mem32(0)));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::LocalGet(pairs_new));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Store(mem64(8)));
    // the new record cell: same tag/aux, descriptor kept, region replaced
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(newcell));
    for offset in [0u32, 4] {
        function.instruction(&Instruction::LocalGet(newcell));
        function.instruction(&Instruction::LocalGet(0));
        function.instruction(&Instruction::I32Load(mem32(offset)));
        function.instruction(&Instruction::I32Store(mem32(offset)));
    }
    function.instruction(&Instruction::LocalGet(newcell));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Const(-4_294_967_296));
    function.instruction(&Instruction::I64And);
    function.instruction(&Instruction::LocalGet(region_new));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(newcell));
    function.instruction(&Instruction::End);
    Ok(function)
}

/// `(request cell) -> response cell`: encodes the request record, calls the
/// host `db_execute` import and wraps the answer bytes `(ptr, len)` into an
/// opaque response cell (tag RESPONSE, aux = length, payload = pointer).
pub(crate) fn emit_db_execute_helper(
    plan: &EmissionPlan,
    import_index: u32,
) -> Result<Function, EmitError> {
    let bases = plan.wrapper_bases();
    let ctx = bases.context;
    let buffer: u32 = 1;
    let length: u32 = 2;
    let cell: u32 = 3;
    let packed: u32 = 4;
    let mut function = Function::new([(3, ValType::I32), (1, ValType::I64)]);
    emit_context_init(&mut function, &bases, DB_BUFFER_BYTES);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_DB_BUFFER_OFFSET)));
    function.instruction(&Instruction::LocalTee(buffer));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(DB_BUFFER_BYTES as i32));
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(buffer));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_DB_BUFFER_OFFSET)));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_OUTPUT_PTR_OFFSET)));
    store_i32(&mut function, ctx, CONTEXT_OUTPUT_LEN_OFFSET, 0);
    copy_cell_to_address(&mut function, bases.scratch, 0);
    // the request keeps the insertion order of its dynamic maps (`queries`)
    store_i32(&mut function, ctx, CONTEXT_KEEP_ORDER_OFFSET, 1);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::Call(plan.encode_index));
    store_i32(&mut function, ctx, CONTEXT_KEEP_ORDER_OFFSET, 0);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_OUTPUT_LEN_OFFSET)));
    function.instruction(&Instruction::LocalSet(length));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::LocalGet(length));
    function.instruction(&Instruction::Call(import_index));
    function.instruction(&Instruction::LocalSet(packed));
    // opaque response cell: tag RESPONSE, aux = answer length, payload = ptr
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::Call(plan.alloc_index));
    function.instruction(&Instruction::LocalSet(cell));
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::I32Const(RESPONSE_TAG));
    function.instruction(&Instruction::I32Store(mem32(0)));
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::LocalGet(packed));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::LocalGet(packed));
    function.instruction(&Instruction::I64Const(32));
    function.instruction(&Instruction::I64ShrU);
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::End);
    Ok(function)
}

/// `(map cell, key ptr, key len) -> child cell pointer or 0`: scans the pairs
/// of a decoded dynamic map for a string key.
pub(crate) fn emit_dynmap_find_helper(plan: &EmissionPlan) -> Result<Function, EmitError> {
    let count: u32 = 3;
    let pairs: u32 = 4;
    let index: u32 = 5;
    let key: u32 = 6;
    let mut function = Function::new([(4, ValType::I32)]);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(count));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(pairs));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(pairs));
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(key));
    // string key of the same length with equal bytes
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_STRING));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::Call(plan.compare_index));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(key));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::End);
    Ok(function)
}

/// `(response cell, alias cell) -> records list cell or the null cell`:
/// decodes the raw answer and walks `results -> alias -> records`; a missing
/// `results` map, a non-map level or a missing `records` list is a codec
/// failure (kind 3), a missing alias reads as null.
pub(crate) fn emit_response_get_helper(
    plan: &EmissionPlan,
    find_index: u32,
) -> Result<Function, EmitError> {
    let bases = plan.wrapper_bases();
    let ctx = bases.context;
    let aptr: u32 = 2;
    let alen: u32 = 3;
    let found: u32 = 4;
    let mut function = Function::new([(3, ValType::I32)]);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(aptr));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(alen));
    emit_context_init(&mut function, &bases, bases.output_cap);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_INPUT_PTR_OFFSET)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_INPUT_LEN_OFFSET)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::Call(plan.decode_index));
    // root must be a map
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_DYNAMIC_MAP));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 3, plan.msg_arg, 3);
    function.instruction(&Instruction::End);
    // results = root["results"]
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I32Const(plan.key_results as i32));
    function.instruction(&Instruction::I32Const(RESULTS_KEY_LEN as i32));
    function.instruction(&Instruction::Call(find_index));
    function.instruction(&Instruction::LocalTee(found));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 3, plan.msg_arg, 3);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(found));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_DYNAMIC_MAP));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 3, plan.msg_arg, 3);
    function.instruction(&Instruction::End);
    // entry = results[alias]; absent alias is null
    function.instruction(&Instruction::LocalGet(found));
    function.instruction(&Instruction::LocalGet(aptr));
    function.instruction(&Instruction::LocalGet(alen));
    function.instruction(&Instruction::Call(find_index));
    function.instruction(&Instruction::LocalTee(found));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    function.instruction(&Instruction::I32Const(bases.null as i32));
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(found));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_DYNAMIC_MAP));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 3, plan.msg_arg, 3);
    function.instruction(&Instruction::End);
    // records = entry["records"]
    function.instruction(&Instruction::LocalGet(found));
    function.instruction(&Instruction::I32Const(plan.key_records as i32));
    function.instruction(&Instruction::I32Const(RECORDS_KEY_LEN as i32));
    function.instruction(&Instruction::Call(find_index));
    function.instruction(&Instruction::LocalTee(found));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_with(&mut function, plan, 3, plan.msg_arg, 3);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(found));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    Ok(function)
}
