//! M8 raw-byte conformance of the guest codec: the design's codec vectors,
//! shortest-integer pins and the second-call guard are executed against the
//! compiled guest runtime itself, not only against the reference decoder.
//!
//! The runtime module's `shamir_call` decodes the root value and encodes it
//! again, so an accepted input comes back as its canonical bytes and a
//! rejected one ends in a kind-3 `raise_error` whose message is the class
//! marker. Each call gets a fresh instance: a second call on one instance is
//! the guard's subject (`second_call_on_one_instance_raises_reentry`).
use super::harness::{hex, raw_call, raw_module};
use shamir_lang::testsupport::{codec_vectors, runtime_module, VectorExpectation};

/// Codec rejections surface as raise kind 3.
const KIND_CODEC: i32 = 3;
/// The re-entry guard raises kind 4.
const KIND_REENTRY: i32 = 4;

#[tokio::test]
async fn every_accepted_vector_round_trips_to_canonical_bytes_in_the_guest() {
    let wasm = runtime_module();
    let mut executed = 0usize;
    for vector in codec_vectors() {
        let VectorExpectation::Accepted(canonical) = &vector.expectation else {
            continue;
        };
        let mut module = raw_module(&wasm).await;
        let output = raw_call(&mut module, &vector.input)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "vector {} trapped: {error}; raised {:?}",
                    vector.name,
                    module.raised()
                )
            });
        assert_eq!(&output, canonical, "vector {}", vector.name);
        assert_eq!(module.raised(), None, "vector {} raised", vector.name);
        executed += 1;
    }
    assert!(executed >= 14, "only {executed} accepted vectors executed");
}

#[tokio::test]
async fn every_rejected_vector_raises_its_class_marker_in_the_guest() {
    let wasm = runtime_module();
    let mut executed = 0usize;
    for vector in codec_vectors() {
        let VectorExpectation::Rejected(marker) = &vector.expectation else {
            continue;
        };
        let mut module = raw_module(&wasm).await;
        let result = raw_call(&mut module, &vector.input).await;
        assert!(result.is_err(), "vector {} must be rejected", vector.name);
        assert_eq!(
            module.raised(),
            Some((KIND_CODEC, (*marker).to_string())),
            "vector {}",
            vector.name
        );
        executed += 1;
    }
    assert!(executed >= 17, "only {executed} rejected vectors executed");
}

/// Shortest-width canonical bytes per value (`derivation:` the encoder's
/// width table: fixint, uint8, uint16, uint32 for non-negatives up to
/// `u32::MAX`, uint64 above; fixint, int8, int16, int32 for negatives, int64
/// below `i32::MIN`).
const SHORTEST_INTEGERS: [(i64, &str); 22] = [
    (0, "00"),
    (127, "7f"),
    (128, "cc 80"),
    (255, "cc ff"),
    (256, "cd 01 00"),
    (65_535, "cd ff ff"),
    (65_536, "ce 00 01 00 00"),
    (4_294_967_295, "ce ff ff ff ff"),
    (4_294_967_296, "cf 00 00 00 01 00 00 00 00"),
    (i64::MAX, "cf 7f ff ff ff ff ff ff ff"),
    (-1, "ff"),
    (-32, "e0"),
    (-33, "d0 df"),
    (-128, "d0 80"),
    (-129, "d1 ff 7f"),
    (-32_768, "d1 80 00"),
    (-32_769, "d2 ff ff 7f ff"),
    (-2_147_483_648, "d2 80 00 00 00"),
    (-2_147_483_649, "d3 ff ff ff ff 7f ff ff ff"),
    (-4_294_967_296, "d3 ff ff ff ff 00 00 00 00"),
    (i64::MIN + 1, "d3 80 00 00 00 00 00 00 01"),
    (i64::MIN, "d3 80 00 00 00 00 00 00 00"),
];

#[tokio::test]
async fn integers_in_any_wide_form_re_encode_to_the_shortest_form() {
    let wasm = runtime_module();
    for (value, expected) in SHORTEST_INTEGERS {
        let mut wide = vec![0xd3];
        wide.extend_from_slice(&value.to_be_bytes());
        let mut module = raw_module(&wasm).await;
        let output = raw_call(&mut module, &wide)
            .await
            .unwrap_or_else(|error| panic!("int64 form of {value} trapped: {error}"));
        assert_eq!(output, hex(expected), "int64 form of {value}");
        if value >= 0 {
            let mut unsigned = vec![0xcf];
            unsigned.extend_from_slice(&(value as u64).to_be_bytes());
            let mut module = raw_module(&wasm).await;
            let output = raw_call(&mut module, &unsigned)
                .await
                .unwrap_or_else(|error| panic!("uint64 form of {value} trapped: {error}"));
            assert_eq!(output, hex(expected), "uint64 form of {value}");
        }
    }
}

#[tokio::test]
async fn map_keys_are_emitted_in_canonical_order_whatever_the_input_order() {
    // derivation: canonical order is the UTF-8 key-byte order (vector
    // `map_canonical_key_order`: "a" before "b"); "a" < "ab" < "bb" also pins
    // that a prefix sorts before its extension.
    let wasm = runtime_module();
    // {"bb": 1, "a": 2, "ab": 3} in input order bb, a, ab
    let input = hex("83 a2 62 62 01 a1 61 02 a2 61 62 03");
    let mut module = raw_module(&wasm).await;
    let output = raw_call(&mut module, &input).await.expect("map accepted");
    assert_eq!(output, hex("83 a1 61 02 a2 61 62 03 a2 62 62 01"));
}

#[tokio::test]
async fn second_call_on_one_instance_raises_reentry() {
    // derivation: the call wrapper sets the `call_state` global once its
    // early exits are passed and raises kind 4 with the ASCII message
    // "reentry" when it is already set.
    let wasm = runtime_module();
    let mut module = raw_module(&wasm).await;
    let first = raw_call(&mut module, &hex("c0"))
        .await
        .expect("first call succeeds");
    assert_eq!(first, hex("c0"));
    assert_eq!(module.raised(), None);
    let second = raw_call(&mut module, &hex("c0")).await;
    assert!(second.is_err(), "the second call must trap");
    assert_eq!(module.raised(), Some((KIND_REENTRY, "reentry".to_string())));
}
