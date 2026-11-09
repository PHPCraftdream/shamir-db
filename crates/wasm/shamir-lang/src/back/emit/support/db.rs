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
    CONTEXT_INPUT_PTR_OFFSET, CONTEXT_OUTPUT_CAP_OFFSET, CONTEXT_OUTPUT_LEN_OFFSET,
    CONTEXT_OUTPUT_PTR_OFFSET, CONTEXT_RESERVED_OFFSET, CONTEXT_SCRATCH_OFFSET,
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
