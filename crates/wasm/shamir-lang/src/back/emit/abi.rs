//! `shamir_call` export wrapper emission plus the wrapper base addresses and
//! null-cell template it needs. Split out of `emit/module.rs` so the scalar
//! call-bridge work extends this file instead of the assembler.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::ir::ParamProjection;
use crate::back::runtime::{
    CONTEXT_OUTPUT_LEN_OFFSET, CONTEXT_OUTPUT_PTR_OFFSET, CONTEXT_SCRATCH_OFFSET,
};

/// Linear-memory page size in bytes, mirrored from the layout arithmetic.
pub(crate) const WASM_PAGE_BYTES: u32 = 65_536;

/// Workspace addresses and bounds the wrapper bakes into the guest context.
#[derive(Clone, Copy)]
pub(crate) struct WrapperBases {
    pub context: u32,
    pub frame: u32,
    pub cell: u32,
    pub scratch: u32,
    /// Start of the encoder pair-index window inside the workspace.
    /// Output-buffer capacity stored at context offset 52; append_byte reads
    /// it as the defensive append bound.
    pub output_cap: u32,
}

pub(crate) struct NullCell {
    pub tag: i32,
    pub aux: i32,
    pub payload: i64,
}

pub(crate) fn emit_call_wrapper(
    decode_index: u32,
    encode_index: u32,
    alloc_index: u32,
    raise_index: u32,
    bases: WrapperBases,
    null_cell: NullCell,
) -> Function {
    let mut function = Function::new([(4, ValType::I32)]);
    // memory_bytes = memory.size * 64 KiB (local 2)
    function.instruction(&Instruction::MemorySize(0));
    function.instruction(&Instruction::I32Const(WASM_PAGE_BYTES as i32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::LocalSet(2));
    // input pointer/length signed checks
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
    // wrap-free end = ptr + len (local 3), bounded by memory_bytes
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
    // second-call guard: a re-entry with call_state set raises kind 4
    // ("reentry", 7 ASCII bytes packed into one I64Store) and never returns;
    // the flag is set only after every early exit is passed
    function.instruction(&Instruction::GlobalGet(1));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    {
        let message = b"reentry";
        let mut packed = [0u8; 8];
        packed[..message.len()].copy_from_slice(message);
        function.instruction(&Instruction::I32Const(bases.scratch as i32));
        function.instruction(&Instruction::I64Const(i64::from_le_bytes(packed)));
        function.instruction(&Instruction::I64Store(store_offset(0)));
    }
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I32Const(7));
    function.instruction(&Instruction::Call(raise_index));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::GlobalSet(1));
    // context base (local 4) and zeroed header; the frame stack base stored
    // in the context sits CONTEXT_HEADER_BYTES above so frame 0 stays clear
    // of the header (the encoder reads the root cell pointer from it)
    function.instruction(&Instruction::I32Const(bases.context as i32));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(
        crate::back::runtime::CONTEXT_RESERVED_OFFSET as i32,
    ));
    function.instruction(&Instruction::MemoryFill(0));
    // input pointer/length
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Store(store_offset(
        crate::back::runtime::CONTEXT_INPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Store(store_offset(
        crate::back::runtime::CONTEXT_INPUT_LEN_OFFSET,
    )));
    // frame/cell/scratch bases
    for (field_offset, value) in [
        (crate::back::runtime::CONTEXT_FRAME_BASE_OFFSET, bases.frame),
        (crate::back::runtime::CONTEXT_CELL_BASE_OFFSET, bases.cell),
        (crate::back::runtime::CONTEXT_SCRATCH_OFFSET, bases.scratch),
        (
            crate::back::runtime::CONTEXT_OUTPUT_CAP_OFFSET,
            bases.output_cap,
        ),
    ] {
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::I32Const(value as i32));
        function.instruction(&Instruction::I32Store(store_offset(field_offset)));
    }
    // null cell at the cell base (tag/aux/payload from the runtime codec)
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I32Const(null_cell.tag));
    function.instruction(&Instruction::I32Store(store_offset(0)));
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I32Const(null_cell.aux));
    function.instruction(&Instruction::I32Store(store_offset(4)));
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I64Const(null_cell.payload));
    function.instruction(&Instruction::I64Store(store_offset(8)));
    // decode the root envelope; codec rejections raise kind 3 and never return
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(decode_index));
    // allocate the output buffer sized by the validated output cap; the same
    // byte count is stored as the context capacity the encoder budget reads
    function.instruction(&Instruction::I32Const(
        i32::try_from(bases.output_cap).expect("output cap fits i32"),
    ));
    function.instruction(&Instruction::Call(alloc_index));
    function.instruction(&Instruction::LocalSet(5));
    // record the output pointer and a zero length before encoding
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::I32Store(store_offset(
        crate::back::runtime::CONTEXT_OUTPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Store(store_offset(
        crate::back::runtime::CONTEXT_OUTPUT_LEN_OFFSET,
    )));
    // encode the decoded root cell into the output buffer
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(encode_index));
    // result = (ptr << 32) | len, both read back from the context
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(store_offset(
        crate::back::runtime::CONTEXT_OUTPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Const(32));
    function.instruction(&Instruction::I64Shl);
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(store_offset(
        crate::back::runtime::CONTEXT_OUTPUT_LEN_OFFSET,
    )));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::End);
    function
}

