//! Codec acceptance and rejection vectors from design section 9 ("Runtime
//! codec") and the section 5 "Codec contract" paragraph.
//!
//! Each vector is `(name, raw input bytes, expectation)`. `Accepted` carries
//! the canonical bytes the guest runtime must re-encode (shortest widths,
//! map keys sorted); `Rejected` carries the ASCII literal the kind-3 codec
//! raise surfaces to the host. Fixture data lives in code, not files.
use crate::testsupport::msgpack::{
    enc_array, enc_bin, enc_bool, enc_call_envelope, enc_f64, enc_i64, enc_map_sorted, enc_nil,
    enc_str,
};

/// What the guest runtime must do with a vector's input bytes.
pub enum VectorExpectation {
    /// Decoder accepts; the encoder must emit exactly these canonical bytes.
    Accepted(Vec<u8>),
    /// Decoder rejects with a kind-3 codec raise carrying this ASCII literal
    /// (surfaced host-side as `guest codec: <literal>`).
    Rejected(&'static str),
}

/// One codec vector: raw MessagePack input plus the expected outcome.
pub struct CodecVector {
    /// Stable vector name for test failure messages.
    pub name: &'static str,
    /// Raw input bytes handed to the guest decoder.
    pub input: Vec<u8>,
    /// Expected decode outcome.
    pub expectation: VectorExpectation,
}

/// Root array nested `depth` levels deep, `nil` at the bottom (0x91 chain).
fn nested_arrays(depth: usize) -> Vec<u8> {
    let mut out = vec![0x91; depth];
    out.push(0xc0);
    out
}

fn accept(out: &mut Vec<CodecVector>, name: &'static str, input: Vec<u8>, canonical: Vec<u8>) {
    out.push(CodecVector {
        name,
        input,
        expectation: VectorExpectation::Accepted(canonical),
    });
}

fn reject(out: &mut Vec<CodecVector>, name: &'static str, input: Vec<u8>, marker: &'static str) {
    out.push(CodecVector {
        name,
        input,
        expectation: VectorExpectation::Rejected(marker),
    });
}

/// All M1 codec vectors: every acceptance class (each tag, f32 promotion,
/// shortest ints, canonical map order, depth boundary) and every rejection
/// class named in the Codec contract paragraph.
pub fn codec_vectors() -> Vec<CodecVector> {
    let mut out = Vec::new();
    accept(&mut out, "nil", vec![0xc0], enc_nil());
    accept(&mut out, "bool_true", vec![0xc3], enc_bool(true));
    accept(&mut out, "bool_false", vec![0xc2], enc_bool(false));
    accept(&mut out, "int_negative_fixint", vec![0xfb], enc_i64(-5));
    accept(
        &mut out,
        "int_u16_shortest",
        vec![0xcd, 0x01, 0x2c],
        enc_i64(300),
    );
    accept(
        &mut out,
        "int_i64_min",
        {
            let mut bytes = vec![0xd3];
            bytes.extend_from_slice(&i64::MIN.to_be_bytes());
            bytes
        },
        enc_i64(i64::MIN),
    );
    accept(
        &mut out,
        "f64_basic",
        {
            let mut bytes = vec![0xcb];
            bytes.extend_from_slice(&42.5f64.to_bits().to_be_bytes());
            bytes
        },
        enc_f64(42.5),
    );
    accept(
        &mut out,
        "f32_promotes_exactly",
        vec![0xca, 0x42, 0x28, 0x00, 0x00],
        enc_f64(42.0),
    );
    accept(
        &mut out,
        "string_fixstr",
        enc_str("héllo"),
        enc_str("héllo"),
    );
    accept(
        &mut out,
        "bin_shortest",
        enc_bin(&[0x00, 0x01, 0x02, 0xff]),
        enc_bin(&[0x00, 0x01, 0x02, 0xff]),
    );
    accept(
        &mut out,
        "array_heterogeneous",
        enc_array(&[enc_i64(1), enc_str("two"), enc_nil()]),
        enc_array(&[enc_i64(1), enc_str("two"), enc_nil()]),
    );
    accept(
        &mut out,
        "map_canonical_key_order",
        // deliberately unsorted input: b before a
        {
            let mut bytes = vec![0x82];
            bytes.extend_from_slice(&enc_str("b"));
            bytes.extend_from_slice(&enc_i64(1));
            bytes.extend_from_slice(&enc_str("a"));
            bytes.extend_from_slice(&enc_i64(2));
            bytes
        },
        enc_map_sorted(&[(enc_str("a"), enc_i64(2)), (enc_str("b"), enc_i64(1))]),
    );
    accept(
        &mut out,
        "call_envelope_positional",
        enc_call_envelope(&[3, 5]),
        enc_call_envelope(&[3, 5]),
    );
    // Depth boundary per design section 5: the root value is the envelope and
    // counts as the first container, so a root array chain of 32 is the last
    // accepted level and 33 (below) is rejected.
    accept(
        &mut out,
        "depth_32_boundary",
        nested_arrays(32),
        nested_arrays(32),
    );

    // ── rejection: every Codec-contract class ──
    reject(&mut out, "ext8", vec![0xc7, 0x02, 0x05, 0x01, 0x02], "ext");
    reject(
        &mut out,
        "fixext4",
        vec![0xd6, 0x05, 0x01, 0x02, 0x03, 0x04],
        "ext",
    );
    reject(
        &mut out,
        "timestamp_fixext4",
        vec![0xd6, 0xff, 0x00, 0x00, 0x00, 0x00],
        "ext",
    );
    reject(&mut out, "reserved_marker", vec![0xc1], "reserved");
    reject(
        &mut out,
        "utf8_overlong_c0_80",
        {
            let mut bytes = vec![0xd9, 0x02];
            bytes.extend_from_slice(&[0xc0, 0x80]);
            bytes
        },
        "utf8",
    );
    reject(
        &mut out,
        "utf8_surrogate",
        {
            let mut bytes = vec![0xd9, 0x03];
            bytes.extend_from_slice(&[0xed, 0xa0, 0x80]);
            bytes
        },
        "utf8",
    );
    reject(
        &mut out,
        "utf8_above_10ffff",
        {
            let mut bytes = vec![0xd9, 0x04];
            bytes.extend_from_slice(&[0xf4, 0x90, 0x80, 0x80]);
            bytes
        },
        "utf8",
    );
    reject(
        &mut out,
        "utf8_truncated_continuation",
        vec![0xa1, 0xc3],
        "utf8",
    );
    reject(
        &mut out,
        "map_non_string_key",
        vec![0x81, 0x01, 0xc0],
        "key",
    );
    reject(
        &mut out,
        "map_duplicate_keys",
        {
            let mut bytes = vec![0x82];
            bytes.extend_from_slice(&enc_str("a"));
            bytes.extend_from_slice(&enc_i64(1));
            bytes.extend_from_slice(&enc_str("a"));
            bytes.extend_from_slice(&enc_i64(2));
            bytes
        },
        "dup",
    );
    reject(
        &mut out,
        "truncated_string_body",
        vec![0xa5, 0x61, 0x62],
        "trunc",
    );
    reject(
        &mut out,
        "truncated_array_elements",
        vec![0x93, 0x01],
        "trunc",
    );
    reject(
        &mut out,
        "trailing_bytes_after_root",
        vec![0xc0, 0x01],
        "trail",
    );
    reject(&mut out, "depth_33_over_limit", nested_arrays(33), "depth");
    // 32-bit array header so the count check fires before any element read:
    // 100_001 entries exceeds the 100_000 entries cap.
    reject(
        &mut out,
        "array_count_over_entries_cap",
        {
            let mut bytes = vec![0xdd];
            bytes.extend_from_slice(&100_001u32.to_be_bytes());
            bytes
        },
        "ent",
    );
    reject(
        &mut out,
        "u64_above_i64_max",
        {
            let mut bytes = vec![0xcf];
            bytes.extend_from_slice(&(i64::MAX as u64 + 1).to_be_bytes());
            bytes
        },
        "u64",
    );
    // two map keys of 32_769 bytes each (legal str16 lengths) total 65_538
    // key bytes, exceeding the 64 KiB (65_536) cumulative key cap.
    reject(
        &mut out,
        "map_key_bytes_over_cap",
        {
            let mut bytes = vec![0xde, 0x00, 0x02];
            for _ in 0..2 {
                bytes.push(0xda); // str16: one marker byte, then BE u16 length
                bytes.extend_from_slice(&32_769u16.to_be_bytes());
                bytes.extend_from_slice(&vec![b'a'; 32_769]);
                bytes.push(0xc0); // nil value
            }
            bytes
        },
        "keys",
    );
    // str32 header with a length above the 1 MiB string cap; the length
    // check fires before the (here absent) body is read.
    reject(
        &mut out,
        "string_length_over_cap",
        {
            let mut bytes = vec![0xdb];
            bytes.extend_from_slice(&1_048_577u32.to_be_bytes());
            bytes
        },
        "str",
    );
    reject(
        &mut out,
        "f64_non_finite",
        {
            let mut bytes = vec![0xcb];
            bytes.extend_from_slice(&f64::NAN.to_bits().to_be_bytes());
            bytes
        },
        "nonfin",
    );

    out
}
