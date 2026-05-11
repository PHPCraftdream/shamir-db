//! Order-preserving binary encoding for scalar values.
//!
//! Used by sorted indexes: the physical KV key derived from an indexed
//! value sorts under standard byte comparison the same way the value
//! itself sorts under its native ordering. Once that property holds,
//! the storage layer's B-tree gives us range / order / min-max for
//! free — we don't need a "sorted index" subsystem in the engine, just
//! the right key encoding.
//!
//! Each encoder writes a self-delimiting prefix-byte tag so different
//! value types interleave correctly in a single index (mainly relevant
//! for composite indexes; single-typed columns also benefit because
//! null sorts cleanly to one end).
//!
//! Order across types: `Null < Bool < Int < Float < String`. Inside a
//! type the native order is preserved.
//!
//! Encoding details per type:
//!
//! - `Null` — single tag byte `0x10`.
//! - `Bool` — tag `0x20`, then `0u8` for `false`, `1u8` for `true`.
//! - `i64` — tag `0x30`, then big-endian bytes with the sign bit XORed:
//!   the trick that makes two's-complement integers sort correctly as
//!   unsigned bytes. Negative numbers compare less than zero, etc.
//! - `u64` — tag `0x35`, then big-endian bytes (already orders correctly).
//! - `f64` — tag `0x40`. NaN excluded (refuse to encode); for finite
//!   floats, flip all bits if sign-bit set, otherwise flip just the
//!   sign bit. Encodes the IEEE-754 total order without NaN.
//! - `String` — tag `0x50`, then raw UTF-8 bytes (UTF-8 sorts
//!   lexicographically by code point, which matches Rust's `String`
//!   ord).
//! - `Bytes` — tag `0x60`, then raw bytes.
//!
//! Compose by concatenation: `encode(a) || encode(b) || ...` for a
//! composite key.

#![allow(clippy::module_name_repetitions)]

const TAG_NULL: u8 = 0x10;
const TAG_BOOL: u8 = 0x20;
const TAG_I64: u8 = 0x30;
const TAG_U64: u8 = 0x35;
const TAG_F64: u8 = 0x40;
const TAG_STR: u8 = 0x50;
const TAG_BYTES: u8 = 0x60;

#[derive(Debug, thiserror::Error)]
pub enum SortCodecError {
    #[error("NaN cannot be encoded into an order-preserving key")]
    NaN,
}

/// Encode a null marker.
pub fn encode_null(buf: &mut Vec<u8>) {
    buf.push(TAG_NULL);
}

/// Encode a boolean.
pub fn encode_bool(buf: &mut Vec<u8>, b: bool) {
    buf.push(TAG_BOOL);
    buf.push(if b { 1 } else { 0 });
}

/// Encode a signed integer. Two's-complement sign-flip trick lets the
/// byte-order match the integer order.
pub fn encode_i64(buf: &mut Vec<u8>, v: i64) {
    buf.push(TAG_I64);
    let bits = (v as u64) ^ (1u64 << 63);
    buf.extend_from_slice(&bits.to_be_bytes());
}

