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
