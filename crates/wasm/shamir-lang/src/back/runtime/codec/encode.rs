use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::runtime::layout::RuntimeCaps;

use super::decode::{
    reject_message, FAIL_DEPTH, FAIL_ENTRIES, FAIL_LENGTH, FAIL_NONFINITE, FAIL_STR,
};
use super::frames::{
    CONTEXT_FRAME_BASE_OFFSET, CONTEXT_OUTPUT_CAP_OFFSET, CONTEXT_SCRATCH_OFFSET, FRAME_BYTES,
};

mod sort;

use sort::{build_pair_index, insertion_sort_pairs, resolve_sort_base, sort_window_entries};

/// Function indices and caps the encoder emitters need at emission time. Only
/// helpers the encoder actually calls are listed; it allocates nothing.
pub(crate) struct EncodeEmit {
    pub scratch: u32,
    /// Start of the encoder pair-index window inside the workspace.
    pub sort_base: u32,
    pub raise_index: u32,
    pub append_index: u32,
    pub compare_index: u32,
    pub caps: RuntimeCaps,
}

const TAG_NULL: i32 = 1;
const TAG_BOOL: i32 = 2;
const TAG_I32: i32 = 3;
const TAG_I64: i32 = 4;
const TAG_F64: i32 = 5;
const TAG_STRING: i32 = 6;
const TAG_BYTES: i32 = 7;
const TAG_LIST: i32 = 8;
const TAG_RECORD: i32 = 9;
const TAG_DYNAMIC_MAP: i32 = 14;

const KIND_ARRAY: i32 = 1;
const KIND_MAP: i32 = 2;
/// Record frames walk field cells directly (fields are pre-sorted, so the
/// sort window is never touched).
const KIND_RECORD: i32 = 3;

/// Frame fields (32-byte frames, same region as the decoder frame stack).
const E_KIND: u32 = 0;
const E_COUNT: u32 = 16;
const E_TOTAL: u32 = 20;
const E_CURSOR: u32 = 24;
/// Record frames only: the record's static descriptor offset.
const E_DESC: u32 = 4;

// Locals: 0 ctx param, i32 1..=22, i64 23..=24 (exported via `LOCAL_TYPES`).
pub(crate) const L_DEPTH: u32 = 1;
pub(crate) const L_FRAME: u32 = 2;
pub(crate) const L_TAG: u32 = 3;
pub(crate) const L_AUX: u32 = 4;
pub(crate) const L_COUNT: u32 = 5;
pub(crate) const L_CELL: u32 = 6;
pub(crate) const L_I: u32 = 7;
pub(crate) const L_SCAN: u32 = 8;
pub(crate) const L_BASE: u32 = 9;
pub(crate) const L_TMP: u32 = 10;
pub(crate) const L_CMP: u32 = 11;
pub(crate) const L_PAIRS: u32 = 12;
pub(crate) const L_J: u32 = 13;
pub(crate) const L_PREV: u32 = 14;
pub(crate) const L_LA: u32 = 15;
pub(crate) const L_PA: u32 = 16;
pub(crate) const L_LB: u32 = 17;
pub(crate) const L_PB: u32 = 18;
pub(crate) const L_KIND: u32 = 19;
pub(crate) const L_SEQ: u32 = 20;
pub(crate) const L_LEN: u32 = 21;
pub(crate) const L_PTR: u32 = 22;
pub(crate) const L_PAYLOAD: u32 = 23;
pub(crate) const L_VAL: u32 = 24;
/// Types of locals 0..=24 (0 = the ctx param); 22 declared i32 then 2 i64:
/// depth, frame, tag, aux, count, cell, i, scan, base, tmp, cmp, pairs, j,
/// prev, la, pa, lb, pb, kind, seq, len, ptr, payload, val.
pub(crate) const LOCAL_TYPES: [ValType; 25] = {
    let mut types = [ValType::I32; 25];
    types[23] = ValType::I64;
    types[24] = ValType::I64;
    types
};

/// Run-length encodes `LOCAL_TYPES[1..]` into `Function::new` local counts,
/// so the declaration can never drift from the type map.
fn local_decl_counts() -> Vec<(u32, ValType)> {
    let mut counts: Vec<(u32, ValType)> = Vec::new();
    for &kind in LOCAL_TYPES[1..].iter() {
        match counts.last_mut() {
            Some((count, last)) if *last == kind => *count += 1,
            _ => counts.push((1, kind)),
        }
    }
    counts
}

