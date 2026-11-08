use wasm_encoder::{BlockType, Function, Instruction, ValType};

use thiserror::Error;

use super::layout::LayoutError;
use crate::back::emit::HelperOrderError;

/// Errors raised while assembling the M1 runtime module.
///
/// Variants mirror the emission-time checks in `back/emit/module.rs`: the
/// emitted module size, the declared runtime caps and the cell codec template.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RuntimeError {
    #[error("runtime module exceeds its {limit}-byte budget: {observed} bytes")]
    RuntimeTooLarge { limit: u64, observed: u64 },
    #[error("runtime caps exceed the wasm32 address space")]
    CapsBeyondAddressSpace,
    #[error("runtime cap exceeds the design hard ceiling")]
    CapAboveCeiling,
    #[error("runtime cell template failed to round-trip")]
    CellTemplate,
    #[error("helper dependency graph produced an unknown node")]
    UnknownHelper,
    #[error("module layout rejected")]
    Layout(#[from] LayoutError),
    #[error("helper dependency order rejected")]
    HelperOrder(#[from] HelperOrderError),
}

/// `(kind:i32,ptr:i32,len:i32) -> ()`; validates the full pointer range against
/// the linear memory bound only. Message sanitation/truncation is not yet
/// emitted here (the host sanitizes within the declared length).
pub(crate) fn emit_error_raiser(_raise_error_index: u32, _workspace_start: u32) -> Function {
    // locals: 3 = end (i32), 4 = memory_bytes (i64); one declared local per type
    let mut function = Function::new([(1, ValType::I32), (1, ValType::I64)]);
    // ptr signed check
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32LtS);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // len signed check
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32LtS);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // end = ptr + len (local 3)
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(3));
    // wrap check: end unsigned < ptr traps
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // memory_bytes = memory.size * 65536 (local 4, i64)
    function.instruction(&Instruction::MemorySize(0));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Const(65536));
    function.instruction(&Instruction::I64Mul);
    function.instruction(&Instruction::LocalSet(4));
    // bound check: end (u64) > memory_bytes traps
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I64GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    // raise
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::Call(_raise_error_index));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function
}
