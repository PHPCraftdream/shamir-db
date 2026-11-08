use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::runtime::cell::CellTag;
use crate::back::runtime::codec::frames::{
    CONTEXT_CELL_BASE_OFFSET, CONTEXT_FRAME_BASE_OFFSET, CONTEXT_INPUT_LEN_OFFSET,
    CONTEXT_INPUT_PTR_OFFSET, CONTEXT_SCRATCH_OFFSET,
};
use crate::back::runtime::layout::RuntimeCaps;

use super::container::container;
use super::payload::{be_len, be_read_const, be_read_len, bin_payload, need_bytes, str_payload};
use super::reject::{
    leaf_bool, reject_sequence, FAIL_DUP, FAIL_EXT, FAIL_INPUT, FAIL_KEY, FAIL_RESERVED,
    FAIL_TRAIL, FAIL_TRUNC, FAIL_U64,
};

/// Function indices and caps the decoder emitters need at emission time. All
/// indices are module-level defined-function indices resolved from the
/// dependency-first helper order.
pub(crate) struct DecodeEmit {
    pub scratch: u32,
    pub raise_index: u32,
    pub alloc_index: u32,
    pub check_range_index: u32,
    pub read_byte_index: u32,
    pub read_be_index: u32,
    pub compare_index: u32,
    pub utf8_index: u32,
    pub store_cell_index: u32,
    pub caps: RuntimeCaps,
}

/// Named decoder locals; the `Function::new` counts are derived from these
/// (`[(21, I32), (2, I64)]`: the ctx param occupies index 0, declared locals
/// follow in this order). `LOCAL_TYPES` mirrors the map for the codec
/// type-map test.
pub(super) const L_CURSOR: u32 = 1;
pub(super) const L_END: u32 = 2;
pub(super) const L_BUMP: u32 = 3;
pub(super) const L_CELLS: u32 = 4;
pub(super) const L_CONTAINERS: u32 = 5;
pub(super) const L_KEYBYTES: u32 = 6;
pub(super) const L_DEPTH: u32 = 7;
pub(super) const L_FRAME: u32 = 8;
pub(super) const L_OP: u32 = 9;
pub(super) const L_LEN: u32 = 10;
pub(super) const L_PTR: u32 = 11;
pub(super) const L_TMP: u32 = 12;
pub(super) const L_PAIRS: u32 = 13;
pub(super) const L_ISKEY: u32 = 14;
pub(super) const L_REMAINING: u32 = 15;
pub(super) const L_TAG: u32 = 16;
pub(super) const L_SCAN: u32 = 17;
pub(super) const L_KEYCELL: u32 = 18;
pub(super) const L_MEMBYTES: u32 = 19;
pub(super) const L_DEST: u32 = 20;
/// Aux holder for the leaf store; also the aux slot while writing a completed
/// container cell (the two uses never overlap in time).
pub(super) const L_AUX: u32 = 21;
pub(super) const L_PAYLOAD: u32 = 22;
pub(super) const L_SHIFT: u32 = 23;

/// Types of locals 0..=23 (0 = the ctx parameter); 21 declared i32 then 2 i64.
pub(crate) const LOCAL_TYPES: [ValType; 24] = [
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I64,
    ValType::I64,
];

pub(super) const F_KIND: u32 = 12;
pub(super) const F_DEST: u32 = 8;
pub(super) const F_COUNT: u32 = 16;
pub(super) const F_AUX: u32 = 20;
pub(super) const F_FIRST: u32 = 24;
pub(super) const F_KEYBASE: u32 = 28;
pub(super) const KIND_SCALAR: i32 = 0;
pub(super) const KIND_ARRAY: i32 = 1;
pub(super) const KIND_MAP: i32 = 2;

pub(super) const TAG_NULL: i32 = CellTag::Null as u32 as i32;
pub(super) const TAG_BOOL: i32 = CellTag::Bool as u32 as i32;
pub(super) const TAG_I64: i32 = CellTag::I64 as u32 as i32;
pub(super) const TAG_F64: i32 = CellTag::F64 as u32 as i32;
pub(super) const TAG_STRING: i32 = CellTag::String as u32 as i32;
pub(super) const TAG_BYTES: i32 = CellTag::Bytes as u32 as i32;
pub(super) const TAG_LIST: i32 = CellTag::List as u32 as i32;
pub(super) const TAG_DYNAMIC_MAP: i32 = CellTag::DynamicMap as u32 as i32;