fn cap32(value: u64) -> i32 {
    i32::try_from(value).expect("encoder cap fits i32")
}

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

fn byte_mem() -> MemArg {
    MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }
}

fn lg(function: &mut Function, local: u32) {
    function.instruction(&Instruction::LocalGet(local));
}

fn st(function: &mut Function, local: u32) {
    function.instruction(&Instruction::LocalSet(local));
}

fn ic(function: &mut Function, value: i32) {
    function.instruction(&Instruction::I32Const(value));
}

fn lc(function: &mut Function, value: i64) {
    function.instruction(&Instruction::I64Const(value));
}

/// Kind-3 raise: scratch := packed literal, then
/// raise_error(3, scratch, len) — args pushed in parameter order (first
/// deepest); control never returns (the raiser ends in `unreachable`).
fn reject_sequence(function: &mut Function, params: &EncodeEmit, code: i32) {
    let message = reject_message(code);
    let mut packed = [0u8; 8];
    packed[..message.len()].copy_from_slice(message);
    ic(function, params.scratch as i32);
    lc(function, i64::from_le_bytes(packed));
    function.instruction(&Instruction::I64Store(mem32(0)));
    ic(function, 3);
    ic(function, params.scratch as i32);
    ic(function, message.len() as i32);
    function.instruction(&Instruction::Call(params.raise_index));
    function.instruction(&Instruction::Unreachable);
}

/// Charges `bytes` against the output budget before anything is written. The
/// live length is at ctx offset 12 and the real allocation capacity (written
/// by the wrapper) at ctx offset 52.
fn budget(function: &mut Function, params: &EncodeEmit, bytes: i32) {
    lg(function, 0);
    function.instruction(&Instruction::I32Load(mem32(12)));
    ic(function, bytes);
    function.instruction(&Instruction::I32Add);
    lg(function, 0);
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_OUTPUT_CAP_OFFSET)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_LENGTH);
    function.instruction(&Instruction::End);
}

/// Like `budget`, but charges the blob payload in `L_LEN` plus `header`.
fn budget_blob(function: &mut Function, params: &EncodeEmit, header: i32) {
    lg(function, 0);
    function.instruction(&Instruction::I32Load(mem32(12)));
    lg(function, L_LEN);
    function.instruction(&Instruction::I32Add);
    ic(function, header);
    function.instruction(&Instruction::I32Add);
    lg(function, 0);
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_OUTPUT_CAP_OFFSET)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_LENGTH);
    function.instruction(&Instruction::End);
}

/// Closes an append_byte producer whose stack holds `(ctx, byte)`: every
/// producer pushes ctx FIRST (parameter order: first param deepest).
fn append_call(function: &mut Function, params: &EncodeEmit) {
    function.instruction(&Instruction::Call(params.append_index));
}

fn append_const(function: &mut Function, params: &EncodeEmit, byte: i32) {
    lg(function, 0);
    ic(function, byte);
    append_call(function, params);
}

/// Appends the big-endian `n` low bytes of the i64 in `val`: ctx first, then
/// each extracted byte, one call per byte.
fn append_be(function: &mut Function, params: &EncodeEmit, val: u32, n: i32) {
    for index in 0..n {
        lg(function, 0);
        lg(function, val);
        lc(function, i64::from(8 * (n - 1 - index)));
        function.instruction(&Instruction::I64ShrU);
        function.instruction(&Instruction::I32WrapI64);
        ic(function, 0xff);
        function.instruction(&Instruction::I32And);
        append_call(function, params);
    }
}

/// Appends the `n` low bytes of the i32 in `src`, widened into `L_VAL`.
fn append_len_be(function: &mut Function, params: &EncodeEmit, src: u32, n: i32) {
    lg(function, src);
    function.instruction(&Instruction::I64ExtendI32U);
    st(function, L_VAL);
    append_be(function, params, L_VAL, n);
}

