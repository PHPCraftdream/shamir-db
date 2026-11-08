use wasm_encoder::Encode;

use wasm_encoder::ValType;

use super::super::frames::{FRAME_BYTES, FRAME_COUNT};
use crate::back::runtime::codec::decode::{
    emit_decode_root, reject_message, DecodeEmit, LOCAL_TYPES,
};
use crate::back::runtime::layout::RuntimeCaps;

/// Stand-in indices; emission only encodes them into `call` operands.
fn fixture() -> DecodeEmit {
    DecodeEmit {
        scratch: 64,
        raise_index: 1,
        alloc_index: 2,
        check_range_index: 3,
        read_byte_index: 4,
        read_be_index: 5,
        compare_index: 6,
        utf8_index: 7,
        store_cell_index: 8,
        caps: RuntimeCaps::default(),
    }
}

#[test]
fn reject_messages_are_short_distinct_ascii_literals() {
    let mut seen: Vec<&[u8]> = Vec::new();
    for code in 0..=16 {
        let message = reject_message(code);
        assert!(!message.is_empty());
        assert!(message.len() <= 8);
        assert!(message.iter().all(|byte| (0x21..=0x7e).contains(byte)));
        assert!(!seen.contains(&message));
        seen.push(message);
    }
}

#[test]
fn decode_root_emission_is_deterministic() {
    let mut first = Vec::new();
    emit_decode_root(&fixture()).encode(&mut first);
    let mut second = Vec::new();
    emit_decode_root(&fixture()).encode(&mut second);
    assert_eq!(first, second);
    assert!(!first.is_empty());
}

#[test]
fn decode_root_body_ends_with_implicit_end_opcode() {
    let mut bytes = Vec::new();
    emit_decode_root(&fixture()).encode(&mut bytes);
    assert_eq!(*bytes.last().expect("encoded function body"), 0x0b);
}

#[test]
fn decoder_local_type_map_matches_named_indices() {
    // first and last declared i32 locals
    assert_eq!(LOCAL_TYPES[1], ValType::I32);
    assert_eq!(LOCAL_TYPES[21], ValType::I32);
    // i64 payload and shift-scratch locals
    assert_eq!(LOCAL_TYPES[22], ValType::I64);
    assert_eq!(LOCAL_TYPES[23], ValType::I64);
}

const _: () = assert!(FRAME_COUNT >= 32);

#[test]
fn frame_stack_holds_the_full_depth_budget() {
    assert_eq!(FRAME_BYTES, 32);
}