pub(super) fn cap32(value: u64) -> i32 {
    i32::try_from(value).expect("decoder cap fits i32")
}

pub(super) fn mem32(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

pub(super) fn byte_mem() -> MemArg {
    MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }
}

/// `frame = ctx.frame_base + depth * 32 - 32`, into local 8.
pub(super) fn frame_addr(function: &mut Function) {
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_FRAME_BASE_OFFSET)));
    function.instruction(&Instruction::LocalGet(L_DEPTH));
    function.instruction(&Instruction::I32Const(32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Const(32));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::LocalSet(L_FRAME));
}

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

/// `(ctx:i32) -> ()`: iterative MessagePack decoder over the explicit frame
/// stack. Container children are stored into contiguously pre-reserved frame
/// windows (the whole child block is reserved before descending), so every
/// cap is enforced before any allocation. Locals per `LOCAL_TYPES`.
pub(crate) fn emit_decode_root(params: &DecodeEmit) -> Function {
    let caps = &params.caps;
    let mut function = Function::new(local_decl_counts());
    macro_rules! i {
        ($op:ident $(, $arg:expr)?) => { function.instruction(&Instruction::$op $(($arg))?); };
    }
    macro_rules! fail {
        ($code:expr) => {
            reject_sequence(&mut function, params, $code);
        };
    }
    macro_rules! frame_store {
        ($field:expr) => {{
            // the value is pushed by the call site AFTER `LocalGet(L_FRAME)`,
            // so the store pops value-then-address (address deepest)
            i!(I32Store, mem32($field));
        }};
    }

    // memory byte bound, then input range validation
    i!(MemorySize, 0);
    i!(I32Const, 65_536);
    i!(I32Mul);
    i!(LocalSet, L_MEMBYTES);
    i!(LocalGet, 0);
    i!(I32Load, mem32(CONTEXT_INPUT_PTR_OFFSET));
    i!(LocalSet, L_PTR);
    i!(LocalGet, 0);
    i!(I32Load, mem32(CONTEXT_INPUT_LEN_OFFSET));
    i!(LocalSet, L_LEN);
    i!(LocalGet, L_PTR);
    i!(LocalGet, L_LEN);
    i!(LocalGet, L_MEMBYTES);
    i!(Call, params.check_range_index);
    i!(Drop);
    // input size cap before any work
    i!(LocalGet, L_LEN);
    i!(I32Const, cap32(caps.input_bytes));
    i!(I32GtU);
    i!(If, BlockType::Empty);
    fail!(FAIL_INPUT);
    i!(End);
    i!(LocalGet, L_PTR);
    i!(LocalSet, L_CURSOR);
    i!(LocalGet, L_PTR);
    i!(LocalGet, L_LEN);
    i!(I32Add);
    i!(LocalSet, L_END);
    // the root cell is pre-counted; the container counter starts empty
    i!(I32Const, 1);
    i!(LocalSet, L_CELLS);
    i!(I32Const, 0);
    i!(LocalSet, L_CONTAINERS);
    i!(I32Const, 0);
    i!(LocalSet, L_KEYBYTES);
    i!(I32Const, 0);
    i!(LocalSet, L_DEPTH);
    i!(LocalGet, 0);
    i!(I32Load, mem32(CONTEXT_CELL_BASE_OFFSET));
    i!(LocalSet, L_BUMP);
    // push the synthetic scalar-root frame (dest = context scratch slot). It
    // is bookkeeping only and not a container: the root value itself is the
    // envelope, so L_DEPTH counts this frame plus the open containers and the
    // cap check allows `caps.depth + 1` frames
    i!(LocalGet, L_DEPTH);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, L_DEPTH);
    frame_addr(&mut function);
    i!(LocalGet, L_FRAME);
    i!(I32Const, KIND_SCALAR);
    frame_store!(F_KIND);
    i!(LocalGet, L_FRAME);
    i!(LocalGet, 0);
    i!(I32Load, mem32(CONTEXT_SCRATCH_OFFSET));
    frame_store!(F_DEST);
    i!(LocalGet, L_FRAME);
    i!(I32Const, 0);
    frame_store!(F_COUNT);
    i!(LocalGet, L_FRAME);
    i!(I32Const, 0);
    frame_store!(F_AUX);
    i!(LocalGet, L_FRAME);
    i!(I32Const, 0);
    frame_store!(F_FIRST);
    i!(LocalGet, L_FRAME);
    i!(I32Const, 0);
    frame_store!(F_KEYBASE);

    // main loop: decode one value per iteration, then propagate it into the
    // nearest open container frame
    i!(Block, BlockType::Empty); // $done
    i!(Loop, BlockType::Empty); // $main
                                // defensive: depth 0 means the stack drained without the scalar-root exit
    i!(LocalGet, L_DEPTH);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    i!(Br, 2); // -> $done (If(0) $main(1) $done(2))
    i!(End);
    frame_addr(&mut function);
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_KIND));
    i!(LocalSet, L_TMP);
    // destination cell and key mode for the value about to be decoded
    i!(LocalGet, L_TMP);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    // scalar frame (root envelope only): dest is the context scratch slot
    i!(LocalGet, 0);
    i!(I32Load, mem32(CONTEXT_SCRATCH_OFFSET));
    i!(LocalSet, L_DEST);
    i!(I32Const, 0);
    i!(LocalSet, L_ISKEY);
    i!(Else);
    i!(LocalGet, L_TMP);
    i!(I32Const, KIND_ARRAY);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    // list children are contiguous 16-byte cells: FIRST + (AUX - COUNT) * 16
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_FIRST));
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_AUX));
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_COUNT));
    i!(I32Sub);
    i!(I32Const, 16);
    i!(I32Mul);
    i!(I32Add);
    i!(LocalSet, L_DEST);
    i!(I32Const, 0);
    i!(LocalSet, L_ISKEY);
    i!(Else);
    // map pairs are contiguous 32-byte key/value slots; elements done =
    // 2 * AUX - COUNT, pair index = elements done >> 1
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_AUX));
    i!(I32Const, 1);
    i!(I32Shl);
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_COUNT));
    i!(I32Sub);
    i!(I32Const, 1);
    i!(I32ShrU);
    i!(LocalSet, L_PAIRS);
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_COUNT));
    i!(I32Const, 1);
    i!(I32And);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    // even remaining count: this element is a key
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_KEYBASE));
    i!(LocalGet, L_PAIRS);
    i!(I32Const, 32);
    i!(I32Mul);
    i!(I32Add);
    i!(LocalSet, L_DEST);
    i!(I32Const, 1);
    i!(LocalSet, L_ISKEY);
    i!(Else);
    // odd remaining count: this element is the pair value
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_KEYBASE));
    i!(LocalGet, L_PAIRS);
    i!(I32Const, 32);
    i!(I32Mul);
    i!(I32Const, 16);
    i!(I32Add);
    i!(I32Add);
    i!(LocalSet, L_DEST);
    i!(I32Const, 0);
    i!(LocalSet, L_ISKEY);
    i!(End);
    i!(End);
    i!(End);
    // opcode fetch with explicit truncation check
    i!(LocalGet, L_CURSOR);
    i!(LocalGet, L_END);
    i!(I32GeU);
    i!(If, BlockType::Empty);
    fail!(FAIL_TRUNC);
    i!(End);
    i!(LocalGet, L_CURSOR);
    i!(LocalGet, L_END);
    i!(Call, params.read_byte_index);
    i!(LocalSet, L_OP);
    i!(LocalGet, L_CURSOR);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, L_CURSOR);

    // dispatch on the opcode byte: every leaf branch sets tag/aux/payload and
    // brs 1 to exit $disp into the shared leaf store
    i!(Block, BlockType::Empty); // $disp
    dispatch(&mut function, params);
    i!(End); // $disp
             // leaf completion: store_cell(dest, tag, aux, payload), args in order
    i!(LocalGet, L_DEST);
    i!(LocalGet, L_TAG);
    i!(LocalGet, L_AUX);
    i!(LocalGet, L_PAYLOAD);
    i!(Call, params.store_cell_index);

    propagate(&mut function, params);
    // EXACT end sequence: End $prop, End $main, End $done, End function --
    // the propagation loop closes inside the main loop body and every path
    // out of $prop is an explicit br, so no trailing br to $main is needed.
    i!(End); // $prop
    i!(End); // $main
    i!(End); // $done
    function
}