/// String header with the shortest length prefix: `0xa0|n` below 32, `d9`
/// with a 1-byte length below 256, `da` with 2 bytes below 65,536, else `db`
/// with 4 bytes. Length expected in `L_LEN`.
fn emit_str_header(function: &mut Function, params: &EncodeEmit) {
    lg(function, L_LEN);
    ic(function, 32);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, 0);
    lg(function, L_LEN);
    ic(function, 0xa0);
    function.instruction(&Instruction::I32Or);
    append_call(function, params);
    function.instruction(&Instruction::Else);
    lg(function, L_LEN);
    ic(function, 256);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xd9);
    append_len_be(function, params, L_LEN, 1);
    function.instruction(&Instruction::Else);
    lg(function, L_LEN);
    ic(function, 65_536);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xda);
    append_len_be(function, params, L_LEN, 2);
    function.instruction(&Instruction::Else);
    append_const(function, params, 0xdb);
    append_len_be(function, params, L_LEN, 4);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
}

/// Binary header: no fix range — `c4` with a 1-byte length below 256, `c5`
/// with 2 bytes below 65,536, else `c6` with 4 bytes. Length in `L_LEN`.
fn emit_bin_header(function: &mut Function, params: &EncodeEmit) {
    lg(function, L_LEN);
    ic(function, 256);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xc4);
    append_len_be(function, params, L_LEN, 1);
    function.instruction(&Instruction::Else);
    lg(function, L_LEN);
    ic(function, 65_536);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xc5);
    append_len_be(function, params, L_LEN, 2);
    function.instruction(&Instruction::Else);
    append_const(function, params, 0xc6);
    append_len_be(function, params, L_LEN, 4);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
}

/// Container header with the shortest count prefix: `fix_base|n` below 16,
/// `wide16` with a 2-byte count below 65,536, else `wide32` with 4 bytes.
fn emit_container_header(
    function: &mut Function,
    params: &EncodeEmit,
    count: u32,
    fix_base: i32,
    wide16: i32,
    wide32: i32,
) {
    lg(function, count);
    ic(function, 16);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, 0);
    lg(function, count);
    ic(function, fix_base);
    function.instruction(&Instruction::I32Or);
    append_call(function, params);
    function.instruction(&Instruction::Else);
    lg(function, count);
    ic(function, 65_536);
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, wide16);
    append_len_be(function, params, count, 2);
    function.instruction(&Instruction::Else);
    append_const(function, params, wide32);
    append_len_be(function, params, count, 4);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
}

/// Shortest MessagePack integer for the signed i64 in `L_VAL`.
fn emit_int(function: &mut Function, params: &EncodeEmit) {
    budget(function, params, 9);
    lg(function, L_VAL);
    lc(function, 0);
    function.instruction(&Instruction::I64LtS);
    function.instruction(&Instruction::If(BlockType::Empty));
    // negative: fixint down to -32, then int8/16/32/64 by shortest width
    lg(function, L_VAL);
    lc(function, -32);
    function.instruction(&Instruction::I64GeS);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, 0);
    lg(function, L_VAL);
    lc(function, 32);
    function.instruction(&Instruction::I64Add);
    function.instruction(&Instruction::I32WrapI64);
    ic(function, 0xe0);
    function.instruction(&Instruction::I32Or);
    append_call(function, params);
    function.instruction(&Instruction::Else);
    lg(function, L_VAL);
    lc(function, -128);
    function.instruction(&Instruction::I64GeS);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xd0);
    append_be(function, params, L_VAL, 1);
    function.instruction(&Instruction::Else);
    lg(function, L_VAL);
    lc(function, -32_768);
    function.instruction(&Instruction::I64GeS);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xd1);
    append_be(function, params, L_VAL, 2);
    function.instruction(&Instruction::Else);
    lg(function, L_VAL);
    lc(function, i64::from(i32::MIN));
    function.instruction(&Instruction::I64GeS);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xd2);
    append_be(function, params, L_VAL, 4);
    function.instruction(&Instruction::Else);
    append_const(function, params, 0xd3);
    append_be(function, params, L_VAL, 8);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::Else);
    // non-negative: fixint up to 127, then uint8/16/32/64 by shortest width
    lg(function, L_VAL);
    lc(function, 0x80);
    function.instruction(&Instruction::I64LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, 0);
    lg(function, L_VAL);
    function.instruction(&Instruction::I32WrapI64);
    append_call(function, params);
    function.instruction(&Instruction::Else);
    lg(function, L_VAL);
    lc(function, 0x100);
    function.instruction(&Instruction::I64LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xcc);
    append_be(function, params, L_VAL, 1);
    function.instruction(&Instruction::Else);
    lg(function, L_VAL);
    lc(function, 0x1_0000);
    function.instruction(&Instruction::I64LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xcd);
    append_be(function, params, L_VAL, 2);
    function.instruction(&Instruction::Else);
    lg(function, L_VAL);
    lc(function, 0x1_0000_0000);
    function.instruction(&Instruction::I64LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    append_const(function, params, 0xce);
    append_be(function, params, L_VAL, 4);
    function.instruction(&Instruction::Else);
    append_const(function, params, 0xcf);
    append_be(function, params, L_VAL, 8);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
}

