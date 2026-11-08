use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

/// `(left:i32,left_len:i32,right:i32,right_len:i32) -> i32`; caller validates ranges.
pub(crate) fn emit_compare_bytes() -> Function {
    let mut function = Function::new([(3, ValType::I32), (1, ValType::I32)]);
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(5));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(6));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::LocalGet(6));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::LocalGet(6));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::End);
    function
}

/// `(ptr:i32,end:i32) -> i32`; returns the byte or traps when the cursor is out of range.
pub(crate) fn emit_read_byte() -> Function {
    let mut function = Function::new([]);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32LeU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::End);
    function
}

/// `(ptr:i32,end:i32,count:i32) -> i64`; big-endian n-byte read, traps when short.
pub(crate) fn emit_read_be() -> Function {
    let mut function = Function::new([(1, ValType::I32), (1, ValType::I64)]);
    function.instruction(&Instruction::I64Const(0));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(3));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Const(8));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // acc = (acc << 8) | byte
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I64Const(8));
    function.instruction(&Instruction::I64Shl);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(3));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::End);
    function
}
