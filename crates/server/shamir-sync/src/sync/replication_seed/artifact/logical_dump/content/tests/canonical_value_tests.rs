//! Canonical-value rejection cases (hand-built MessagePack wire bytes) and
//! the seed Manifest reference-digest vector.

use super::super::super::codec::{decode_canonical, decode_value, encode_canonical, encode_value};
use super::super::super::content::LogicalValue;
use crate::replication_seed::codec::{decode, digest, encode};
use crate::replication_seed::{Manifest, TableRecord};

/// Two-element tagged envelope: `92 <tag> <payload>`.
fn envelope(tag: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x92, 0xa0 + tag.len() as u8];
    bytes.extend_from_slice(tag.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn fixed_bin(bytes: &[u8]) -> Vec<u8> {
    let mut wire = vec![0xc4, bytes.len() as u8];
    wire.extend_from_slice(bytes);
    wire
}

fn value_bytes(tag: &str, payload: &[u8]) -> Vec<u8> {
    envelope(tag, &fixed_bin(payload))
}

/// An inner logical value: `92 <tag> <payload>` (envelope itself).
fn inner(tag: &str, payload: &[u8]) -> Vec<u8> {
    value_bytes(tag, payload)
}

#[test]
fn duplicate_map_key_is_rejected() {
    let mut pair = vec![0x92, 0xa1, b'a'];
    pair.extend_from_slice(&inner("int64", &[0; 8]));
    let mut full = vec![0x92, 0xa3, b'm', b'a', b'p', 0x92];
    full.extend_from_slice(&pair);
    full.extend_from_slice(&pair);
    let error = decode_value(&full).expect_err("duplicate map key");
    assert!(
        error.to_string().contains("duplicate logical map key"),
        "unexpected: {error}"
    );
}

#[test]
fn duplicate_set_element_is_rejected() {
    let element = inner("str", b"first");
    let mut full = vec![0x92, 0xa3, b's', b'e', b't', 0x92];
    full.extend_from_slice(&element);
    full.extend_from_slice(&element);
    let error = decode_value(&full).expect_err("duplicate set element");
    assert!(
        error.to_string().contains("duplicate value in logical Set"),
        "unexpected: {error}"
    );
}

#[test]
fn negative_zero_decimal_is_rejected() {
    let mut payload = [0_u8; 17];
    payload[0] = 1; // negative sign over a zero coefficient
    let wire = value_bytes("dec", &payload);
    let error = decode_value(&wire).expect_err("negative zero decimal");
    assert!(
        error.to_string().contains("noncanonical decimal"),
        "unexpected: {error}"
    );
}

#[test]
fn oversized_decimal_scale_is_rejected() {
    let mut payload = [0_u8; 17];
    payload[1] = 1;
    payload[13..17].copy_from_slice(&29_u32.to_be_bytes());
    let wire = value_bytes("dec", &payload);
    let error = decode_value(&wire).expect_err("scale above 28");
    assert!(
        error.to_string().contains("noncanonical decimal"),
        "unexpected: {error}"
    );
}

#[test]
fn oversized_decimal_coefficient_is_rejected_at_encode() {
    let error = encode_value(&LogicalValue::Decimal {
        coefficient: 1_u128 << 96,
        scale: 0,
        negative: false,
    })
    .expect_err("coefficient above 96 bits");
    assert!(
        error.to_string().contains("outside supported range"),
        "unexpected: {error}"
    );
}

#[test]
fn noncanonical_big_forms_are_rejected() {
    for literal in ["+1", "01", "-0", "1.0", ""] {
        let wire = envelope("big", literal.as_bytes());
        let error = decode_value(&wire).expect_err("noncanonical Big");
        assert!(
            error.to_string().contains("noncanonical Big integer"),
            "unexpected: {error}"
        );
        let error = encode_value(&LogicalValue::Big(literal.into()))
            .expect_err("noncanonical Big cannot encode");
        assert!(
            error.to_string().contains("noncanonical Big integer"),
            "unexpected: {error}"
        );
    }
}

#[test]
fn unknown_value_tag_is_rejected() {
    let wire = envelope("boolx", &[0xc0]);
    let error = decode_value(&wire).expect_err("unknown tag");
    assert!(
        error.to_string().contains("unknown logical value tag"),
        "unexpected: {error}"
    );
}

#[test]
fn wrong_length_fixed_payload_is_rejected() {
    let wire = value_bytes("int64", &[0; 7]);
    let error = decode_value(&wire).expect_err("short integer payload");
    assert!(
        error.to_string().contains("wrong length"),
        "unexpected: {error}"
    );
    let wire = value_bytes("f64", &[0; 9]);
    let error = decode_value(&wire).expect_err("long float payload");
    assert!(
        error.to_string().contains("wrong length"),
        "unexpected: {error}"
    );
    let wire = value_bytes("dec", &[0; 16]);
    let error = decode_value(&wire).expect_err("short decimal payload");
    assert!(
        error.to_string().contains("wrong length"),
        "unexpected: {error}"
    );
}

#[test]
fn extra_envelope_element_is_rejected() {
    let mut wire = envelope("null", &[0xc0]);
    wire[0] = 0x93; // three-element array
    wire.push(0xc0);
    let error = decode_value(&wire).expect_err("extra element");
    assert!(
        error.to_string().contains("two elements") || error.to_string().contains("extra"),
        "unexpected: {error}"
    );
}

#[test]
fn trailing_bytes_after_a_value_are_rejected() {
    let value = LogicalValue::Str("x".into());
    let mut wire = encode_value(&value).expect("value encodes");
    wire.push(0x00);
    let error = decode_value(&wire).expect_err("trailing bytes");
    assert!(
        error.to_string().contains("trailing"),
        "unexpected: {error}"
    );

    let valid = encode_canonical(&value).expect("canonical encode");
    let mut trailing = valid.clone();
    trailing.push(0x01);
    let error = decode_canonical::<serde_json::Value>(&trailing).expect_err("trailing bytes");
    assert!(
        error.to_string().contains("trailing MessagePack bytes"),
        "unexpected: {error}"
    );
}

#[test]
fn seed_manifest_digest_matches_committed_reference() {
    let manifest = Manifest {
        format_version: 1,
        codec_version: 1,
        requirements: Vec::new(),
        source_db: "app".into(),
        source_repo: "main".into(),
        lineage_id: [1; 16],
        history_epoch: 2,
        version: 7,
        certified_gap: None,
        schema_sha256: "0".repeat(64),
        artifact_id: [3; 16],
        index_policy: "EXCLUDED".into(),
        tables: Vec::<TableRecord>::new(),
        files: Vec::new(),
    };
    let bytes = encode(&manifest).expect("manifest encodes");
    assert_eq!(
        digest(&bytes),
        "b4b4b6c528217c2a11b4d074dd4e9485ca06378e56a179e5b90c568e9b96f772"
    );
    let decoded: Manifest = decode(&bytes).expect("manifest decodes");
    assert_eq!(
        encode(&decoded).expect("decoded manifest re-encodes"),
        bytes,
        "round trip must be byte-identical"
    );
}
