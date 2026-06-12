//! Bytes-level pre-filter for msgpack records.
//!
//! `FilterNode::matches_msgpack_bytes` walks raw msgpack without decoding the
//! full record into `InnerValue`.  It is a **fast-skip pre-filter** — it
//! never produces a false-negative or a false-positive:
//!
//! - `Some(false)` — filter definitely rejects the row; caller skips it (no
//!   full decode needed → the win).
//! - `Some(true)` — filter definitely accepts; caller still does the full
//!   decode + normal filter as a safety net (cheap on the happy path).
//! - `None` — filter shape is too complex for bytes-eval; caller falls through
//!   to the full decode + normal filter unchanged.
//!
//! Only simple atoms that can be evaluated by inspecting raw msgpack scalars
//! are handled.  Complex atoms (`Like`, `Regex`, `Contains`, `ContainsAny`,
//! `ContainsAll`, `Between`, `FtsMatch`, `ComputedCompare`, dynamic `In`)
//! return `None` immediately so the safe full-decode path runs instead.
//!
//! Restriction: only top-level and nested map fields (any depth) are resolved.
//! The raw-msgpack path supports the same depth as `resolve_field_ref` — it
//! iterates through nested maps one level at a time.
//!
//! # Zero-alloc cursor
//!
//! `matches_msgpack_bytes` no longer calls `rmpv::decode::read_value`
//! (which allocates an `Rv` tree for the whole record).  Instead,
//! `eval_node_raw` seeks each filter atom's target field directly inside
//! the raw bytes:
//!
//! - `seek_msgpack_field` walks the map header and key/value pairs at one
//!   nesting level, comparing binary keys without any allocation.
//! - `skip_msgpack_value` skips an arbitrary msgpack value (recursing for
//!   maps/arrays) so the cursor can advance past unmatched entries.
//! - `decode_scalar_at` reads exactly the scalar at the matched position
//!   and returns a `RawScalar` enum for comparison — zero heap.
//!
//! Only rows that pass the pre-filter proceed to the full rmpv decode, so
//! the allocating path is paid only on matched (hit) rows.

use std::cmp::Ordering;

use super::filter_node::{CompareOp, FilterNode};
use crate::query::filter::FilterValue;

// ── key encoding (unchanged) ──────────────────────────────────────────────────

/// A minimal `AsRef<[u8]>` wrapper for the variable-length interned key bytes.
struct KeyBuf([u8; 8], usize);
impl AsRef<[u8]> for KeyBuf {
    fn as_ref(&self) -> &[u8] {
        &self.0[..self.1]
    }
}

/// Re-encode an interned `u64` key the same way `InternerKey::serialize`
/// does: variable-width little-endian bytes, 1/2/4/8 bytes.
fn interned_key_bytes(id: u64) -> KeyBuf {
    let mut buf = [0u8; 8];
    let len = if id <= u8::MAX as u64 {
        buf[0] = id as u8;
        1
    } else if id <= u16::MAX as u64 {
        let b = (id as u16).to_le_bytes();
        buf[..2].copy_from_slice(&b);
        2
    } else if id <= u32::MAX as u64 {
        let b = (id as u32).to_le_bytes();
        buf[..4].copy_from_slice(&b);
        4
    } else {
        buf.copy_from_slice(&id.to_le_bytes());
        8
    };
    KeyBuf(buf, len)
}

// ── zero-alloc msgpack cursor ─────────────────────────────────────────────────