fn store_offset(bytes: u32) -> MemArg {
    MemArg {
        offset: u64::from(bytes),
        align: 2,
        memory_index: 0,
    }
}

fn load_offset(bytes: u32) -> MemArg {
    store_offset(bytes)
}

fn load64_offset(bytes: u32) -> MemArg {
    MemArg {
        offset: u64::from(bytes),
        align: 3,
        memory_index: 0,
    }
}

/// Addresses (inside the static image) of the wrapper raise messages
/// appended in a fixed second block after the scalar messages.
pub(crate) struct WrapperMessages {
    /// Unknown or out-of-range envelope key.
    pub arg: u32,
    /// Positional and named carriers combined (numeric keys or `args` with
    /// named keys, or `args` with named keys).
    pub mixed: u32,
    /// Argument count does not match the entry arity.
    pub arity: u32,
    /// i32 parameter value out of range.
    pub i32: u32,
}

/// Stable cell tags the wrapper reads (mirrors `runtime::CellTag`).
const TAG_BOOL: i32 = 2;
const TAG_I32: i32 = 3;
const TAG_I64: i32 = 4;
const TAG_F64: i32 = 5;
const TAG_STRING: i32 = 6;
const TAG_LIST: i32 = 8;
/// little-endian i32 of `b"args"`.
const ARGS_WORD: i32 = 0x7367_7261;

/// Raises a kind-2 wrapper error and never returns (`unreachable`).
fn raise(function: &mut Function, raise_index: u32, message: u32, len: u32) {
    function.instruction(&Instruction::I32Const(2));
    function.instruction(&Instruction::I32Const(message as i32));
    function.instruction(&Instruction::I32Const(len as i32));
    function.instruction(&Instruction::Call(raise_index));
    function.instruction(&Instruction::Unreachable);
}

/// Shared inputs of the compiled-module `shamir_call` scalar wrapper.
pub(crate) struct WrapperInputs<'a> {
    pub decode_index: u32,
    pub encode_index: u32,
    pub alloc_index: u32,
    pub raise_index: u32,
    pub entry_index: u32,
    pub bases: WrapperBases,
    pub null_cell: NullCell,
    pub messages: WrapperMessages,
    pub param_kinds: &'a [ParamProjection],
    pub result_projection: Option<ParamProjection>,
    /// Entry parameter names as `(static address, byte length)` pairs, in
    /// declaration order; baked into the named-key scan.
    pub param_names: &'a [(u32, u32)],
}