/// Encode an unsigned integer.
pub fn encode_u64(buf: &mut Vec<u8>, v: u64) {
    buf.push(TAG_U64);
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Encode a finite f64. Refuses NaN.
pub fn encode_f64(buf: &mut Vec<u8>, v: f64) -> Result<(), SortCodecError> {
    if v.is_nan() {
        return Err(SortCodecError::NaN);
    }
    buf.push(TAG_F64);
    let mut bits = v.to_bits();
    // IEEE-754 sortable transform: if sign bit set (negative), flip
    // every bit; otherwise flip only the sign bit. Result is a u64
    // whose unsigned byte order matches the float's numeric order.
    if (bits >> 63) & 1 == 1 {
        bits = !bits;
    } else {
        bits ^= 1u64 << 63;
    }
    buf.extend_from_slice(&bits.to_be_bytes());
    Ok(())
}

/// Encode a string with a self-delimiting, order-preserving wrapping.
///
/// Raw UTF-8 already sorts lexicographically, but in a sorted-index
/// physical key the encoded value is followed by a record_id suffix:
/// `prefix || encode_str(s) || record_id`. Without a terminator,
/// `"a"||rid_X` and `"aa"||rid_Y` collide on byte ordering whenever
/// rid_X[0] > 'a' = 0x61, because raw UTF-8 has no marker for "the
/// value ended here".
///
/// Encoding shape: escape every literal `0x00` in the UTF-8 bytes as
/// `0x00 0x01`, then append a `0x00 0x00` terminator. The escape
/// sequence is strictly greater than the terminator byte-wise, so the
/// terminator is unique and the original lexicographic order is
/// preserved. Subsequent bytes (e.g. a record_id suffix) compare AFTER
/// the terminator, so two distinct values can never have their order
/// flipped by their suffix.
pub fn encode_str(buf: &mut Vec<u8>, s: &str) {
    buf.push(TAG_STR);
    for &b in s.as_bytes() {
        if b == 0x00 {
            buf.push(0x00);
            buf.push(0x01);
        } else {
            buf.push(b);
        }
    }
    buf.push(0x00);
    buf.push(0x00);
}

/// Encode raw bytes. Same self-delimiting wrap as `encode_str` — see
/// the doc-comment there for the reason and the encoding shape.
pub fn encode_bytes(buf: &mut Vec<u8>, b: &[u8]) {
    buf.push(TAG_BYTES);
    for &byte in b {
        if byte == 0x00 {
            buf.push(0x00);
            buf.push(0x01);
        } else {
            buf.push(byte);
        }
    }
    buf.push(0x00);
    buf.push(0x00);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn enc_i64(v: i64) -> Vec<u8> {
        let mut b = Vec::new();
        encode_i64(&mut b, v);
        b
    }
    fn enc_u64(v: u64) -> Vec<u8> {
        let mut b = Vec::new();
        encode_u64(&mut b, v);
        b
    }
    fn enc_f64(v: f64) -> Vec<u8> {
        let mut b = Vec::new();
        encode_f64(&mut b, v).unwrap();
        b
    }
    fn enc_str(s: &str) -> Vec<u8> {
        let mut b = Vec::new();
        encode_str(&mut b, s);
        b
    }
    fn enc_bool(v: bool) -> Vec<u8> {
        let mut b = Vec::new();
        encode_bool(&mut b, v);
        b
    }

    #[test]
    fn i64_sorts_correctly() {
        // Pick spread including negatives, zero, positives.
        let vals = [i64::MIN, -100, -1, 0, 1, 100, i64::MAX];
        let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_i64(v))).collect();
        encoded.sort_by(|a, b| a.1.cmp(&b.1));
        let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
        assert_eq!(sorted_vals, vals);
    }

    #[test]
    fn u64_sorts_correctly() {
        let vals = [0, 1, 100, u64::MAX / 2, u64::MAX];
        let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_u64(v))).collect();
        encoded.sort_by(|a, b| a.1.cmp(&b.1));
        let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
        assert_eq!(sorted_vals, vals);
    }

    #[test]
    fn f64_sorts_correctly() {
        let vals = [
            f64::NEG_INFINITY,
            -1e100,
            -1.0,
            -0.0,
            0.0,
            1e-100,
            1.0,
            1e100,
            f64::INFINITY,
        ];
        let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_f64(v))).collect();
        encoded.sort_by(|a, b| a.1.cmp(&b.1));
        let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
        for (i, (a, b)) in sorted_vals.iter().zip(vals.iter()).enumerate() {
            // -0.0 == 0.0 in float compare; our codec puts -0.0 BEFORE
            // 0.0 (sign-bit flip). Allow either order for that pair.
            if i < sorted_vals.len() - 1 {
                let next = sorted_vals[i + 1];
                assert!(
                    a <= &next,
                    "out of order at {i}: {a} > {next} (expected {b})"
                );
            }
        }
    }

    #[test]
    fn f64_nan_refuses() {
        let mut buf = Vec::new();
        assert!(encode_f64(&mut buf, f64::NAN).is_err());
    }

    #[test]
    fn str_sorts_lexicographically() {
        let vals = ["", "a", "aa", "ab", "b", "ba", "你好", "🦀"];
        let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_str(v))).collect();
        encoded.sort_by(|a, b| a.1.cmp(&b.1));
        let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
        let mut expected = vals.to_vec();
        expected.sort();
        assert_eq!(sorted_vals, expected);
    }

    /// Regression: the sorted-index physical key is
    /// `prefix || encode_str(s) || record_id_suffix`. If `encode_str`
    /// is not self-delimiting, two values where one is a prefix of
    /// the other can have their order inverted depending on the
    /// suffix bytes — i.e. `"a" || [0xFF; 16]` sorts AFTER
    /// `"aa" || [0x00; 16]` even though `"a" < "aa"` semantically.
    ///
    /// Range bounds break the same way: an inclusive upper of
    /// `enc("a") || [0xFF; 16]` would match `enc("aa") || ...` keys.
    #[test]
    fn str_with_suffix_preserves_order() {
        // Encoded values for two strings where one is a prefix of
        // the other. Append a worst-case suffix:
        //   "a"  + 16 bytes of 0xFF (maximum possible suffix)
        //   "aa" + 16 bytes of 0x00 (minimum possible suffix)
        // Lexicographically "a" < "aa", so the FIRST composite key
        // must sort BEFORE the second regardless of the suffix.
        let mut a_key = enc_str("a");
        a_key.extend_from_slice(&[0xFFu8; 16]);
        let mut aa_key = enc_str("aa");
        aa_key.extend_from_slice(&[0x00u8; 16]);
        assert!(
            a_key < aa_key,
            "enc(\"a\")||0xFF*16 must sort before enc(\"aa\")||0x00*16, \
             else sorted-index range queries on strings return wrong results"
        );

        // Same shape but with an extra distractor in the middle.
        let mut a_pad = enc_str("a");
        a_pad.extend_from_slice(b"zzzzzzzzzzzzzzzz"); // 16 'z' bytes
        let mut ab_key = enc_str("ab");
        ab_key.extend_from_slice(b"aaaaaaaaaaaaaaaa");
        assert!(
            a_pad < ab_key,
            "enc(\"a\")||'z'*16 must sort before enc(\"ab\")||'a'*16"
        );

        // Bytes encoding has the same flaw — cover it too.
        let mut buf_a = Vec::new();
        encode_bytes(&mut buf_a, b"\x01");
        buf_a.extend_from_slice(&[0xFFu8; 16]);
        let mut buf_aa = Vec::new();
        encode_bytes(&mut buf_aa, b"\x01\x01");
        buf_aa.extend_from_slice(&[0x00u8; 16]);
        assert!(
            buf_a < buf_aa,
            "encode_bytes must be self-delimiting for sorted-index keys"
        );
    }

    #[test]
    fn bool_sorts_correctly() {
        assert!(enc_bool(false) < enc_bool(true));
    }

    #[test]
    fn tags_order_types() {
        // Across types: Null < Bool < Int < Float < String.
        let null_buf = {
            let mut b = Vec::new();
            encode_null(&mut b);
            b
        };
        assert!(null_buf < enc_bool(false));
        assert!(enc_bool(true) < enc_i64(i64::MIN));
        assert!(enc_i64(i64::MAX) < enc_f64(f64::NEG_INFINITY));
        assert!(enc_f64(f64::INFINITY) < enc_str(""));
    }

    #[test]
    fn composite_via_concatenation() {
        // (Int, String) — sort by (a, b).
        let mut k1 = Vec::new();
        encode_i64(&mut k1, 5);
        encode_str(&mut k1, "zzz");
        let mut k2 = Vec::new();
        encode_i64(&mut k2, 7);
        encode_str(&mut k2, "aaa");
        // 5 < 7 regardless of second column
        assert!(k1 < k2);

        let mut k3 = Vec::new();
        encode_i64(&mut k3, 5);
        encode_str(&mut k3, "aaa");
        // (5, "aaa") < (5, "zzz")
        assert!(k3 < k1);
    }
}