/// Read a big-endian u16 from `bytes[pos..]`.
#[inline]
fn read_u16_be(bytes: &[u8], pos: usize) -> Option<u16> {
    let b = bytes.get(pos..pos + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Read a big-endian u32 from `bytes[pos..]`.
#[inline]
fn read_u32_be(bytes: &[u8], pos: usize) -> Option<u32> {
    let b = bytes.get(pos..pos + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Read a big-endian u64 from `bytes[pos..]`.
#[inline]
fn read_u64_be(bytes: &[u8], pos: usize) -> Option<u64> {
    let b = bytes.get(pos..pos + 8)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// Skip a single msgpack value starting at `bytes[pos]`.
///
/// Returns the position immediately after the skipped value, or `None` on
/// malformed/truncated bytes or on `Ext` (which we never emit).
fn skip_msgpack_value(bytes: &[u8], pos: usize) -> Option<usize> {
    let b = *bytes.get(pos)?;
    let pos = pos + 1; // consume type byte

    match b {
        // nil / bool
        0xc0 | 0xc2 | 0xc3 => Some(pos),

        // positive fixint (0x00..=0x7f) — value encoded in the byte itself
        0x00..=0x7f => Some(pos),

        // negative fixint (0xe0..=0xff)
        0xe0..=0xff => Some(pos),

        // uint8 / int8
        0xcc | 0xd0 => Some(pos + 1),
        // uint16 / int16
        0xcd | 0xd1 => Some(pos + 2),
        // uint32 / int32 / float32
        0xce | 0xd2 | 0xca => Some(pos + 4),
        // uint64 / int64 / float64
        0xcf | 0xd3 | 0xcb => Some(pos + 8),

        // fixstr (0xa0..=0xbf) — lower 5 bits = length
        0xa0..=0xbf => {
            let len = (b & 0x1f) as usize;
            Some(pos + len)
        }
        // str8
        0xd9 => {
            let len = *bytes.get(pos)? as usize;
            Some(pos + 1 + len)
        }
        // str16
        0xda => {
            let len = read_u16_be(bytes, pos)? as usize;
            Some(pos + 2 + len)
        }
        // str32
        0xdb => {
            let len = read_u32_be(bytes, pos)? as usize;
            Some(pos + 4 + len)
        }

        // bin8
        0xc4 => {
            let len = *bytes.get(pos)? as usize;
            Some(pos + 1 + len)
        }
        // bin16
        0xc5 => {
            let len = read_u16_be(bytes, pos)? as usize;
            Some(pos + 2 + len)
        }
        // bin32
        0xc6 => {
            let len = read_u32_be(bytes, pos)? as usize;
            Some(pos + 4 + len)
        }

        // fixarray (0x90..=0x9f) — lower 4 bits = count
        0x90..=0x9f => {
            let count = (b & 0x0f) as usize;
            skip_n_values(bytes, pos, count)
        }
        // array16
        0xdc => {
            let count = read_u16_be(bytes, pos)? as usize;
            skip_n_values(bytes, pos + 2, count)
        }
        // array32
        0xdd => {
            let count = read_u32_be(bytes, pos)? as usize;
            skip_n_values(bytes, pos + 4, count)
        }

        // fixmap (0x80..=0x8f) — lower 4 bits = entry count
        0x80..=0x8f => {
            let count = (b & 0x0f) as usize;
            // each entry = key + value
            skip_n_values(bytes, pos, count * 2)
        }
        // map16
        0xde => {
            let count = read_u16_be(bytes, pos)? as usize;
            skip_n_values(bytes, pos + 2, count * 2)
        }
        // map32
        0xdf => {
            let count = read_u32_be(bytes, pos)? as usize;
            skip_n_values(bytes, pos + 4, count * 2)
        }

        // ext — we never emit ext; return None (safe fall-through)
        0xc7 | 0xc8 | 0xc9 | 0xd4..=0xd8 => None,

        // 0xc1 is unused in the msgpack spec
        _ => None,
    }
}

/// Skip `n` consecutive msgpack values starting at `bytes[pos]`.
fn skip_n_values(bytes: &[u8], mut pos: usize, n: usize) -> Option<usize> {
    for _ in 0..n {
        pos = skip_msgpack_value(bytes, pos)?;
    }
    Some(pos)
}

/// Parse the map header at `bytes[pos]`.
///
/// Returns `(entry_count, position_of_first_key)` or `None` if `bytes[pos]`
/// is not a map header.
fn read_map_header(bytes: &[u8], pos: usize) -> Option<(usize, usize)> {
    let b = *bytes.get(pos)?;
    match b {
        0x80..=0x8f => Some(((b & 0x0f) as usize, pos + 1)),
        0xde => {
            let count = read_u16_be(bytes, pos + 1)? as usize;
            Some((count, pos + 3))
        }
        0xdf => {
            let count = read_u32_be(bytes, pos + 1)? as usize;
            Some((count, pos + 5))
        }
        _ => None,
    }
}

/// Read a binary (bin) key at `bytes[pos]`.
///
/// On-disk format: interned keys are serialised as msgpack `bin` via
/// `serialize_bytes`.  Returns `(key_bytes_slice, pos_after_key)`.
fn read_bin_key(bytes: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    let b = *bytes.get(pos)?;
    let pos = pos + 1;
    match b {
        0xc4 => {
            let len = *bytes.get(pos)? as usize;
            let payload = bytes.get(pos + 1..pos + 1 + len)?;
            Some((payload, pos + 1 + len))
        }
        0xc5 => {
            let len = read_u16_be(bytes, pos)? as usize;
            let payload = bytes.get(pos + 2..pos + 2 + len)?;
            Some((payload, pos + 2 + len))
        }
        0xc6 => {
            let len = read_u32_be(bytes, pos)? as usize;
            let payload = bytes.get(pos + 4..pos + 4 + len)?;
            Some((payload, pos + 4 + len))
        }
        _ => None, // key is not bin — unexpected format → fall through
    }
}

/// Seek a single map level for a key matching `target_key_bytes`.
///
/// Starts scanning the map header at `bytes[map_pos]`.  Returns
/// `Some(value_pos)` where `value_pos` is the offset of the matching entry's
/// value, or `None` if the key is not present or bytes are malformed.
fn seek_map_key(bytes: &[u8], map_pos: usize, target_key_bytes: &[u8]) -> Option<usize> {
    let (entry_count, mut pos) = read_map_header(bytes, map_pos)?;
    for _ in 0..entry_count {
        let (key_bytes, value_pos) = read_bin_key(bytes, pos)?;
        if key_bytes == target_key_bytes {
            return Some(value_pos);
        }
        // Skip the value to advance to the next key.
        pos = skip_msgpack_value(bytes, value_pos)?;
    }
    None
}

/// Navigate a multi-segment field path through nested msgpack maps.
///
/// Each segment in `path` names one interned `u64` key.  Returns the byte
/// offset of the innermost value, or `None` if any segment is absent or the
/// bytes are malformed.
fn find_field_pos(bytes: &[u8], path: &[u64]) -> Option<usize> {
    // Start at offset 0 (the root map).
    let mut cur_map_pos = 0usize;
    let mut segments = path.iter().peekable();
    while let Some(&id) = segments.next() {
        let key_buf = interned_key_bytes(id);
        let value_pos = seek_map_key(bytes, cur_map_pos, key_buf.as_ref())?;
        if segments.peek().is_some() {
            // More segments → the value must itself be a map; descend.
            cur_map_pos = value_pos;
        } else {
            return Some(value_pos);
        }
    }
    None // empty path
}

// ── raw scalar decoder ────────────────────────────────────────────────────────

/// A zero-alloc representation of a decoded msgpack scalar.
///
/// Only the types that appear in `FilterValue` literals are represented.
/// Everything else maps to `Other` which causes a `None` (fall-through)
/// result from the comparison helper.
enum RawScalar<'a> {
    Nil,
    Bool(bool),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(&'a [u8]), // UTF-8 bytes, compared as bytes
    Bin(&'a [u8]),
    /// Map, array, ext, or unsupported type byte — caller returns None.
    Other,
}

/// Decode the scalar at `bytes[pos]`.  Returns `(scalar, pos_after_value)`.
fn decode_scalar_at(bytes: &[u8], pos: usize) -> Option<(RawScalar<'_>, usize)> {
    let b = *bytes.get(pos)?;
    let after = pos + 1; // position after the type byte

    let s = match b {
        0xc0 => (RawScalar::Nil, after),
        0xc2 => (RawScalar::Bool(false), after),
        0xc3 => (RawScalar::Bool(true), after),

        // positive fixint
        0x00..=0x7f => (RawScalar::I64(i64::from(b)), after),
        // negative fixint
        0xe0..=0xff => (RawScalar::I64(i64::from(b as i8)), after),

        // uint8
        0xcc => {
            let v = *bytes.get(after)? as u64;
            (RawScalar::U64(v), after + 1)
        }
        // uint16
        0xcd => {
            let v = read_u16_be(bytes, after)? as u64;
            (RawScalar::U64(v), after + 2)
        }
        // uint32
        0xce => {
            let v = read_u32_be(bytes, after)? as u64;
            (RawScalar::U64(v), after + 4)
        }
        // uint64
        0xcf => {
            let v = read_u64_be(bytes, after)?;
            (RawScalar::U64(v), after + 8)
        }

        // int8
        0xd0 => {
            let v = *bytes.get(after)? as i8;
            (RawScalar::I64(i64::from(v)), after + 1)
        }
        // int16
        0xd1 => {
            let v = read_u16_be(bytes, after)? as i16;
            (RawScalar::I64(i64::from(v)), after + 2)
        }
        // int32
        0xd2 => {
            let v = read_u32_be(bytes, after)? as i32;
            (RawScalar::I64(i64::from(v)), after + 4)
        }
        // int64
        0xd3 => {
            let v = read_u64_be(bytes, after)? as i64;
            (RawScalar::I64(v), after + 8)
        }

        // float32
        0xca => {
            let raw = read_u32_be(bytes, after)?;
            (RawScalar::F32(f32::from_bits(raw)), after + 4)
        }
        // float64
        0xcb => {
            let raw = read_u64_be(bytes, after)?;
            (RawScalar::F64(f64::from_bits(raw)), after + 8)
        }

        // fixstr
        0xa0..=0xbf => {
            let len = (b & 0x1f) as usize;
            let payload = bytes.get(after..after + len)?;
            (RawScalar::Str(payload), after + len)
        }
        // str8
        0xd9 => {
            let len = *bytes.get(after)? as usize;
            let payload = bytes.get(after + 1..after + 1 + len)?;
            (RawScalar::Str(payload), after + 1 + len)
        }
        // str16
        0xda => {
            let len = read_u16_be(bytes, after)? as usize;
            let payload = bytes.get(after + 2..after + 2 + len)?;
            (RawScalar::Str(payload), after + 2 + len)
        }
        // str32
        0xdb => {
            let len = read_u32_be(bytes, after)? as usize;
            let payload = bytes.get(after + 4..after + 4 + len)?;
            (RawScalar::Str(payload), after + 4 + len)
        }

        // bin8
        0xc4 => {
            let len = *bytes.get(after)? as usize;
            let payload = bytes.get(after + 1..after + 1 + len)?;
            (RawScalar::Bin(payload), after + 1 + len)
        }
        // bin16
        0xc5 => {
            let len = read_u16_be(bytes, after)? as usize;
            let payload = bytes.get(after + 2..after + 2 + len)?;
            (RawScalar::Bin(payload), after + 2 + len)
        }
        // bin32
        0xc6 => {
            let len = read_u32_be(bytes, after)? as usize;
            let payload = bytes.get(after + 4..after + 4 + len)?;
            (RawScalar::Bin(payload), after + 4 + len)
        }

        // map / array / ext — composite, return Other
        _ => (RawScalar::Other, after),
    };
    Some(s)
}

// ── scalar comparison ─────────────────────────────────────────────────────────

/// Compare a `RawScalar` decoded from msgpack bytes against a `FilterValue`
/// literal.  Returns `None` for type mismatches or unsupported shapes.
fn compare_raw_to_filter(raw: &RawScalar<'_>, fv: &FilterValue) -> Option<Ordering> {
    match (raw, fv) {
        (RawScalar::Nil, FilterValue::Null) => Some(Ordering::Equal),
        // null vs non-null: not clearly ordered → fall back
        (_, FilterValue::Null) => None,

        (RawScalar::Bool(a), FilterValue::Bool(b)) => a.partial_cmp(b),

        // Int (positive) vs Int
        (RawScalar::U64(a), FilterValue::Int(b)) => {
            if *b < 0 {
                Some(Ordering::Greater) // a is non-negative, b is negative
            } else {
                a.partial_cmp(&(*b as u64))
            }
        }
        // Int (signed) vs Int
        (RawScalar::I64(a), FilterValue::Int(b)) => a.partial_cmp(b),

        // Float vs Float
        (RawScalar::F64(a), FilterValue::Float(b)) => a.partial_cmp(b),
        (RawScalar::F32(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
        // Int (signed) vs Float (widening)
        (RawScalar::I64(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
        // Int (unsigned) vs Float (widening)
        (RawScalar::U64(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
        // Float vs Int (widening)
        (RawScalar::F64(a), FilterValue::Int(b)) => a.partial_cmp(&(*b as f64)),
        (RawScalar::F32(a), FilterValue::Int(b)) => (*a as f64).partial_cmp(&(*b as f64)),

        // Str vs Str — compare as bytes (UTF-8 is byte-ordered correctly for Ord)
        (RawScalar::Str(a), FilterValue::String(b)) => Some((*a).cmp(b.as_bytes())),

        // Bin vs Bin
        (RawScalar::Bin(a), FilterValue::Binary(b)) => Some((*a).cmp(b.as_slice())),

        // Mismatch / unsupported → fall back to full decode
        _ => None,
    }
}

/// Apply a `CompareOp` to an `Ordering`.
#[inline]
fn apply_op(ord: Ordering, op: CompareOp) -> bool {
    match op {
        CompareOp::Eq => ord == Ordering::Equal,
        CompareOp::Ne => ord != Ordering::Equal,
        CompareOp::Gt => ord == Ordering::Greater,
        CompareOp::Gte => matches!(ord, Ordering::Greater | Ordering::Equal),
        CompareOp::Lt => ord == Ordering::Less,
        CompareOp::Lte => matches!(ord, Ordering::Less | Ordering::Equal),
    }
}

// ── raw-bytes filter evaluation ───────────────────────────────────────────────

/// Evaluate a `FilterNode` directly against raw msgpack `bytes` without ever
/// allocating an `Rv` tree.
///
/// Returns the same tri-state as `matches_msgpack_bytes`:
/// `Some(false)` = definite reject, `Some(true)` = definite accept,
/// `None` = fall through to full decode.
fn eval_node_raw(node: &FilterNode, bytes: &[u8]) -> Option<bool> {
    match node {
        FilterNode::True => Some(true),
        FilterNode::False => Some(false),

        // ── Compare ──────────────────────────────────────────────────────────
        FilterNode::Compare {
            field_path,
            value,
            pre_resolved,
            op,
        } => {
            let fv_lit: FilterValue = if let Some(pre) = pre_resolved {
                inner_value_to_filter_value_lit(pre)?
            } else {
                match value {
                    FilterValue::Null
                    | FilterValue::Bool(_)
                    | FilterValue::Int(_)
                    | FilterValue::Float(_)
                    | FilterValue::String(_)
                    | FilterValue::Binary(_) => value.clone(),
                    _ => return None, // dynamic FieldRef / Param / etc.
                }
            };

            match find_field_pos(bytes, field_path) {
                None => {
                    // Field absent: treat as null.
                    // Null vs non-null comparisons are semantically defined in
                    // the normal filter path.  Here we conservatively fall back
                    // so the full-decode path handles the semantics correctly.
                    None
                }
                Some(val_pos) => {
                    let (scalar, _) = decode_scalar_at(bytes, val_pos)?;
                    if matches!(scalar, RawScalar::Other) {
                        return None; // composite value → full decode
                    }
                    let ord = compare_raw_to_filter(&scalar, &fv_lit)?;
                    Some(apply_op(ord, *op))
                }
            }
        }

        // ── Logical ──────────────────────────────────────────────────────────
        FilterNode::And(children) => {
            for child in children {
                match eval_node_raw(child, bytes) {
                    Some(false) => return Some(false),
                    None => return None,
                    Some(true) => {}
                }
            }
            Some(true)
        }
        FilterNode::Or(children) => {
            for child in children {
                match eval_node_raw(child, bytes) {
                    Some(true) => return Some(true),
                    None => return None,
                    Some(false) => {}
                }
            }
            Some(false)
        }
        FilterNode::Not(inner) => eval_node_raw(inner, bytes).map(|b| !b),

        // ── Existence checks ─────────────────────────────────────────────────
        FilterNode::Exists { field_path } => Some(find_field_pos(bytes, field_path).is_some()),
        FilterNode::NotExists { field_path } => Some(find_field_pos(bytes, field_path).is_none()),
        FilterNode::IsNull { field_path } => {
            match find_field_pos(bytes, field_path) {
                None => Some(true), // absent == null
                Some(val_pos) => {
                    let b = bytes.get(val_pos)?;
                    Some(*b == 0xc0) // 0xc0 = nil
                }
            }
        }
        FilterNode::IsNotNull { field_path } => {
            match find_field_pos(bytes, field_path) {
                None => Some(false), // absent == null, so not-null is false
                Some(val_pos) => {
                    let b = bytes.get(val_pos)?;
                    Some(*b != 0xc0)
                }
            }
        }

        // ── Unsupported atoms — fall back to full decode ──────────────────────
        FilterNode::Like { .. }
        | FilterNode::Regex { .. }
        | FilterNode::Contains { .. }
        | FilterNode::ContainsAny { .. }
        | FilterNode::ContainsAll { .. }
        | FilterNode::Between { .. }
        | FilterNode::FtsMatch { .. }
        | FilterNode::ComputedCompare { .. }
        | FilterNode::In { .. } => None,
    }
}

// ── public entry point ────────────────────────────────────────────────────────

impl FilterNode {
    /// Try to evaluate this filter against raw msgpack bytes without decoding
    /// to `InnerValue`.
    ///
    /// # Returns
    /// - `Some(false)` — row is definitely rejected; skip full decode.
    /// - `Some(true)` — row passes; proceed to full decode + normal filter.
    /// - `None` — filter shape is unsupported here; fall back to full decode.
    ///
    /// **Precondition**: `bytes` must be a valid msgpack encoding of an
    /// `InnerValue::Map` with `u64` (interned) keys serialised as `bin`.
    /// Bytes produced by any other codec return `None` (safe fall-through).
    pub fn matches_msgpack_bytes(&self, bytes: &[u8]) -> Option<bool> {
        // Zero-alloc raw-cursor path: no rmpv tree is allocated.
        eval_node_raw(self, bytes)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Convert a pre-resolved `InnerValue` literal into a `FilterValue` literal
/// for the scalar comparison helper.  Non-scalar variants return `None`.
fn inner_value_to_filter_value_lit(
    v: &shamir_types::types::value::InnerValue,
) -> Option<FilterValue> {
    use shamir_types::types::value::InnerValue as Iv;
    match v {
        Iv::Null => Some(FilterValue::Null),
        Iv::Bool(b) => Some(FilterValue::Bool(*b)),
        Iv::Int(i) => Some(FilterValue::Int(*i)),
        Iv::F64(f) => Some(FilterValue::Float(*f)),
        Iv::Str(s) => Some(FilterValue::String(s.clone())),
        Iv::Bin(b) => Some(FilterValue::Binary(b.clone())),
        // Dec, Big, List, Set, Map — fall back to full decode
        _ => None,
    }
}
