//! Emission of the checked-call host bridge using shared database codec helpers.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::emit::module::{EmissionPlan, EmitError};
use crate::back::runtime::{
    CONTEXT_INPUT_LEN_OFFSET, CONTEXT_INPUT_PTR_OFFSET, CONTEXT_OUTPUT_LEN_OFFSET,
    CONTEXT_OUTPUT_PTR_OFFSET,
};

use super::implementation::{emit_context_init, CONTEXT_DB_BUFFER_OFFSET, DB_BUFFER_BYTES};

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

/// Emits `(name cell, signature cell, args cell) -> typed result cell`.
pub(crate) fn emit_checked_call_helper(
    plan: &EmissionPlan,
    import_index: u32,
) -> Result<Function, EmitError> {
    let bases = plan.wrapper_bases();
    let ctx = bases.context;
    let mut function = Function::new([(4, ValType::I32), (1, ValType::I64)]);
    let buffer = 3;
    let arg_ptr = 4;
    let arg_len = 5;
    let cell = 6;
    let packed = 7;
    // Encode argument map using shared root codec into the capped DB buffer.
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
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_OUTPUT_LEN_OFFSET)));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Store(mem32(0)));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::Call(plan.encode_index));
    function.instruction(&Instruction::LocalGet(buffer));
    function.instruction(&Instruction::LocalSet(arg_ptr));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_OUTPUT_LEN_OFFSET)));
    function.instruction(&Instruction::LocalSet(arg_len));
    // name ptr/len and signature ptr/len come from static STRING/BYTES cells.
    // The codec supports an empty map: zero pairs encodes as MessagePack {}.
    // Each cell stores length at +4 and pointer in payload low word.
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalGet(arg_ptr));
    function.instruction(&Instruction::LocalGet(arg_len));
    function.instruction(&Instruction::Call(import_index));
    function.instruction(&Instruction::LocalTee(packed));
    function.instruction(&Instruction::I64Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    // Zero is never a nullable result: raise kind 4 host-contract failure.
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Const(plan.msg_arg as i32));
    function.instruction(&Instruction::I32Const(3));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
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
    // Validate the decoded root by projecting a fresh heap copy, retaining its value for the caller.
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
    Ok(function)
}