/// Float64 for the raw bits in `L_PAYLOAD`; non-finite rejects (kind 3)
/// before any write and negative zero normalizes to positive zero.
fn emit_f64(function: &mut Function, params: &EncodeEmit) {
    lg(function, L_PAYLOAD);
    lc(function, 0x7ff0_0000_0000_0000);
    function.instruction(&Instruction::I64And);
    lc(function, 0x7ff0_0000_0000_0000);
    function.instruction(&Instruction::I64GeU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_NONFINITE);
    function.instruction(&Instruction::End);
    lg(function, L_PAYLOAD);
    lc(function, i64::MIN);
    function.instruction(&Instruction::I64Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lc(function, 0);
    st(function, L_PAYLOAD);
    function.instruction(&Instruction::End);
    budget(function, params, 9);
    append_const(function, params, 0xcb);
    append_be(function, params, L_PAYLOAD, 8);
}

/// Blob for the length in `L_LEN` and heap pointer in `L_PTR`: string cap
/// re-check, budget charge, shortest header, byte-at-a-time copy loop.
fn emit_blob(function: &mut Function, params: &EncodeEmit, is_string: bool) {
    lg(function, L_LEN);
    ic(function, cap32(params.caps.string_bytes));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_STR);
    function.instruction(&Instruction::End);
    budget_blob(function, params, 5);
    if is_string {
        emit_str_header(function, params);
    } else {
        emit_bin_header(function, params);
    }
    // copy loop; branch targets annotated at the br sites below
    ic(function, 0);
    st(function, L_I);
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    lg(function, L_I);
    lg(function, L_LEN);
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1)); // exit Block
    lg(function, 0);
    lg(function, L_PTR);
    lg(function, L_I);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(byte_mem()));
    append_call(function, params);
    lg(function, L_I);
    ic(function, 1);
    function.instruction(&Instruction::I32Add);
    st(function, L_I);
    function.instruction(&Instruction::Br(0)); // Loop
    function.instruction(&Instruction::End); // Loop
    function.instruction(&Instruction::End); // Block
}

/// Depth check (`depth + 1 > caps.depth` rejects) before a frame push, then
/// the new frame address in `L_FRAME`.
fn claim_depth(function: &mut Function, params: &EncodeEmit) {
    lg(function, L_DEPTH);
    ic(function, 1);
    function.instruction(&Instruction::I32Add);
    ic(function, cap32(u64::from(params.caps.depth)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_DEPTH);
    function.instruction(&Instruction::End);
    lg(function, L_DEPTH);
    ic(function, 1);
    function.instruction(&Instruction::I32Add);
    st(function, L_DEPTH);
    lg(function, 0);
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_FRAME_BASE_OFFSET)));
    lg(function, L_DEPTH);
    ic(function, FRAME_BYTES as i32);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    ic(function, FRAME_BYTES as i32);
    function.instruction(&Instruction::I32Sub);
    st(function, L_FRAME);
}

/// Stores into `frame[field]` with the frame address pushed first: the store
/// pops the value from the top of the stack, so the address goes deepest.
fn frame_store(function: &mut Function, field: u32, value: u32) {
    lg(function, L_FRAME);
    lg(function, value);
    function.instruction(&Instruction::I32Store(mem32(field)));
}

fn frame_store_const(function: &mut Function, field: u32, value: i32) {
    lg(function, L_FRAME);
    ic(function, value);
    function.instruction(&Instruction::I32Store(mem32(field)));
}

/// List cell (count in `L_COUNT`, first element cell in `L_CELL`): charges
/// the header budget, emits the shortest array header, then pushes a frame
/// when the list is non-empty.
fn emit_list_value(function: &mut Function, params: &EncodeEmit) {
    budget(function, params, 5);
    emit_container_header(function, params, L_COUNT, 0x90, 0xdc, 0xdd);
    lg(function, L_COUNT);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Else);
    claim_depth(function, params);
    frame_store_const(function, E_KIND, KIND_ARRAY);
    frame_store(function, E_COUNT, L_COUNT);
    frame_store_const(function, E_TOTAL, 0);
    frame_store(function, E_CURSOR, L_CELL);
    function.instruction(&Instruction::End);
}

