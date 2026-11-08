use wasm_encoder::{BlockType, Function, Instruction};

use super::reject::{reject_sequence, FAIL_CELLS, FAIL_CONTAINERS, FAIL_DEPTH, FAIL_ENTRIES};
use super::root::DecodeEmit;
use super::root::{
    cap32, frame_addr, mem32, F_AUX, F_COUNT, F_DEST, F_FIRST, F_KEYBASE, F_KIND, L_AUX, L_BUMP,
    L_CELLS, L_CONTAINERS, L_DEPTH, L_DEST, L_FRAME, L_LEN, L_PAYLOAD, L_TAG, TAG_DYNAMIC_MAP,
    TAG_LIST,
};

/// Array/map count in local 10, emitted inside a `$disp` dispatch branch
/// (branchIf(0) $disp(1) $main(2) $done(3)). The header count is PAIRS for
/// maps. Every cap is checked before the frame is pushed; the whole child
/// block is reserved from the bump before descending, which guarantees list
/// children stay contiguous (16B cells) and map pairs contiguous (32B).
pub(super) fn container(function: &mut Function, params: &DecodeEmit, kind: i32) {
    let caps = &params.caps;
    // per-container entry cap (pairs for maps)
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Const(cap32(caps.entries)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_ENTRIES);
    function.instruction(&Instruction::End);
    // cumulative container cap; empty containers count too
    function.instruction(&Instruction::LocalGet(L_CONTAINERS));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Const(cap32(caps.containers)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_CONTAINERS);
    function.instruction(&Instruction::End);
    // nesting depth cap, checked before pushing or completing an empty container: L_DEPTH counts the synthetic
    // scalar-root frame, so a new container is allowed while the frames
    // including it stay within `caps.depth + 1`
    function.instruction(&Instruction::LocalGet(L_DEPTH));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Const(cap32(u64::from(caps.depth) + 1)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_DEPTH);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    // empty container: completes immediately as a leaf cell at dest
    container_empty(function, kind);
    function.instruction(&Instruction::LocalGet(L_CONTAINERS));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CONTAINERS));
    function.instruction(&Instruction::Br(2)); // exit $disp -> leaf store
                                               // (emptyIf(0) branchIf(1) $disp(2) $main(3) $done(4))
    function.instruction(&Instruction::Else);
    container_push(function, params, kind);
    function.instruction(&Instruction::Br(3)); // -> $main: decode first child
                                               // (else(0) branchIf(1) $disp(2) $main(3) $done(4))
    function.instruction(&Instruction::End);
}

/// Empty container leaf: tag/aux/payload locals for the shared leaf store.
/// The payload points at the current bump (no cell is consumed).
fn container_empty(function: &mut Function, kind: i32) {
    let tag = if kind == super::root::KIND_MAP {
        TAG_DYNAMIC_MAP
    } else {
        TAG_LIST
    };
    function.instruction(&Instruction::I32Const(tag));
    function.instruction(&Instruction::LocalSet(L_TAG));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(L_AUX));
    function.instruction(&Instruction::LocalGet(L_BUMP));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::LocalSet(L_PAYLOAD));
}

/// Non-empty container: reserve the whole child window from the bump, push
/// the frame, and hand decoding back to `$main`. List children are contiguous
/// 16B cells; map pairs are contiguous 32B — the reservation happens BEFORE
/// descending, so no sibling allocation can split a child block.
fn container_push(function: &mut Function, params: &DecodeEmit, kind: i32) {
    let caps = &params.caps;
    let is_map = kind == super::root::KIND_MAP;
    // aggregate cell budget for every eventual element cell, then charge it
    function.instruction(&Instruction::LocalGet(L_CELLS));
    function.instruction(&Instruction::LocalGet(L_LEN));
    if is_map {
        function.instruction(&Instruction::I32Const(2));
        function.instruction(&Instruction::I32Mul);
    }
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Const(cap32(caps.cells)));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    reject_sequence(function, params, FAIL_CELLS);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(L_CELLS));
    function.instruction(&Instruction::LocalGet(L_LEN));
    if is_map {
        function.instruction(&Instruction::I32Const(2));
        function.instruction(&Instruction::I32Mul);
    }
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CELLS));
    function.instruction(&Instruction::LocalGet(L_CONTAINERS));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_CONTAINERS));
    function.instruction(&Instruction::LocalGet(L_DEPTH));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_DEPTH));
    frame_addr(function);
    // frame fields, each stored address-first: the store pops the VALUE from
    // the top, so `LocalGet(L_FRAME)` goes deepest, then the value
    function.instruction(&Instruction::LocalGet(L_FRAME));
    function.instruction(&Instruction::I32Const(kind));
    function.instruction(&Instruction::I32Store(mem32(F_KIND)));
    // COUNT counts elements (pairs*2 for maps)
    function.instruction(&Instruction::LocalGet(L_FRAME));
    function.instruction(&Instruction::LocalGet(L_LEN));
    if is_map {
        function.instruction(&Instruction::I32Const(2));
        function.instruction(&Instruction::I32Mul);
    }
    function.instruction(&Instruction::I32Store(mem32(F_COUNT)));
    // AUX is the raw header count (pairs for maps)
    function.instruction(&Instruction::LocalGet(L_FRAME));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Store(mem32(F_AUX)));
    // FIRST and KEYBASE both start at the reserved window
    function.instruction(&Instruction::LocalGet(L_FRAME));
    function.instruction(&Instruction::LocalGet(L_BUMP));
    function.instruction(&Instruction::I32Store(mem32(F_FIRST)));
    function.instruction(&Instruction::LocalGet(L_FRAME));
    function.instruction(&Instruction::LocalGet(L_BUMP));
    function.instruction(&Instruction::I32Store(mem32(F_KEYBASE)));
    // DEST was computed by the caller for this slot
    function.instruction(&Instruction::LocalGet(L_FRAME));
    function.instruction(&Instruction::LocalGet(L_DEST));
    function.instruction(&Instruction::I32Store(mem32(F_DEST)));
    // bump advances past the reserved child window
    function.instruction(&Instruction::LocalGet(L_BUMP));
    function.instruction(&Instruction::LocalGet(L_LEN));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Mul);
    if is_map {
        function.instruction(&Instruction::I32Const(1));
        function.instruction(&Instruction::I32Shl);
    }
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(L_BUMP));
}
