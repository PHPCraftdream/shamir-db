use wasm_encoder::{BlockType, Function, Instruction, ValType};

use super::layout::{Layout, MAX_ADDRESS, PAGE_BYTES};

/// Emit a checked monotonic allocator body. The module owner declares its `(i32)->i32` type,
/// function, memory and mutable i32 heap global at the supplied indices.
pub(crate) fn emit_allocator(
    layout: Layout,
    memory_index: u32,
    heap_global_index: u32,
) -> Function {
    let mut body = Function::new([(4, ValType::I64)]);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32LtS);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::I32Const(
        i32::try_from(layout.heap_base).expect("heap base fits i32"),
    ));
    body.instruction(&Instruction::Else);

    body.instruction(&Instruction::GlobalGet(heap_global_index));
    body.instruction(&Instruction::I64ExtendI32U);
    body.instruction(&Instruction::LocalSet(1));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::I64ExtendI32U);
    body.instruction(&Instruction::I64Const(7));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalTee(2));
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::I64ExtendI32U);
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(2));
    body.instruction(&Instruction::I64Const(-8));
    body.instruction(&Instruction::I64And);
    body.instruction(&Instruction::LocalSet(2));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::LocalGet(2));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalTee(3));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::I64Const(
        i64::try_from(MAX_ADDRESS).expect("address cap fits i64"),
    ));
    body.instruction(&Instruction::I64GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::MemorySize(memory_index));
    body.instruction(&Instruction::I64ExtendI32U);
    body.instruction(&Instruction::I64Const(
        i64::try_from(PAGE_BYTES).expect("page size fits i64"),
    ));
    body.instruction(&Instruction::I64Mul);
    body.instruction(&Instruction::LocalSet(4));
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::LocalGet(4));
    body.instruction(&Instruction::I64GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::LocalGet(4));
    body.instruction(&Instruction::I64Sub);
    body.instruction(&Instruction::I64Const(
        i64::try_from(PAGE_BYTES - 1).expect("page size fits i64"),
    ));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::I64Const(16));
    body.instruction(&Instruction::I64ShrU);
    body.instruction(&Instruction::I32WrapI64);
    body.instruction(&Instruction::MemoryGrow(memory_index));
    body.instruction(&Instruction::I32Const(-1));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::I32WrapI64);
    body.instruction(&Instruction::GlobalSet(heap_global_index));
    body.instruction(&Instruction::LocalGet(1));
    body.instruction(&Instruction::I32WrapI64);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body
}