/// Opcode dispatch, emitted inside $disp; leaf branches use
/// `Br 1` (exit $disp: If(0) $disp(1) $main(2) $done(3)) into the shared
/// leaf store, container branches defer to `container()`.
fn dispatch(function: &mut Function, params: &DecodeEmit) {
    macro_rules! i {
        ($op:ident $(, $arg:expr)?) => { function.instruction(&Instruction::$op $(($arg))?); };
    }
    macro_rules! fail {
        ($code:expr) => {
            reject_sequence(function, params, $code);
        };
    }
    macro_rules! leaf {
        ($tag:expr, $aux:expr, $payload_const:expr) => {{
            i!(I32Const, $tag);
            i!(LocalSet, L_TAG);
            i!(I32Const, $aux);
            i!(LocalSet, L_AUX);
            if let Some(value) = $payload_const {
                i!(I64Const, value);
                i!(LocalSet, L_PAYLOAD);
            }
            i!(Br, 1); // exit $disp: leaf store + propagation
        }};
    }

    // nil
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc0);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    leaf!(TAG_NULL, 0, Some(0));
    i!(End);
    // reserved never-used marker
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc1);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    fail!(FAIL_RESERVED);
    i!(End);
    // bool
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc2);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    leaf_bool(function, 0);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc3);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    leaf_bool(function, 1);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // extension families: ext8/16/32 and fixext1..16
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc7);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc9);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    fail!(FAIL_EXT);
    i!(End);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xd4);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xd8);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    fail!(FAIL_EXT);
    i!(End);
    // float32: big-endian bits promoted exactly to f64
    i!(LocalGet, L_OP);
    i!(I32Const, 0xca);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    need_bytes(function, params, 4);
    be_read_const(function, params, 4);
    i!(LocalGet, L_PAYLOAD);
    i!(I32WrapI64);
    i!(F32ReinterpretI32);
    i!(F64PromoteF32);
    i!(I64ReinterpretF64);
    i!(LocalSet, L_PAYLOAD);
    i!(I32Const, TAG_F64);
    i!(LocalSet, L_TAG);
    i!(I32Const, 0);
    i!(LocalSet, L_AUX);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // float64: big-endian bits
    i!(LocalGet, L_OP);
    i!(I32Const, 0xcb);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    need_bytes(function, params, 8);
    be_read_const(function, params, 8);
    i!(I32Const, TAG_F64);
    i!(LocalSet, L_TAG);
    i!(I32Const, 0);
    i!(LocalSet, L_AUX);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // uint8/16/32/64: a u64 above i64::MAX rejects
    i!(LocalGet, L_OP);
    i!(I32Const, 0xcc);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xcf);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    i!(I32Const, 1);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xcc);
    i!(I32Sub);
    i!(I32Shl);
    i!(LocalSet, L_LEN);
    be_read_len(function, params);
    i!(LocalGet, L_PAYLOAD);
    i!(I64Const, 0);
    i!(I64LtS);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xcf);
    i!(I32Eq);
    i!(I32And);
    i!(If, BlockType::Empty);
    fail!(FAIL_U64);
    i!(End);
    i!(I32Const, TAG_I64);
    i!(LocalSet, L_TAG);
    i!(I32Const, 0);
    i!(LocalSet, L_AUX);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // int8/16/32/64 with sign extension
    i!(LocalGet, L_OP);
    i!(I32Const, 0xd0);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xd3);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    i!(I32Const, 1);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xd0);
    i!(I32Sub);
    i!(I32Shl);
    i!(LocalSet, L_LEN);
    be_read_len(function, params);
    // shift = 64 - width * 8 (i64 scratch), payload = (p << shift) >> shift
    i!(I64Const, 64);
    i!(LocalGet, L_LEN);
    i!(I64ExtendI32U);
    i!(I64Const, 8);
    i!(I64Mul);
    i!(I64Sub);
    i!(LocalSet, L_SHIFT);
    i!(LocalGet, L_PAYLOAD);
    i!(LocalGet, L_SHIFT);
    i!(I64Shl);
    i!(LocalGet, L_SHIFT);
    i!(I64ShrS);
    i!(LocalSet, L_PAYLOAD);
    i!(I32Const, TAG_I64);
    i!(LocalSet, L_TAG);
    i!(I32Const, 0);
    i!(LocalSet, L_AUX);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // positive fixint
    i!(LocalGet, L_OP);
    i!(I32Const, 0x80);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(LocalGet, L_OP);
    i!(I64ExtendI32U);
    i!(LocalSet, L_PAYLOAD);
    i!(I32Const, TAG_I64);
    i!(LocalSet, L_TAG);
    i!(I32Const, 0);
    i!(LocalSet, L_AUX);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // negative fixint
    i!(LocalGet, L_OP);
    i!(I32Const, 0xe0);
    i!(I32GeU);
    i!(If, BlockType::Empty);
    i!(LocalGet, L_OP);
    i!(I32Const, 24);
    i!(I32Shl);
    i!(I32Const, 24);
    i!(I32ShrS);
    i!(I64ExtendI32S);
    i!(LocalSet, L_PAYLOAD);
    i!(I32Const, TAG_I64);
    i!(LocalSet, L_TAG);
    i!(I32Const, 0);
    i!(LocalSet, L_AUX);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // fixstr
    i!(LocalGet, L_OP);
    i!(I32Const, 0xa0);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xbf);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    i!(LocalGet, L_OP);
    i!(I32Const, 0x1f);
    i!(I32And);
    i!(LocalSet, L_LEN);
    str_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // str8
    i!(LocalGet, L_OP);
    i!(I32Const, 0xd9);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    need_bytes(function, params, 1);
    i!(LocalGet, L_CURSOR);
    i!(I32Load8U, byte_mem());
    i!(LocalSet, L_LEN);
    i!(LocalGet, L_CURSOR);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, L_CURSOR);
    str_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // str16
    i!(LocalGet, L_OP);
    i!(I32Const, 0xda);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 2);
    str_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // str32
    i!(LocalGet, L_OP);
    i!(I32Const, 0xdb);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 4);
    str_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // bin8: a bin in key position rejects before any allocation
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc4);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    need_bytes(function, params, 1);
    i!(LocalGet, L_CURSOR);
    i!(I32Load8U, byte_mem());
    i!(LocalSet, L_LEN);
    i!(LocalGet, L_CURSOR);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, L_CURSOR);
    bin_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // bin16
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc5);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 2);
    bin_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // bin32
    i!(LocalGet, L_OP);
    i!(I32Const, 0xc6);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 4);
    bin_payload(function, params);
    i!(Br, 1); // exit $disp: leaf store + propagation
    i!(End);
    // array16 / array32
    i!(LocalGet, L_OP);
    i!(I32Const, 0xdc);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 2);
    container(function, params, KIND_ARRAY);
    i!(End);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xdd);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 4);
    container(function, params, KIND_ARRAY);
    i!(End);
    // fixarray
    i!(LocalGet, L_OP);
    i!(I32Const, 0x90);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0x9f);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    i!(LocalGet, L_OP);
    i!(I32Const, 0x0f);
    i!(I32And);
    i!(LocalSet, L_LEN);
    container(function, params, KIND_ARRAY);
    i!(End);
    // map16 / map32
    i!(LocalGet, L_OP);
    i!(I32Const, 0xde);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 2);
    container(function, params, KIND_MAP);
    i!(End);
    i!(LocalGet, L_OP);
    i!(I32Const, 0xdf);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    be_len(function, params, 4);
    container(function, params, KIND_MAP);
    i!(End);
    // fixmap
    i!(LocalGet, L_OP);
    i!(I32Const, 0x80);
    i!(I32GeU);
    i!(LocalGet, L_OP);
    i!(I32Const, 0x8f);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    i!(LocalGet, L_OP);
    i!(I32Const, 0x0f);
    i!(I32And);
    i!(LocalSet, L_LEN);
    container(function, params, KIND_MAP);
    i!(End);
    // any other byte is a reserved marker
    fail!(FAIL_RESERVED);
}