/// Map cell (pairs count in `L_PAIRS`, pair base in `L_CELL`): entries cap,
/// nearest free sort window slot, cumulative window bound, pair index build,
/// insertion sort, shortest map header and the frame push.
fn emit_map_value(function: &mut Function, params: &EncodeEmit) {
    lg(function, L_PAIRS);
    ic(function, cap32(params.caps.entries));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_ENTRIES);
    function.instruction(&Instruction::End);
    resolve_sort_base(function, params);
    // the new index array [base, base + pairs * 4) must end inside the window
    lg(function, L_BASE);
    lg(function, L_PAIRS);
    ic(function, 2);
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::I32Add);
    ic(
        function,
        params.sort_base as i32 + sort_window_entries(params) * 4,
    );
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_LENGTH);
    function.instruction(&Instruction::End);
    build_pair_index(function, params);
    insertion_sort_pairs(function, params);
    budget(function, params, 5);
    emit_container_header(function, params, L_PAIRS, 0x80, 0xde, 0xdf);
    claim_depth(function, params);
    // pairs << 1 parked in L_SEQ for both count and total stores
    lg(function, L_PAIRS);
    ic(function, 1);
    function.instruction(&Instruction::I32Shl);
    st(function, L_SEQ);
    frame_store_const(function, E_KIND, KIND_MAP);
    frame_store(function, E_COUNT, L_SEQ);
    frame_store(function, E_TOTAL, L_SEQ);
    frame_store(function, E_CURSOR, L_BASE);
}

/// Record cell (field count in `L_COUNT`, field-cell region in `L_CELL`,
/// descriptor offset in `L_SEQ`): map header for the field count, then a
/// KIND_RECORD frame whose cursor walks the region. Field names are read
/// from the descriptor in the static data at encode time; the sort window is
/// not touched (fields are pre-sorted by the lowering).
fn emit_record_value(function: &mut Function, params: &EncodeEmit) {
    budget(function, params, 5);
    emit_container_header(function, params, L_COUNT, 0x80, 0xde, 0xdf);
    claim_depth(function, params);
    frame_store_const(function, E_KIND, KIND_RECORD);
    frame_store(function, E_COUNT, L_COUNT);
    frame_store_const(function, E_TOTAL, 0);
    frame_store(function, E_CURSOR, L_CELL);
    frame_store(function, E_DESC, L_SEQ);
}

/// Appends `L_LEN` raw bytes at `L_PTR` (static field names), one
/// append_byte call per byte. Mirrors the blob copy loop without the
/// string-cap re-check (field names are compiler-bounded).
fn emit_name_bytes(function: &mut Function, params: &EncodeEmit) {
    ic(function, 0);
    st(function, L_I);
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    lg(function, L_I);
    lg(function, L_LEN);
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1)); // exit Block
    lg(function, 0);
    lg(function, L_PTR);
    lg(function, L_I);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(byte_mem()));
    append_call(function, params);
    lg(function, L_I);
    ic(function, 1);
    function.instruction(&Instruction::I32Add);
    st(function, L_I);
    function.instruction(&Instruction::Br(0)); // Loop
    function.instruction(&Instruction::End); // Loop
    function.instruction(&Instruction::End); // Block
}

