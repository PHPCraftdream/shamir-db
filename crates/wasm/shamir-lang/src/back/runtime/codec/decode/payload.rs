use wasm_encoder::{BlockType, Function, Instruction};

use super::reject::{reject_sequence, FAIL_KEY, FAIL_KEYS, FAIL_STR, FAIL_TRUNC, FAIL_UTF8};
use super::root::{
    cap32, DecodeEmit, L_AUX, L_CURSOR, L_END, L_ISKEY, L_KEYBYTES, L_LEN, L_PAYLOAD, L_TAG,
    TAG_BYTES, TAG_STRING,
};

/// Truncation guard: rejects when fewer than `count` bytes remain.
pub(super) fn need_bytes(function: &mut Function, params: &DecodeEmit, count: i32) {
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Const(count));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_TRUNC);
    function.instruction(&Instruction::End);
}

/// Reads `count` big-endian bytes through read_be into the payload local.
pub(super) fn be_read_const(function: &mut Function, params: &DecodeEmit, count: i32) {
    need_bytes(function, params, count);
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::I32Const(count));
    function.instruction(&Instruction::Call(params.read_be_index));
    function.instruction(&Instruction::LocalSet(L_PAYLOAD));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::I32Const(count));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CURSOR));
}

/// Reads local 10 big-endian bytes through read_be into the payload local.
pub(super) fn be_read_len(function: &mut Function, params: &DecodeEmit) {
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_TRUNC);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::Call(params.read_be_index));
    function.instruction(&Instruction::LocalSet(L_PAYLOAD));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CURSOR));
}

/// Reads a big-endian length/count into local 10 and advances the cursor.
pub(super) fn be_len(function: &mut Function, params: &DecodeEmit, count: i32) {
    need_bytes(function, params, count);
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::I32Const(count));
    function.instruction(&Instruction::Call(params.read_be_index));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(L_LEN));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::I32Const(count));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CURSOR));
}

/// String leaf: caps, key-byte budget and UTF-8 validity all precede the heap
/// allocation. Length is in local 10.
pub(super) fn str_payload(function: &mut Function, params: &DecodeEmit) {
    let caps = &params.caps;
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Const(cap32(caps.string_bytes)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_STR);
    function.instruction(&Instruction::End);
    // remaining input must cover the payload before anything allocates
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_TRUNC);
    function.instruction(&Instruction::End);
    // cumulative map-key bytes are charged for key strings only
    function.instruction(&Instruction::LocalGet(L_ISKEY));
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(L_KEYBYTES));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_KEYBYTES));
    function.instruction(&Instruction::LocalGet(L_KEYBYTES));
    function.instruction(&Instruction::I32Const(cap32(caps.key_bytes)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_KEYS);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::Call(params.utf8_index));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_UTF8);
    function.instruction(&Instruction::End);
    blob_cell(function, params, TAG_STRING);
}

/// Binary leaf: a bin key rejects before allocation; no UTF-8 validation.
/// Length is in local 10.
pub(super) fn bin_payload(function: &mut Function, params: &DecodeEmit) {
    let caps = &params.caps;
    function.instruction(&Instruction::LocalGet(L_ISKEY));
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_KEY);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Const(cap32(caps.string_bytes)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_STR);
    function.instruction(&Instruction::End);
    // remaining input must cover the payload before the heap copy
    function.instruction(&Instruction::LocalGet(L_END));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_TRUNC);
    function.instruction(&Instruction::End);
    blob_cell(function, params, TAG_BYTES);
}

/// Allocates the heap copy through the allocator, copies the payload bytes
/// with memory.copy (pops dst, src, size — pushed in that order), advances
/// the cursor and leaves tag/aux/payload for the shared leaf store.
fn blob_cell(function: &mut Function, params: &DecodeEmit, tag: i32) {
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::Call(params.alloc_index));
    function.instruction(&Instruction::LocalSet(11));
    // memory.copy(dst = copy, src = cursor, size = len)
    function.instruction(&Instruction::LocalGet(11));
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    function.instruction(&Instruction::LocalGet(L_CURSOR));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CURSOR));
    function.instruction(&Instruction::I32Const(tag));
    function.instruction(&Instruction::LocalSet(L_TAG));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::LocalSet(L_AUX));
    function.instruction(&Instruction::LocalGet(11));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::LocalSet(L_PAYLOAD));
}
