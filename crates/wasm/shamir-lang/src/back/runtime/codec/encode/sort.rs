use wasm_encoder::{BlockType, Function, Instruction};

use super::{
    ic, lg, mem32, mem64, st, EncodeEmit, E_CURSOR, E_KIND, E_TOTAL, KIND_MAP, L_BASE, L_CELL,
    L_CMP, L_DEPTH, L_FRAME, L_I, L_J, L_LA, L_LB, L_PA, L_PAIRS, L_PB, L_PREV, L_SCAN, L_TMP,
};
use crate::back::runtime::codec::frames::{CONTEXT_FRAME_BASE_OFFSET, FRAME_BYTES};

/// Key-byte order comparison of the string keys of the pair cells in
/// `L_PREV` and `L_TMP`; the result lands in `L_CMP` (memcmp semantics).
pub(super) fn emit_key_compare(function: &mut Function, params: &EncodeEmit) {
    lg(function, L_PREV);
    function.instruction(&Instruction::I32Load(mem32(4)));
    st(function, L_LA);
    lg(function, L_PREV);
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_PA);
    lg(function, L_TMP);
    function.instruction(&Instruction::I32Load(mem32(4)));
    st(function, L_LB);
    lg(function, L_TMP);
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    st(function, L_PB);
    lg(function, L_LA);
    function.instruction(&Instruction::I32Eqz);
    lg(function, L_LB);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::I32Or);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_LA);
    lg(function, L_LB);
    function.instruction(&Instruction::I32Sub);
    st(function, L_CMP);
    function.instruction(&Instruction::Else);
    lg(function, L_PA);
    lg(function, L_LA);
    lg(function, L_PB);
    lg(function, L_LB);
    function.instruction(&Instruction::Call(params.compare_index));
    st(function, L_CMP);
    function.instruction(&Instruction::End);
}

/// Pair-index window in entries: one `caps.entries` array; nested maps reuse
/// ancestor windows after their subtree completes, and a nesting that would
/// exceed the window rejects (kind 3) instead of spilling into the heap.
pub(super) fn sort_window_entries(params: &EncodeEmit) -> i32 {
    let limit = i32::MAX as u64 / 4;
    (params.caps.entries.min(limit)) as i32
}

/// The encoder window slot one past the nearest open map ancestor's index
/// array; sibling subtrees reuse the slot sequentially, so a single array is
/// live at any time. Without a map ancestor the window starts at `sort_base`.
/// Block/Loop branches are relative, so the depths hold at any dispatch
/// nesting: BrIf 1 exits the Block (scan drained), Br 2 exits the Block from
/// inside the map check, Br 0 repeats the Loop.
pub(super) fn resolve_sort_base(function: &mut Function, params: &EncodeEmit) {
    ic(function, params.sort_base as i32);
    st(function, L_BASE);
    lg(function, L_DEPTH);
    st(function, L_SCAN);
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    lg(function, L_SCAN);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::BrIf(1)); // exit Block
    lg(function, 0);
    function.instruction(&Instruction::I32Load(mem32(CONTEXT_FRAME_BASE_OFFSET)));
    lg(function, L_SCAN);
    ic(function, FRAME_BYTES as i32);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    ic(function, FRAME_BYTES as i32);
    function.instruction(&Instruction::I32Sub);
    st(function, L_FRAME);
    lg(function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_KIND)));
    ic(function, KIND_MAP);
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    lg(function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_CURSOR)));
    lg(function, L_FRAME);
    function.instruction(&Instruction::I32Load(mem32(E_TOTAL)));
    ic(function, 1);
    function.instruction(&Instruction::I32ShrU);
    ic(function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    st(function, L_BASE);
    function.instruction(&Instruction::Br(2)); // exit Block (inside If)
    function.instruction(&Instruction::End);
    lg(function, L_SCAN);
    ic(function, 1);
    function.instruction(&Instruction::I32Sub);
    st(function, L_SCAN);
    function.instruction(&Instruction::Br(0)); // Loop
    function.instruction(&Instruction::End); // Loop
    function.instruction(&Instruction::End); // Block
}

/// `arr[i] = map_cell + 32*i` for the pairs count in `L_PAIRS`; relative
/// branches: BrIf 1 exits the Block, Br 0 repeats the Loop.
pub(super) fn build_pair_index(function: &mut Function, _params: &EncodeEmit) {
    ic(function, 0);
    st(function, L_I);
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    lg(function, L_I);
    lg(function, L_PAIRS);
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1)); // exit Block
    lg(function, L_BASE);
    lg(function, L_I);
    ic(function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    lg(function, L_CELL);
    lg(function, L_I);
    ic(function, 32);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Store(mem32(0)));
    lg(function, L_I);
    ic(function, 1);
    function.instruction(&Instruction::I32Add);
    st(function, L_I);
    function.instruction(&Instruction::Br(0)); // Loop
    function.instruction(&Instruction::End); // Loop
    function.instruction(&Instruction::End); // Block
}

/// Iterative insertion sort of the index array in `L_BASE` by UTF-8 key
/// bytes; charges guest fuel exactly like the decoder duplicate scan. The
/// inner loop keeps shifting while `prev > tmp` (signed compare result) and
/// stops once the previous key sorts before or equal to the moving key. Relative branches: BrIf 1 exits
/// the outer Block, Br 0 repeats the outer Loop; the inner BrIf 1 exits the
/// inner Block and Br 0 repeats the inner Loop.
pub(super) fn insertion_sort_pairs(function: &mut Function, params: &EncodeEmit) {
    ic(function, 1);
    st(function, L_I);
    function.instruction(&Instruction::Block(BlockType::Empty)); // outer Block
    function.instruction(&Instruction::Loop(BlockType::Empty)); // outer Loop
    lg(function, L_I);
    lg(function, L_PAIRS);
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1)); // exit outer Block
    lg(function, L_BASE);
    lg(function, L_I);
    ic(function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load(mem32(0)));
    st(function, L_TMP);
    lg(function, L_I);
    st(function, L_J);
    function.instruction(&Instruction::Block(BlockType::Empty)); // inner Block
    function.instruction(&Instruction::Loop(BlockType::Empty)); // inner Loop
    lg(function, L_J);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::BrIf(1)); // exit inner Block
    lg(function, L_BASE);
    lg(function, L_J);
    ic(function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    ic(function, 4);
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Load(mem32(0)));
    st(function, L_PREV);
    emit_key_compare(function, params);
    lg(function, L_CMP);
    ic(function, 0);
    function.instruction(&Instruction::I32LeS);
    function.instruction(&Instruction::BrIf(1)); // prev <= tmp: stop shifting
    lg(function, L_BASE);
    lg(function, L_J);
    ic(function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    lg(function, L_PREV);
    function.instruction(&Instruction::I32Store(mem32(0)));
    lg(function, L_J);
    ic(function, 1);
    function.instruction(&Instruction::I32Sub);
    st(function, L_J);
    function.instruction(&Instruction::Br(0)); // inner Loop
    function.instruction(&Instruction::End); // inner Loop
    function.instruction(&Instruction::End); // inner Block
    lg(function, L_BASE);
    lg(function, L_J);
    ic(function, 4);
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    lg(function, L_TMP);
    function.instruction(&Instruction::I32Store(mem32(0)));
    lg(function, L_I);
    ic(function, 1);
    function.instruction(&Instruction::I32Add);
    st(function, L_I);
    function.instruction(&Instruction::Br(0)); // outer Loop
    function.instruction(&Instruction::End); // outer Loop
    function.instruction(&Instruction::End); // outer Block
}
