use wasm_encoder::{Function, Instruction};

use super::root::{DecodeEmit, L_AUX, L_PAYLOAD, L_TAG, TAG_BOOL};

/// Rejection classes surfaced as codec errors (kind 3); the emitted fail path
/// writes the matching ASCII literal to the workspace scratch slot before
/// calling the error raiser.
pub const FAIL_EXT: i32 = 0;
pub const FAIL_RESERVED: i32 = 1;
pub const FAIL_UTF8: i32 = 2;
pub const FAIL_KEY: i32 = 3;
pub const FAIL_DUP: i32 = 4;
pub const FAIL_TRUNC: i32 = 5;
pub const FAIL_TRAIL: i32 = 6;
pub const FAIL_DEPTH: i32 = 7;
pub const FAIL_CELLS: i32 = 8;
pub const FAIL_CONTAINERS: i32 = 9;
pub const FAIL_ENTRIES: i32 = 10;
pub const FAIL_STR: i32 = 11;
pub const FAIL_KEYS: i32 = 12;
pub const FAIL_INPUT: i32 = 13;
pub const FAIL_U64: i32 = 14;
pub const FAIL_NONFINITE: i32 = 15;
pub const FAIL_LENGTH: i32 = 16;

/// Exact message bytes per rejection class; lengths are asserted in tests.
pub fn reject_message(code: i32) -> &'static [u8] {
    match code {
        FAIL_EXT => b"ext",
        FAIL_RESERVED => b"reserved",
        FAIL_UTF8 => b"utf8",
        FAIL_KEY => b"key",
        FAIL_DUP => b"dup",
        FAIL_TRUNC => b"trunc",
        FAIL_TRAIL => b"trail",
        FAIL_DEPTH => b"depth",
        FAIL_CELLS => b"cells",
        FAIL_CONTAINERS => b"cnt",
        FAIL_ENTRIES => b"ent",
        FAIL_STR => b"str",
        FAIL_KEYS => b"keys",
        FAIL_INPUT => b"input",
        FAIL_U64 => b"u64",
        FAIL_NONFINITE => b"nonfin",
        FAIL_LENGTH => b"outlen",
        _ => b"u64",
    }
}

/// Kind-3 raise: scratch := packed literal, then raise_error(3, scratch, len);
/// control never returns (the raiser ends in `unreachable`).
pub fn reject_sequence(function: &mut Function, params: &DecodeEmit, code: i32) {
    let message = reject_message(code);
    let mut packed = [0u8; 8];
    packed[..message.len()].copy_from_slice(message);
    function.instruction(&Instruction::I32Const(params.scratch as i32));
    function.instruction(&Instruction::I64Const(i64::from_le_bytes(packed)));
    function.instruction(&Instruction::I64Store(super::root::mem32(0)));
    function.instruction(&Instruction::I32Const(3));
    function.instruction(&Instruction::I32Const(params.scratch as i32));
    function.instruction(&Instruction::I32Const(message.len() as i32));
    function.instruction(&Instruction::Call(params.raise_index));
    function.instruction(&Instruction::Unreachable);
}

/// Bool leaf: tag/aux/payload locals for the shared leaf store.
pub fn leaf_bool(function: &mut Function, bit: i32) {
    function.instruction(&Instruction::I32Const(TAG_BOOL));
    function.instruction(&Instruction::LocalSet(L_TAG));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(L_AUX));
    function.instruction(&Instruction::I64Const(i64::from(bit)));
    function.instruction(&Instruction::LocalSet(L_PAYLOAD));
}