/// Loads tag/aux/payload of the cell in `L_CELL` and dispatches on the tag.
/// Emitted inside an open dispatch Block: leaf branches `Br exit_depth` (exit
/// the dispatch Block), container branches push a frame and also exit toward
/// the enclosing step loop, unknown tags fall through to `unreachable`.
fn emit_cell_dispatch(function: &mut Function, params: &EncodeEmit, exit_depth: u32) {
    lg(function, L_CELL);
    function.instruction(&Instruction::I32Load(mem32(0)));
    st(function, L_TAG);
    lg(function, L_CELL);
    function.instruction(&Instruction::I32Load(mem32(4)));
    st(function, L_AUX);
    lg(function, L_CELL);
    function.instruction(&Instruction::I64Load(mem64(8)));
    st(function, L_PAYLOAD);
    lg(function, L_TAG);
    ic(function, TAG_NULL);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    budget(function, params, 1);
    append_const(function, params, 0xc0);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_BOOL);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    budget(function, params, 1);
    lg(function, 0);
    ic(function, 0xc2);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I32Add);
    append_call(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_I32);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I64ExtendI32S);
    st(function, L_VAL);
    emit_int(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_I64);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_PAYLOAD);
    st(function, L_VAL);
    emit_int(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_F64);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    emit_f64(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_STRING);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_AUX);
    st(function, L_LEN);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_PTR);
    emit_blob(function, params, true);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_BYTES);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_AUX);
    st(function, L_LEN);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_PTR);
    emit_blob(function, params, false);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_LIST);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_AUX);
    st(function, L_COUNT);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_CELL);
    emit_list_value(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_DYNAMIC_MAP);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_AUX);
    st(function, L_PAIRS);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_CELL);
    emit_map_value(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    lg(function, L_TAG);
    ic(function, TAG_RECORD);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    // aux = field count, payload low = field-cell region, payload high =
    // descriptor offset (parked in L_SEQ, free while the record is open)
    lg(function, L_AUX);
    st(function, L_COUNT);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_CELL);
    lg(function, L_PAYLOAD);
    function.instruction(&Instruction::I64Const(32));
    function.instruction(&Instruction::I64ShrU);
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_SEQ);
    emit_record_value(function, params);
    function.instruction(&Instruction::Br(exit_depth));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::Unreachable);
}

/// Emits the child value cell whose address sits in `L_CELL`, inside the
/// step loop. Leaf and container branches exit the dispatch Block `$disp`
/// (`Br 1` from the tag `if`); the trailing `Br 2` then restarts `$step`
/// (countIf(0) kindIf(1) $step(2)), the only context this is emitted in. An
/// unknown tag hits the `unreachable` that ends the dispatch Block body.
fn dispatch_child(function: &mut Function, params: &EncodeEmit) {
    function.instruction(&Instruction::Block(BlockType::Empty)); // $disp
    emit_cell_dispatch(function, params, 1);
    function.instruction(&Instruction::End); // $disp
    function.instruction(&Instruction::Br(2)); // -> $step
}

