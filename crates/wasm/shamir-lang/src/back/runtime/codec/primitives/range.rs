use wasm_encoder::{BlockType, Function, Instruction, ValType};

/// `(ptr:i32, len:i32, memory_bytes:i32) -> i32`; traps on signed/OOB ranges.
pub(crate) fn emit_check_range() -> Function {
    let mut function = Function::new([(3, ValType::I32)]);
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
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::End);
    function
}
