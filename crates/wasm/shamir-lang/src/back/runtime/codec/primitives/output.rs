use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use super::super::frames::CONTEXT_OUTPUT_CAP_OFFSET;

/// `(context:i32,byte:i32) -> ()`; appends to the output buffer held at
/// context offsets ptr=8 / len=12, bounded by the capacity the wrapper
/// stores at offset 52.
pub(crate) fn emit_append_byte() -> Function {
    let mut function = Function::new([(2, ValType::I32)]);
    // len = context.len (offset 12)
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem(12)));
    function.instruction(&Instruction::LocalSet(2));
    // defensive bound: len >= context cap traps (the encoder pre-checks)
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem(CONTEXT_OUTPUT_CAP_OFFSET)));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // store byte at context.ptr (offset 8) + len; the store pops the value
    // from the top of the stack, so the address is pushed first
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem(8)));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    // store new len at offset 12
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Store(mem(12)));
    function.instruction(&Instruction::End);
    function
}

fn mem(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}
