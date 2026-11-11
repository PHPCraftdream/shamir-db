//! Runtime validation of skipped batch-reference responses.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::emit::module::{static_address, EmissionPlan, EmitError};
use crate::back::emit::support::db::implementation::emit_context_init;
use crate::back::runtime::{CONTEXT_INPUT_LEN_OFFSET, CONTEXT_INPUT_PTR_OFFSET};

const TAG_BOOL: i32 = 2;
const TAG_DYNAMIC_MAP: i32 = 14;
const SKIPPED_KEY: &[u8] = b"skipped";
const SKIPPED_MESSAGE: &[u8] = b"batch reference did not resolve to the expected scalar";
const SKIPPED_MESSAGE_LEN: i32 = SKIPPED_MESSAGE.len() as i32;

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

/// Appends the guarded-response lookup key and stable kind-2 message.
pub(crate) fn append_guard_keys(image: &mut Vec<u8>, limit: u64) -> Result<(u32, u32), EmitError> {
    let key = append_static_bytes(image, SKIPPED_KEY, limit)?;
    let message = append_static_bytes(image, SKIPPED_MESSAGE, limit)?;
    Ok((key, message))
}

fn append_static_bytes(image: &mut Vec<u8>, bytes: &[u8], limit: u64) -> Result<u32, EmitError> {
    let offset = u64::try_from(image.len())
        .map_err(|_| EmitError::Internal("static image offset overflow"))?;
    let observed = offset
        .checked_add(bytes.len() as u64)
        .ok_or(EmitError::Internal("static image length overflow"))?;
    if observed > limit {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit,
            observed,
        });
    }
    let address = static_address(offset)?;
    image.extend_from_slice(bytes);
    Ok(address)
}

/// Emits `(response cell) -> same response cell`, validating every batch
/// result entry and raising if a guarded result was skipped.
pub(crate) fn emit_response_skip_check_helper(
    plan: &EmissionPlan,
    find_index: u32,
) -> Result<Function, EmitError> {
    let bases = plan.wrapper_bases();
    let ctx = bases.context;
    let count = 1;
    let pairs = 2;
    let index = 3;
    let pair = 4;
    let entry = 5;
    let skipped = 6;
    let mut function = Function::new([(6, ValType::I32)]);

    // Decode the opaque response payload while retaining its cell pointer in
    // parameter 0; the decoder reuses the context scratch cell.
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

    // The response root and its mandatory `results` value are dynamic maps.
    require_map(&mut function, plan, bases.scratch, false);
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I32Const(plan.key_results as i32));
    function.instruction(&Instruction::I32Const(7));
    function.instruction(&Instruction::Call(find_index));
    function.instruction(&Instruction::LocalTee(entry));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(&mut function, plan);
    function.instruction(&Instruction::End);
    require_map(&mut function, plan, entry, true);

    // Visit all results pairs. Each value must be an entry map; a present
    // `skipped` field must be boolean, and true is the reference failure.
    function.instruction(&Instruction::LocalGet(entry));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(count));
    function.instruction(&Instruction::LocalGet(entry));
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
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(pair));
    function.instruction(&Instruction::LocalGet(pair));
    function.instruction(&Instruction::LocalSet(entry));
    require_map(&mut function, plan, entry, true);
    function.instruction(&Instruction::LocalGet(entry));
    function.instruction(&Instruction::I32Const(plan.key_skipped as i32));
    function.instruction(&Instruction::I32Const(SKIPPED_KEY.len() as i32));
    function.instruction(&Instruction::Call(find_index));
    function.instruction(&Instruction::LocalTee(skipped));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(skipped));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_BOOL));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(&mut function, plan);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(skipped));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Eqz);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(2));
    function.instruction(&Instruction::I32Const(plan.msg_skipped as i32));
    function.instruction(&Instruction::I32Const(SKIPPED_MESSAGE_LEN));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(index));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(index));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::End);
    Ok(function)
}

fn require_map(function: &mut Function, plan: &EmissionPlan, cell: u32, is_local: bool) {
    if is_local {
        function.instruction(&Instruction::LocalGet(cell));
    } else {
        function.instruction(&Instruction::I32Const(cell as i32));
    }
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_DYNAMIC_MAP));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(function, plan);
    function.instruction(&Instruction::End);
}

fn raise_decode_error(function: &mut Function, plan: &EmissionPlan) {
    function.instruction(&Instruction::I32Const(3));
    function.instruction(&Instruction::I32Const(plan.msg_arg as i32));
    function.instruction(&Instruction::I32Const(3));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
}