/// Emits the compiled-module `shamir_call` wrapper: after the M1 prologue
/// (bounds checks, re-entry guard, context init, null cell, decode, output
/// buffer) it scans the decoded `DynamicMap` envelope for positional keys
/// `"0".."9"`, the `args` list or declared parameter names. Numeric keys are
/// authoritative when present (`args` alongside them is redundant and
/// ignored); without numeric keys, matching declared names select named
/// parameters (omitted optional parameters read as null). Each argument cell
/// is projected into the entry parameter space, the result is boxed back into
/// the context scratch cell and encoded.
///
/// Locals: 0/1 are the wrapper params, 2 `memory_bytes`, 3 scratch, 4
/// context, 5 output pointer, then one i32 per entry parameter, then the
/// scan/box temps, then one i64 payload temp.
pub(crate) fn emit_scalar_wrapper(inputs: WrapperInputs) -> Function {
    let WrapperInputs {
        decode_index,
        encode_index,
        alloc_index,
        raise_index,
        entry_index,
        bases,
        null_cell,
        messages,
        param_kinds,
        result_projection,
        param_names,
    } = inputs;
    let param_count = param_kinds.len();
    let param0: u32 = 6;
    let temp: u32 = param0.saturating_add(param_count as u32);
    // scan/box temps: i, count, seen, args_present, args, pair, vcell,
    // klen, kaddr, bbyte (also the name-compare flag), matched, boxed,
    // scratch, bool value, named_seen, mask, kcell.
    let i = temp;
    let count = temp + 1;
    let seen = temp + 2;
    let args_present = temp + 3;
    let args_cell = temp + 4;
    let pair = temp + 5;
    let vcell = temp + 6;
    let klen = temp + 7;
    let kaddr = temp + 8;
    let bbyte = temp + 9;
    let matched = temp + 10;
    let boxed = temp + 11;
    let scratch = temp + 12;
    let bool_value = temp + 13;
    let named_seen = temp + 14;
    let mask = temp + 15;
    let payload64: u32 = temp + 16;
    let kcell = temp + 17;

    let mut groups = vec![(4u32, ValType::I32)];
    if param_count > 0 {
        groups.push((param_count as u32, ValType::I32));
    }
    groups.push((17, ValType::I32));
    groups.push((1, ValType::I64));
    let mut function = Function::new(groups);

    // memory_bytes = memory.size * 64 KiB (local 2)
    function.instruction(&Instruction::MemorySize(0));
    function.instruction(&Instruction::I32Const(65_536));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::LocalSet(2));
    // input pointer/length signed checks and wrap-free end bound (local 3)
    for value in [0usize, 1usize] {
        function.instruction(&Instruction::LocalGet(value as u32));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32LtS);
        function.instruction(&Instruction::If(BlockType::Empty));
        function.instruction(&Instruction::Unreachable);
        function.instruction(&Instruction::End);
    }
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
    // re-entry guard: raise kind 4 ("reentry") and never return
    function.instruction(&Instruction::GlobalGet(1));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    {
        let message = b"reentry";
        let mut packed = [0u8; 8];
        packed[..message.len()].copy_from_slice(message);
        function.instruction(&Instruction::I32Const(bases.scratch as i32));
        function.instruction(&Instruction::I64Const(i64::from_le_bytes(packed)));
        function.instruction(&Instruction::I64Store(store_offset(0)));
    }
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Const(bases.scratch as i32));
    function.instruction(&Instruction::I32Const(7));
    function.instruction(&Instruction::Call(raise_index));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::GlobalSet(1));
    // context base (local 4) and zeroed header, then frame/cell/scratch caps
    function.instruction(&Instruction::I32Const(bases.context as i32));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(
        crate::back::runtime::CONTEXT_RESERVED_OFFSET as i32,
    ));
    function.instruction(&Instruction::MemoryFill(0));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Store(store_offset(
        crate::back::runtime::CONTEXT_INPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Store(store_offset(
        crate::back::runtime::CONTEXT_INPUT_LEN_OFFSET,
    )));
    for (field_offset, value) in [
        (crate::back::runtime::CONTEXT_FRAME_BASE_OFFSET, bases.frame),
        (crate::back::runtime::CONTEXT_CELL_BASE_OFFSET, bases.cell),
        (crate::back::runtime::CONTEXT_SCRATCH_OFFSET, bases.scratch),
        (
            crate::back::runtime::CONTEXT_OUTPUT_CAP_OFFSET,
            bases.output_cap,
        ),
    ] {
        function.instruction(&Instruction::LocalGet(4));
        function.instruction(&Instruction::I32Const(value as i32));
        function.instruction(&Instruction::I32Store(store_offset(field_offset)));
    }
    // null cell at the cell base (the `-> null` result reads it directly)
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I32Const(null_cell.tag));
    function.instruction(&Instruction::I32Store(store_offset(0)));
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I32Const(null_cell.aux));
    function.instruction(&Instruction::I32Store(store_offset(4)));
    function.instruction(&Instruction::I32Const(bases.cell as i32));
    function.instruction(&Instruction::I64Const(null_cell.payload));
    function.instruction(&Instruction::I64Store(store_offset(8)));
    // decode the root envelope (codec rejections raise kind 3 and never
    // return); the DynamicMap cell lands in the context scratch slot
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(decode_index));
    // output buffer, then a zero length before encoding
    function.instruction(&Instruction::I32Const(bases.output_cap as i32));
    function.instruction(&Instruction::Call(alloc_index));
    function.instruction(&Instruction::LocalSet(5));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::I32Store(store_offset(
        CONTEXT_OUTPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Store(store_offset(
        CONTEXT_OUTPUT_LEN_OFFSET,
    )));

    // every parameter starts at the null-cell address so an omitted optional
    // parameter (named mode) reads as null; named-mode bookkeeping zeroes
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(named_seen));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(mask));
    for slot in 0..param_count {
        function.instruction(&Instruction::I32Const(bases.cell as i32));
        function.instruction(&Instruction::LocalSet(param0 + slot as u32));
    }

    // envelope scan: count and pair base from the scratch DynamicMap cell
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(load_offset(CONTEXT_SCRATCH_OFFSET)));
    function.instruction(&Instruction::I32Load(load_offset(4)));
    function.instruction(&Instruction::LocalSet(count));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(load_offset(CONTEXT_SCRATCH_OFFSET)));
    function.instruction(&Instruction::I64Load(load64_offset(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(pair));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(i));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(i));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    // pair stays the constant pair BASE; per iteration:
    // key cell = base + i * 32, value cell = key cell + 16
    function.instruction(&Instruction::LocalGet(pair));
    function.instruction(&Instruction::LocalGet(i));
    function.instruction(&Instruction::I32Const(32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(kcell));
    function.instruction(&Instruction::LocalGet(kcell));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(vcell));
    // key cell must carry the String tag
    function.instruction(&Instruction::LocalGet(kcell));
    function.instruction(&Instruction::I32Load(load_offset(0)));
    function.instruction(&Instruction::I32Const(TAG_STRING));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(&mut function, raise_index, messages.arg, 3);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(kcell));
    function.instruction(&Instruction::I32Load(load_offset(4)));
    function.instruction(&Instruction::LocalSet(klen));
    function.instruction(&Instruction::LocalGet(kcell));
    function.instruction(&Instruction::I64Load(load64_offset(8)));
    function.instruction(&Instruction::I32WrapI64);
    // decoder payloads are absolute heap addresses; use as-is
    function.instruction(&Instruction::LocalSet(kaddr));
    // numeric keys: one ASCII digit selects a positional slot; a 1-byte key
    // that is NOT a digit falls through to the named-matching branch below
    function.instruction(&Instruction::LocalGet(klen));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::LocalGet(kaddr));
    function.instruction(&Instruction::I32Load8U(load_offset(0)));
    function.instruction(&Instruction::LocalTee(bbyte));
    function.instruction(&Instruction::I32Const(0x30));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::LocalGet(bbyte));
    function.instruction(&Instruction::I32Const(0x39));
    function.instruction(&Instruction::I32LeU);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(matched));
    for digit in 0..param_count {
        function.instruction(&Instruction::LocalGet(bbyte));
        function.instruction(&Instruction::I32Const(0x30 + digit as i32));
        function.instruction(&Instruction::I32Eq);
        function.instruction(&Instruction::If(BlockType::Empty));
        // mixing positional keys with named keys rejects (`args` alongside
        // numeric keys is redundant and ignored per the design)
        function.instruction(&Instruction::LocalGet(named_seen));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32Ne);
        function.instruction(&Instruction::If(BlockType::Empty));
        raise(&mut function, raise_index, messages.mixed, 5);
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::LocalGet(vcell));
        function.instruction(&Instruction::LocalSet(param0 + digit as u32));
        function.instruction(&Instruction::LocalGet(seen));
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(seen));
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::LocalSet(matched));
        if digit < 32 {
            function.instruction(&Instruction::LocalGet(mask));
            function.instruction(&Instruction::I32Const(
                1i32.checked_shl(digit as u32).unwrap_or(0),
            ));
            function.instruction(&Instruction::I32Or);
            function.instruction(&Instruction::LocalSet(mask));
        }
        function.instruction(&Instruction::End);
    }
    function.instruction(&Instruction::LocalGet(matched));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(&mut function, raise_index, messages.arg, 3);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::Else);
    // the `args` list key: redundant alongside numeric keys, authoritative
    // only when no numeric key was seen (checked at resolution); any key the
    // numeric branch did not match (including 1-byte names) reaches named
    // matching here
    function.instruction(&Instruction::LocalGet(klen));
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::LocalGet(kaddr));
    function.instruction(&Instruction::I32Load(load_offset(0)));
    function.instruction(&Instruction::I32Const(ARGS_WORD));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(vcell));
    function.instruction(&Instruction::LocalSet(args_cell));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::LocalSet(args_present));
    function.instruction(&Instruction::Else);
    // named key: match the key bytes against each declared parameter name
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(matched));
    for (slot, (name_addr, name_len)) in param_names.iter().enumerate() {
        function.instruction(&Instruction::Block(BlockType::Empty));
        function.instruction(&Instruction::LocalGet(klen));
        function.instruction(&Instruction::I32Const(*name_len as i32));
        function.instruction(&Instruction::I32Eq);
        function.instruction(&Instruction::If(BlockType::Empty));
        // unrolled per-byte compare folded into the eq flag (`bbyte`)
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::LocalSet(bbyte));
        for byte_index in 0..*name_len {
            function.instruction(&Instruction::LocalGet(bbyte));
            function.instruction(&Instruction::LocalGet(kaddr));
            function.instruction(&Instruction::I32Const(byte_index as i32));
            function.instruction(&Instruction::I32Add);
            function.instruction(&Instruction::I32Load8U(load_offset(0)));
            function.instruction(&Instruction::I32Const(*name_addr as i32));
            function.instruction(&Instruction::I32Const(byte_index as i32));
            function.instruction(&Instruction::I32Add);
            function.instruction(&Instruction::I32Load8U(load_offset(0)));
            function.instruction(&Instruction::I32Eq);
            function.instruction(&Instruction::I32And);
            function.instruction(&Instruction::LocalSet(bbyte));
        }
        function.instruction(&Instruction::LocalGet(bbyte));
        function.instruction(&Instruction::If(BlockType::Empty));
        // a named key never mixes with positional keys
        function.instruction(&Instruction::LocalGet(seen));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32Ne);
        function.instruction(&Instruction::If(BlockType::Empty));
        raise(&mut function, raise_index, messages.mixed, 5);
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::LocalGet(vcell));
        function.instruction(&Instruction::LocalSet(param0 + slot as u32));
        function.instruction(&Instruction::LocalGet(named_seen));
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(named_seen));
        if slot < 32 {
            function.instruction(&Instruction::LocalGet(mask));
            function.instruction(&Instruction::I32Const(
                1i32.checked_shl(slot as u32).unwrap_or(0),
            ));
            function.instruction(&Instruction::I32Or);
            function.instruction(&Instruction::LocalSet(mask));
        }
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::LocalSet(matched));
        function.instruction(&Instruction::Br(1));
        function.instruction(&Instruction::End);
        function.instruction(&Instruction::End);
    }
    function.instruction(&Instruction::LocalGet(matched));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(&mut function, raise_index, messages.arg, 3);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(i));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(i));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    // argument resolution: named keys never combine with the `args` list;
    // numeric keys are authoritative (`args` is redundant and ignored), then
    // named-complete, then named-partial with optional fills, then the args
    // list; anything else rejects
    function.instruction(&Instruction::LocalGet(named_seen));
    function.instruction(&Instruction::LocalGet(args_present));
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(&mut function, raise_index, messages.mixed, 5);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(seen));
    function.instruction(&Instruction::I32Const(param_count as i32));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    // positional mode: the scanned parameter locals stand and `args` is
    // ignored entirely
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(seen));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(named_seen));
    function.instruction(&Instruction::I32Const(param_count as i32));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    // named mode, complete: keep the scanned cells
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(named_seen));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    // named mode, partial: keep the scanned cell when the name was seen,
    // fill omitted optional parameters with the null cell, reject otherwise
    for (slot, kind) in param_kinds.iter().enumerate() {
        function.instruction(&Instruction::Block(BlockType::Empty));
        if slot < 32 {
            function.instruction(&Instruction::LocalGet(mask));
            function.instruction(&Instruction::I32Const(
                1i32.checked_shl(slot as u32).unwrap_or(0),
            ));
            function.instruction(&Instruction::I32And);
            function.instruction(&Instruction::BrIf(1));
        }
        if *kind == ParamProjection::OptionalCell {
            function.instruction(&Instruction::I32Const(bases.cell as i32));
            function.instruction(&Instruction::LocalSet(param0 + slot as u32));
            function.instruction(&Instruction::Br(0));
        }
        raise(&mut function, raise_index, messages.arity, 5);
        function.instruction(&Instruction::End);
    }
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(args_present));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(args_cell));
    function.instruction(&Instruction::I32Load(load_offset(0)));
    function.instruction(&Instruction::I32Const(TAG_LIST));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(&mut function, raise_index, messages.arity, 5);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(args_cell));
    function.instruction(&Instruction::I32Load(load_offset(4)));
    function.instruction(&Instruction::I32Const(param_count as i32));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(&mut function, raise_index, messages.arity, 5);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(args_cell));
    function.instruction(&Instruction::I64Load(load64_offset(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(pair));
    for slot in 0..param_count {
        function.instruction(&Instruction::LocalGet(pair));
        function.instruction(&Instruction::I32Const((slot * 16) as i32));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(param0 + slot as u32));
    }
    function.instruction(&Instruction::Else);
    raise(&mut function, raise_index, messages.arity, 5);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::Else);
    raise(&mut function, raise_index, messages.arity, 5);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    // parameter projection: pushes one wasm value per parameter in order
    for (slot, kind) in param_kinds.iter().enumerate() {
        let cell = param0 + slot as u32;
        match kind {
            ParamProjection::Cell | ParamProjection::OptionalCell => {
                // string/bytes/list/record/DynamicMap pass-through; deep
                // projection is a later-milestone scope
                function.instruction(&Instruction::LocalGet(cell));
            }
            ParamProjection::I32 => {
                // decoded MessagePack ints are I64-tagged
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I32Load(load_offset(0)));
                function.instruction(&Instruction::I32Const(TAG_I64));
                function.instruction(&Instruction::I32Ne);
                function.instruction(&Instruction::If(BlockType::Empty));
                raise(&mut function, raise_index, messages.i32, 3);
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I64Load(load64_offset(8)));
                function.instruction(&Instruction::LocalSet(payload64));
                function.instruction(&Instruction::LocalGet(payload64));
                function.instruction(&Instruction::I64Const(i64::from(i32::MIN)));
                function.instruction(&Instruction::I64GeS);
                function.instruction(&Instruction::LocalGet(payload64));
                function.instruction(&Instruction::I64Const(i64::from(i32::MAX)));
                function.instruction(&Instruction::I64LeS);
                function.instruction(&Instruction::I32And);
                function.instruction(&Instruction::I32Eqz);
                function.instruction(&Instruction::If(BlockType::Empty));
                raise(&mut function, raise_index, messages.i32, 3);
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::LocalGet(payload64));
                function.instruction(&Instruction::I32WrapI64);
            }
            ParamProjection::I64 => {
                // accepts an I64 cell payload, or an I32 cell widened in place
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I32Load(load_offset(0)));
                function.instruction(&Instruction::I32Const(TAG_I64));
                function.instruction(&Instruction::I32Eq);
                function.instruction(&Instruction::If(BlockType::Empty));
                function.instruction(&Instruction::Else);
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I32Load(load_offset(0)));
                function.instruction(&Instruction::I32Const(TAG_I32));
                function.instruction(&Instruction::I32Eq);
                function.instruction(&Instruction::If(BlockType::Empty));
                function.instruction(&Instruction::Else);
                raise(&mut function, raise_index, messages.arg, 3);
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I64Load(load64_offset(8)));
            }
            ParamProjection::F64 => {
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I32Load(load_offset(0)));
                function.instruction(&Instruction::I32Const(TAG_F64));
                function.instruction(&Instruction::I32Ne);
                function.instruction(&Instruction::If(BlockType::Empty));
                raise(&mut function, raise_index, messages.arg, 3);
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I64Load(load64_offset(8)));
                function.instruction(&Instruction::F64ReinterpretI64);
            }
            ParamProjection::Bool => {
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I32Load(load_offset(0)));
                function.instruction(&Instruction::I32Const(TAG_BOOL));
                function.instruction(&Instruction::I32Ne);
                function.instruction(&Instruction::If(BlockType::Empty));
                raise(&mut function, raise_index, messages.arg, 3);
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::LocalGet(cell));
                function.instruction(&Instruction::I32Load(load_offset(8)));
            }
        }
    }
    function.instruction(&Instruction::Call(entry_index));

    // result boxing into the context scratch cell
    match result_projection {
        None => {
            function.instruction(&Instruction::I32Const(bases.cell as i32));
            function.instruction(&Instruction::LocalSet(boxed));
        }
        Some(kind) => match kind {
            ParamProjection::Cell | ParamProjection::OptionalCell => {
                function.instruction(&Instruction::LocalSet(boxed));
            }
            ParamProjection::Bool => {
                function.instruction(&Instruction::LocalSet(bool_value));
                function.instruction(&Instruction::I32Const(16));
                function.instruction(&Instruction::Call(alloc_index));
                function.instruction(&Instruction::LocalSet(boxed));
                function.instruction(&Instruction::LocalGet(boxed));
                function.instruction(&Instruction::I32Const(TAG_BOOL));
                function.instruction(&Instruction::I32Store(store_offset(0)));
                function.instruction(&Instruction::LocalGet(boxed));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Store(store_offset(4)));
                function.instruction(&Instruction::LocalGet(boxed));
                function.instruction(&Instruction::LocalGet(bool_value));
                function.instruction(&Instruction::I32Store(store_offset(8)));
            }
            other => {
                let tag = match other {
                    ParamProjection::I32 | ParamProjection::I64 => TAG_I64,
                    _ => TAG_F64,
                };
                match other {
                    ParamProjection::I32 => {
                        function.instruction(&Instruction::I64ExtendI32S);
                    }
                    ParamProjection::F64 => {
                        function.instruction(&Instruction::I64ReinterpretF64);
                    }
                    _ => {}
                }
                function.instruction(&Instruction::LocalSet(payload64));
                function.instruction(&Instruction::I32Const(16));
                function.instruction(&Instruction::Call(alloc_index));
                function.instruction(&Instruction::LocalSet(boxed));
                function.instruction(&Instruction::LocalGet(boxed));
                function.instruction(&Instruction::I32Const(tag));
                function.instruction(&Instruction::I32Store(store_offset(0)));
                function.instruction(&Instruction::LocalGet(boxed));
                function.instruction(&Instruction::I32Const(0));
                function.instruction(&Instruction::I32Store(store_offset(4)));
                function.instruction(&Instruction::LocalGet(boxed));
                function.instruction(&Instruction::LocalGet(payload64));
                function.instruction(&Instruction::I64Store(store_offset(8)));
            }
        },
    }
    // copy the boxed cell (or null cell / returned cell ptr) into the
    // context scratch slot the encoder reads the root cell from
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(load_offset(CONTEXT_SCRATCH_OFFSET)));
    function.instruction(&Instruction::LocalSet(scratch));
    for (width, offset) in [(0u32, 0u32), (4, 4)] {
        function.instruction(&Instruction::LocalGet(scratch));
        function.instruction(&Instruction::LocalGet(boxed));
        function.instruction(&Instruction::I32Load(load_offset(offset)));
        function.instruction(&Instruction::I32Store(store_offset(width)));
    }
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::LocalGet(boxed));
    function.instruction(&Instruction::I64Load(load64_offset(8)));
    function.instruction(&Instruction::I64Store(store_offset(8)));
    // encode, then the M1 ending: (ptr << 32) | len from the context
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(encode_index));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(load_offset(
        CONTEXT_OUTPUT_PTR_OFFSET,
    )));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Const(32));
    function.instruction(&Instruction::I64Shl);
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32Load(load_offset(
        CONTEXT_OUTPUT_LEN_OFFSET,
    )));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::End);
    function
}