/// Post-store propagation loop `$prop`, emitted inside `$main`; all br depths
/// are annotated with their target at the point of emission.
fn propagate(function: &mut Function, params: &DecodeEmit) {
    macro_rules! i {
        ($op:ident $(, $arg:expr)?) => { function.instruction(&Instruction::$op $(($arg))?); };
    }
    macro_rules! fail {
        ($code:expr) => {
            reject_sequence(function, params, $code);
        };
    }
    macro_rules! frame_load {
        ($field:expr, $into:expr) => {{
            i!(LocalGet, L_FRAME);
            i!(I32Load, mem32($field));
            i!(LocalSet, $into);
        }};
    }

    i!(Loop, BlockType::Empty); // $prop
                                // defensive: no frame exists at depth 0 (the scalar root exits earlier)
    i!(LocalGet, L_DEPTH);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    i!(Unreachable);
    i!(End);
    frame_addr(function);
    frame_load!(F_KIND, L_TMP);
    // scalar root envelope: trailing bytes reject, then decoding completes
    i!(LocalGet, L_TMP);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    i!(LocalGet, L_CURSOR);
    i!(LocalGet, L_END);
    i!(I32Ne);
    i!(If, BlockType::Empty);
    fail!(FAIL_TRAIL);
    i!(End);
    i!(Br, 3); // -> $done (scalarIf(0) $prop(1) $main(2) $done(3))
    i!(End);
    // array frame
    i!(LocalGet, L_TMP);
    i!(I32Const, KIND_ARRAY);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    // remaining = COUNT - 1, stored back before the completion branch
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_COUNT));
    i!(I32Const, 1);
    i!(I32Sub);
    i!(LocalSet, L_REMAINING);
    i!(LocalGet, L_FRAME);
    i!(LocalGet, L_REMAINING);
    i!(I32Store, mem32(F_COUNT));
    i!(LocalGet, L_REMAINING);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    // completed list: TAG_LIST cell with aux=AUX, payload=FIRST to DEST
    frame_load!(F_DEST, L_DEST);
    frame_load!(F_AUX, L_AUX);
    frame_load!(F_FIRST, L_TMP);
    i!(LocalGet, L_TMP);
    i!(I64ExtendI32U);
    i!(LocalSet, L_PAYLOAD);
    i!(LocalGet, L_DEST);
    i!(I32Const, TAG_LIST);
    i!(LocalGet, L_AUX);
    i!(LocalGet, L_PAYLOAD);
    i!(Call, params.store_cell_index);
    i!(LocalGet, L_DEPTH);
    i!(I32Const, 1);
    i!(I32Sub);
    i!(LocalSet, L_DEPTH);
    i!(Br, 2); // -> $prop (doneIf(0) arrayIf(1) $prop(2) $main(3) $done(4))
    i!(End);
    i!(Br, 2); // -> $main (arrayIf(0) $prop(1) $main(2) $done(3)): more children
    i!(End);
    // map frame
    i!(LocalGet, L_TMP);
    i!(I32Const, KIND_MAP);
    i!(I32Eq);
    i!(If, BlockType::Empty);
    frame_load!(F_COUNT, L_REMAINING);
    i!(LocalGet, L_REMAINING);
    i!(I32Const, 1);
    i!(I32And);
    i!(If, BlockType::Empty);
    // odd remaining count: the pair value just completed
    i!(LocalGet, L_REMAINING);
    i!(I32Const, 1);
    i!(I32Sub);
    i!(LocalSet, L_REMAINING);
    i!(LocalGet, L_FRAME);
    i!(LocalGet, L_REMAINING);
    i!(I32Store, mem32(F_COUNT));
    i!(LocalGet, L_REMAINING);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    // completed map: TAG_DYNAMIC_MAP cell with aux=pairs (AUX is the pair
    // count), payload=KEYBASE to DEST
    frame_load!(F_DEST, L_DEST);
    frame_load!(F_AUX, L_AUX);
    frame_load!(F_KEYBASE, L_TMP);
    i!(LocalGet, L_TMP);
    i!(I64ExtendI32U);
    i!(LocalSet, L_PAYLOAD);
    i!(LocalGet, L_DEST);
    i!(I32Const, TAG_DYNAMIC_MAP);
    i!(LocalGet, L_AUX);
    i!(LocalGet, L_PAYLOAD);
    i!(Call, params.store_cell_index);
    i!(LocalGet, L_DEPTH);
    i!(I32Const, 1);
    i!(I32Sub);
    i!(LocalSet, L_DEPTH);
    i!(Br, 3); // -> $prop (pairIf(0) valueIf(1) mapIf(2) $prop(3) $main(4) $done(5))
    i!(End);
    i!(Br, 3); // -> $main (valueIf(0) mapIf(1) $prop(2) $main(3) $done(4))
    i!(Else);
    // even remaining count: the key just completed; decrement to odd, then
    // validate it as a unique string before the value is decoded
    i!(LocalGet, L_REMAINING);
    i!(I32Const, 1);
    i!(I32Sub);
    i!(LocalSet, L_REMAINING);
    i!(LocalGet, L_FRAME);
    i!(LocalGet, L_REMAINING);
    i!(I32Store, mem32(F_COUNT));
    // key cell must be a string cell
    i!(LocalGet, L_DEST);
    i!(I32Load, mem32(0));
    i!(I32Const, TAG_STRING);
    i!(I32Ne);
    i!(If, BlockType::Empty);
    fail!(FAIL_KEY);
    i!(End);
    // completed prior pairs = (2 * AUX - COUNT) >> 1 (COUNT is odd here)
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_AUX));
    i!(I32Const, 1);
    i!(I32Shl);
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_COUNT));
    i!(I32Sub);
    i!(I32Const, 1);
    i!(I32ShrU);
    i!(LocalSet, L_PAIRS);
    i!(I32Const, 0);
    i!(LocalSet, L_SCAN);
    // iterative duplicate-key scan over the completed key cells
    i!(Block, BlockType::Empty);
    i!(Loop, BlockType::Empty);
    i!(LocalGet, L_SCAN);
    i!(LocalGet, L_PAIRS);
    i!(I32GeU);
    i!(BrIf, 1); // exit Block (Loop(0) Block(1) ...): scan exhausted
    i!(LocalGet, L_FRAME);
    i!(I32Load, mem32(F_KEYBASE));
    i!(LocalGet, L_SCAN);
    i!(I32Const, 32);
    i!(I32Mul);
    i!(I32Add);
    i!(LocalSet, L_KEYCELL);
    i!(LocalGet, L_KEYCELL);
    i!(I32Load, mem32(4));
    i!(LocalSet, L_TMP);
    // equal lengths: compare bytes; mismatched lengths cannot be duplicates
    i!(LocalGet, L_TMP);
    i!(LocalGet, L_DEST);
    i!(I32Load, mem32(4));
    i!(I32Eq);
    i!(If, BlockType::Empty);
    // compare_bytes(left, left_len, right, right_len): == 0 rejects as dup
    i!(LocalGet, L_KEYCELL);
    i!(I32Load, mem32(8));
    i!(LocalGet, L_TMP);
    i!(LocalGet, L_DEST);
    i!(I32Load, mem32(8));
    i!(LocalGet, L_DEST);
    i!(I32Load, mem32(4));
    i!(Call, params.compare_index);
    i!(I32Eqz);
    i!(If, BlockType::Empty);
    fail!(FAIL_DUP);
    i!(End);
    i!(End);
    i!(LocalGet, L_SCAN);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, L_SCAN);
    i!(Br, 0); // -> $scan Loop: next prior key
    i!(End);
    i!(End);
    i!(Br, 3); // -> $main (keyElse(0) mapIf(1) $prop(2) $main(3) $done(4))
    i!(End);
    i!(End);
    // impossible: unknown frame kind
    i!(Unreachable);
    i!(End); // $prop
}