/// `(ctx:i32) -> ()`: iterative MessagePack value encoder over the explicit
/// frame stack. The root cell lives at the context scratch slot; output bytes
/// append to the buffer recorded at context offsets ptr=8 / len=12 and every
/// write is charged against the wrapper-stored capacity at offset 52 first.
/// Locals: 0 ctx, i32 1..=22, i64 23..=24 (see `LOCAL_TYPES`). Maps emit
/// pairs sorted by key bytes.
pub(crate) fn emit_encode_root(params: &EncodeEmit) -> Function {
    let mut function = Function::new(local_decl_counts());
    // root cell from the context scratch slot, emitted before any frame exists
    lg(&mut function, 0);
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_SCRATCH_OFFSET)));
    st(&mut function, L_CELL);
    // root dispatch: Block $rd_ok { Block $rd with leaf branches `Br 1`
    // (If(0) $rd(1)); a leaf then `Br 0` out of $rd_ok to the step loop,
    // while an unknown tag falls out of $rd into the trailing unreachable
    function.instruction(&Instruction::Block(BlockType::Empty)); // $rd_ok
    function.instruction(&Instruction::Block(BlockType::Empty)); // $rd
    emit_cell_dispatch(&mut function, params, 1);
    function.instruction(&Instruction::End); // $rd
    function.instruction(&Instruction::Br(0)); // $rd_ok: leaf path continues
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End); // $rd_ok
                                             // frame walk: Block $done { Loop $step { ... } End $step } End $done
    function.instruction(&Instruction::Block(BlockType::Empty)); // $done
    function.instruction(&Instruction::Loop(BlockType::Empty)); // $step
    lg(&mut function, L_DEPTH);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Br(2)); // If(0) $step(1) $done(2)
    function.instruction(&Instruction::End);
    lg(&mut function, 0);
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_FRAME_BASE_OFFSET)));
    lg(&mut function, L_DEPTH);
    ic(&mut function, FRAME_BYTES as i32);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    ic(&mut function, FRAME_BYTES as i32);
    function.instruction(&Instruction::I32Sub);
    st(&mut function, L_FRAME);
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_KIND)));
    st(&mut function, L_KIND);
    // array frame: cursor walks element cells with a 16-byte stride
    lg(&mut function, L_KIND);
    ic(&mut function, KIND_ARRAY);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_COUNT)));
    st(&mut function, L_COUNT);
    lg(&mut function, L_COUNT);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_DEPTH);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Sub);
    st(&mut function, L_DEPTH);
    function.instruction(&Instruction::Br(2)); // countIf(0) kindIf(1) $step(2)
    function.instruction(&Instruction::Else);
    lg(&mut function, L_FRAME);
    lg(&mut function, L_COUNT);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Store(mem32(E_COUNT)));
    // cell = frame[24]; frame[24] = cell + 16
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_CURSOR)));
    st(&mut function, L_CELL);
    lg(&mut function, L_FRAME);
    lg(&mut function, L_CELL);
    ic(&mut function, 16);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Store(mem32(E_CURSOR)));
    dispatch_child(&mut function, params);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    // map frame: the index array walks pair cells, keys and values alternate
    lg(&mut function, L_KIND);
    ic(&mut function, KIND_MAP);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_COUNT)));
    st(&mut function, L_COUNT);
    lg(&mut function, L_COUNT);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_DEPTH);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Sub);
    st(&mut function, L_DEPTH);
    function.instruction(&Instruction::Br(2)); // countIf(0) kindIf(1) $step(2)
    function.instruction(&Instruction::Else);
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_TOTAL)));
    lg(&mut function, L_COUNT);
    function.instruction(&Instruction::I32Sub);
    st(&mut function, L_SEQ);
    lg(&mut function, L_SEQ);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32ShrU);
    st(&mut function, L_J);
    // cell = index[seq >> 1] loaded from frame[24]
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_CURSOR)));
    lg(&mut function, L_J);
    ic(&mut function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load(mem32(0)));
    st(&mut function, L_CELL);
    lg(&mut function, L_SEQ);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_CELL);
    ic(&mut function, 16);
    function.instruction(&Instruction::I32Add);
    st(&mut function, L_CELL);
    function.instruction(&Instruction::End);
    lg(&mut function, L_FRAME);
    lg(&mut function, L_COUNT);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Store(mem32(E_COUNT)));
    dispatch_child(&mut function, params);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    // record frame: the cursor walks field cells; each step emits the field
    // name (read from the static descriptor) then dispatches the field cell
    lg(&mut function, L_KIND);
    ic(&mut function, KIND_RECORD);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_COUNT)));
    st(&mut function, L_COUNT);
    lg(&mut function, L_COUNT);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(&mut function, L_DEPTH);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Sub);
    st(&mut function, L_DEPTH);
    function.instruction(&Instruction::Br(2)); // countIf(0) kindIf(1) $step(2)
    function.instruction(&Instruction::Else);
    // field index = fields emitted so far (E_TOTAL counts up)
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_TOTAL)));
    st(&mut function, L_J);
    lg(&mut function, L_FRAME);
    lg(&mut function, L_J);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Store(mem32(E_TOTAL)));
    lg(&mut function, L_FRAME);
    lg(&mut function, L_COUNT);
    ic(&mut function, 1);
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Store(mem32(E_COUNT)));
    // descriptor offset parked at frame[E_DESC]
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_DESC)));
    st(&mut function, L_SEQ);
    // name length at descriptor + 4 + i*8, name offset at +8 (one add chain;
    // descriptor and name offsets are ABSOLUTE per the builder)
    lg(&mut function, L_SEQ);
    lg(&mut function, L_J);
    ic(&mut function, 8);
    function.instruction(&Instruction::I32Mul);
    ic(&mut function, 4);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load(mem32(0)));
    st(&mut function, L_LEN);
    lg(&mut function, L_SEQ);
    lg(&mut function, L_J);
    ic(&mut function, 8);
    function.instruction(&Instruction::I32Mul);
    ic(&mut function, 8);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load(mem32(0)));
    st(&mut function, L_PTR);
    budget_blob(&mut function, params, 0);
    emit_str_header(&mut function, params);
    emit_name_bytes(&mut function, params);
    // dispatch the field cell and advance the cursor
    lg(&mut function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_CURSOR)));
    st(&mut function, L_CELL);
    lg(&mut function, L_FRAME);
    lg(&mut function, L_CELL);
    ic(&mut function, 16);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Store(mem32(E_CURSOR)));
    dispatch_child(&mut function, params);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    // unknown frame kinds are an impossible emitter invariant; every branch
    // above exits through an explicit br, so only unknown kinds arrive here
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End); // $step
    function.instruction(&Instruction::End); // $done
    function.instruction(&Instruction::End); // function
    function
}
