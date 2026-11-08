use wasm_encoder::{Function, Instruction, MemArg};

/// `(dst:i32,tag:i32,aux:i32,payload:i64) -> ()`, stores a 16-byte cell.
pub(crate) fn emit_store_cell() -> Function {
    let mut function = Function::new([]);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I64Store(MemArg {
        offset: 8,
        align: 3,
        memory_index: 0,
    }));
    function.instruction(&Instruction::End);
    function
}

/// `(dst:i32,src:i32,count:i32) -> ()`, overlap-safe 16-byte cell copy.
pub(crate) fn emit_copy_cells() -> Function {
    let mut function = Function::new([]);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    function.instruction(&Instruction::End);
    function
}
