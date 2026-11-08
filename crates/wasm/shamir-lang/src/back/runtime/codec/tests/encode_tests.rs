use wasm_encoder::{Encode, ValType};

use super::super::decode::reject_message;
use super::super::encode::{emit_encode_root, EncodeEmit, LOCAL_TYPES};
use crate::back::runtime::layout::RuntimeCaps;

/// Stand-in indices; emission only encodes them into `call` operands.
fn fixture() -> EncodeEmit {
    EncodeEmit {
        scratch: 64,
        sort_base: 128,
        raise_index: 1,
        append_index: 2,
        compare_index: 3,
        caps: RuntimeCaps::default(),
        records: false,
    }
}

#[test]
fn reject_messages_include_encoder_classes_and_stay_unique() {
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
fn encode_root_emission_is_deterministic() {
    let mut first = Vec::new();
    emit_encode_root(&fixture()).encode(&mut first);
    let mut second = Vec::new();
    emit_encode_root(&fixture()).encode(&mut second);
    assert_eq!(first, second);
    assert!(!first.is_empty());
}

#[test]
fn encode_root_body_ends_with_implicit_end_opcode() {
    let mut bytes = Vec::new();
    emit_encode_root(&fixture()).encode(&mut bytes);
    assert_eq!(*bytes.last().expect("encoded function body"), 0x0b);
}

/// The exported `LOCAL_TYPES` map must match the local layout: index 0 is the
/// ctx parameter, indices 1..=22 are i32 scratch, 23/24 the i64 payload/value
/// slots; the tail beyond index 22 is i64.
#[test]
fn encoder_local_types_match_the_documented_layout() {
    assert_eq!(LOCAL_TYPES.len(), 25);
    assert_eq!(LOCAL_TYPES[0], ValType::I32);
    assert_eq!(LOCAL_TYPES[1], ValType::I32);
    assert_eq!(LOCAL_TYPES[22], ValType::I32);
    assert_eq!(LOCAL_TYPES[23], ValType::I64);
    assert_eq!(LOCAL_TYPES[24], ValType::I64);
}

/// Typed-record traversal is an additive M4 feature: with it off the runtime
/// encoder is the M1 one, with it on the body strictly grows.
#[test]
fn record_traversal_is_emitted_only_when_enabled() {
    let mut without = Vec::new();
    emit_encode_root(&fixture()).encode(&mut without);
    let mut with = Vec::new();
    emit_encode_root(&EncodeEmit {
        records: true,
        ..fixture()
    })
    .encode(&mut with);
    assert!(with.len() > without.len());
}
