//! MessagePack codec with on-the-fly key interning
//!
//! This module provides MessagePack encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).
//!
//! §5b floor (#61): this is the storage codec — the home of `InnerValue`
//! as the id-keyed wire/storage form, NOT a hot-path materialization. See
//! `docs/perf/innervalue-floor.md` (Category 1 — type library).

use crate::codecs::interned::common::intern_string_key;
use crate::codecs::CodecError;
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::skip_value;
use crate::types::common::{new_map_wc, TMap};
use crate::types::value::{InnerValue, QueryValue, Value};
use bytes::Bytes;
use rmp::decode::{read_marker, RmpRead};
use rmp::Marker;
#[cfg(test)]
use rmpv::Value as RmpvValue;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use shamir_collections::{TFxMap, THasher};

/// Maximum nesting depth for MessagePack decoding. Deep input beyond this
/// cap returns a `CodecError` instead of overflowing the stack.
const MAX_MSGPACK_DEPTH: usize = 128;

/// Decodes MessagePack bytes to InnerValue, interning string keys.
///
/// Delegates to `msgpack_to_inner_zerocopy` which walks the msgpack bytes
/// directly without building an intermediate `rmpv::Value` tree.
pub fn msgpack_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError> {
    msgpack_to_inner_zerocopy(interner, bytes)
}

/// Legacy path — kept for property testing against the new decoder.
#[cfg(test)]
pub(crate) fn msgpack_to_inner_legacy(
    interner: &Interner,
    bytes: &[u8],
) -> Result<InnerValue, CodecError> {
    let rmpv_value: RmpvValue = rmpv::decode::read_value(&mut &*bytes)
        .map_err(|e| CodecError::Decode(format!("MessagePack decode error: {:?}", e)))?;

    rmpv_value_to_inner(&rmpv_value, interner, 0)
}

/// Decodes MessagePack bytes to InnerValue without an intermediate rmpv::Value tree.
///
/// Uses low-level `rmp::decode` helpers to read markers and lengths directly,
/// allocating only the final InnerValue nodes. This eliminates the double-parse
/// overhead of the `msgpack_to_inner` path (rmpv::Value → InnerValue).
pub fn msgpack_to_inner_zerocopy(
    interner: &Interner,
    bytes: &[u8],
) -> Result<InnerValue, CodecError> {
    let mut cur = std::io::Cursor::new(bytes);
    let val = decode_value(&mut cur, interner, 0)?;
    Ok(val)
}

/// Read a big-endian u16 from cursor.
#[inline]
fn read_u8_from(cur: &mut std::io::Cursor<&[u8]>) -> Result<u8, CodecError> {
    let mut buf = [0u8; 1];
    cur.read_exact_buf(&mut buf)
        .map_err(|e| CodecError::Decode(format!("unexpected EOF: {}", e)))?;
    Ok(buf[0])
}

#[inline]
fn read_u16_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<u16, CodecError> {
    let mut buf = [0u8; 2];
    cur.read_exact_buf(&mut buf)
        .map_err(|e| CodecError::Decode(format!("unexpected EOF: {}", e)))?;
    Ok(u16::from_be_bytes(buf))
}

#[inline]
fn read_u32_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<u32, CodecError> {
    let mut buf = [0u8; 4];
    cur.read_exact_buf(&mut buf)
        .map_err(|e| CodecError::Decode(format!("unexpected EOF: {}", e)))?;
    Ok(u32::from_be_bytes(buf))
}

#[inline]
fn read_i8_from(cur: &mut std::io::Cursor<&[u8]>) -> Result<i8, CodecError> {
    Ok(read_u8_from(cur)? as i8)
}

#[inline]
fn read_i16_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<i16, CodecError> {
    Ok(read_u16_be(cur)? as i16)
}

#[inline]
fn read_i32_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<i32, CodecError> {
    Ok(read_u32_be(cur)? as i32)
}

#[inline]
fn read_i64_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<i64, CodecError> {
    let mut buf = [0u8; 8];
    cur.read_exact_buf(&mut buf)
        .map_err(|e| CodecError::Decode(format!("unexpected EOF: {}", e)))?;
    Ok(i64::from_be_bytes(buf))
}

#[inline]
fn read_u64_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<u64, CodecError> {
    let mut buf = [0u8; 8];
    cur.read_exact_buf(&mut buf)
        .map_err(|e| CodecError::Decode(format!("unexpected EOF: {}", e)))?;
    Ok(u64::from_be_bytes(buf))
}

#[inline]
fn read_f32_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<f32, CodecError> {
    Ok(f32::from_bits(read_u32_be(cur)?))
}

#[inline]
fn read_f64_be(cur: &mut std::io::Cursor<&[u8]>) -> Result<f64, CodecError> {
    Ok(f64::from_bits(read_u64_be(cur)?))
}

/// Read `len` bytes as a UTF-8 string.
#[inline]
fn read_str(cur: &mut std::io::Cursor<&[u8]>, len: usize) -> Result<String, CodecError> {
    let pos = cur.position() as usize;
    let data = cur.get_ref();
    if pos + len > data.len() {
        return Err(CodecError::Decode("unexpected EOF reading string".into()));
    }
    let s = std::str::from_utf8(&data[pos..pos + len])
        .map_err(|_| CodecError::Decode("Invalid UTF-8 in string".to_string()))?;
    cur.set_position((pos + len) as u64);
    Ok(s.to_string())
}

/// Read `len` bytes as binary.
#[inline]
fn read_bin(cur: &mut std::io::Cursor<&[u8]>, len: usize) -> Result<Vec<u8>, CodecError> {
    let pos = cur.position() as usize;
    let data = cur.get_ref();
    if pos + len > data.len() {
        return Err(CodecError::Decode("unexpected EOF reading binary".into()));
    }
    let v = data[pos..pos + len].to_vec();
    cur.set_position((pos + len) as u64);
    Ok(v)
}

fn decode_value(
    cur: &mut std::io::Cursor<&[u8]>,
    interner: &Interner,
    depth: usize,
) -> Result<InnerValue, CodecError> {
    if depth > MAX_MSGPACK_DEPTH {
        return Err(CodecError::Decode(format!(
            "MessagePack nesting depth exceeds {}",
            MAX_MSGPACK_DEPTH
        )));
    }

    let marker = read_marker(cur)
        .map_err(|e| CodecError::Decode(format!("MessagePack decode error: {:?}", e)))?;

    match marker {
        Marker::Null => Ok(InnerValue::Null),
        Marker::True => Ok(InnerValue::Bool(true)),
        Marker::False => Ok(InnerValue::Bool(false)),

        // Positive fixint (0x00..=0x7f)
        Marker::FixPos(v) => Ok(InnerValue::Int(v as i64)),
        // Negative fixint (0xe0..=0xff → -32..=-1)
        Marker::FixNeg(v) => Ok(InnerValue::Int(v as i64)),

        Marker::U8 => Ok(InnerValue::Int(read_u8_from(cur)? as i64)),
        Marker::U16 => Ok(InnerValue::Int(read_u16_be(cur)? as i64)),
        Marker::U32 => Ok(InnerValue::Int(read_u32_be(cur)? as i64)),
        Marker::U64 => {
            let u = read_u64_be(cur)?;
            if u <= i64::MAX as u64 {
                Ok(InnerValue::Int(u as i64))
            } else {
                Ok(InnerValue::Str(u.to_string()))
            }
        }
        Marker::I8 => Ok(InnerValue::Int(read_i8_from(cur)? as i64)),
        Marker::I16 => Ok(InnerValue::Int(read_i16_be(cur)? as i64)),
        Marker::I32 => Ok(InnerValue::Int(read_i32_be(cur)? as i64)),
        Marker::I64 => Ok(InnerValue::Int(read_i64_be(cur)?)),

        Marker::F32 => Ok(InnerValue::F64(read_f32_be(cur)? as f64)),
        Marker::F64 => Ok(InnerValue::F64(read_f64_be(cur)?)),

        // Strings
        Marker::FixStr(len) => {
            let s = read_str(cur, len as usize)?;
            Ok(InnerValue::Str(s))
        }
        Marker::Str8 => {
            let len = read_u8_from(cur)? as usize;
            let s = read_str(cur, len)?;
            Ok(InnerValue::Str(s))
        }
        Marker::Str16 => {
            let len = read_u16_be(cur)? as usize;
            let s = read_str(cur, len)?;
            Ok(InnerValue::Str(s))
        }
        Marker::Str32 => {
            let len = read_u32_be(cur)? as usize;
            let s = read_str(cur, len)?;
            Ok(InnerValue::Str(s))
        }

        // Binary
        Marker::Bin8 => {
            let len = read_u8_from(cur)? as usize;
            Ok(InnerValue::Bin(read_bin(cur, len)?))
        }
        Marker::Bin16 => {
            let len = read_u16_be(cur)? as usize;
            Ok(InnerValue::Bin(read_bin(cur, len)?))
        }
        Marker::Bin32 => {
            let len = read_u32_be(cur)? as usize;
            Ok(InnerValue::Bin(read_bin(cur, len)?))
        }

        // Arrays
        Marker::FixArray(len) => decode_array(cur, interner, len as usize, depth),
        Marker::Array16 => {
            let len = read_u16_be(cur)? as usize;
            decode_array(cur, interner, len, depth)
        }
        Marker::Array32 => {
            let len = read_u32_be(cur)? as usize;
            decode_array(cur, interner, len, depth)
        }

        // Maps
        Marker::FixMap(len) => decode_map(cur, interner, len as usize, depth),
        Marker::Map16 => {
            let len = read_u16_be(cur)? as usize;
            decode_map(cur, interner, len, depth)
        }
        Marker::Map32 => {
            let len = read_u32_be(cur)? as usize;
            decode_map(cur, interner, len, depth)
        }

        // Extension types — store as binary (matching old behaviour)
        Marker::FixExt1 => {
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, 1)?))
        }
        Marker::FixExt2 => {
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, 2)?))
        }
        Marker::FixExt4 => {
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, 4)?))
        }
        Marker::FixExt8 => {
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, 8)?))
        }
        Marker::FixExt16 => {
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, 16)?))
        }
        Marker::Ext8 => {
            let len = read_u8_from(cur)? as usize;
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, len)?))
        }
        Marker::Ext16 => {
            let len = read_u16_be(cur)? as usize;
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, len)?))
        }
        Marker::Ext32 => {
            let len = read_u32_be(cur)? as usize;
            let _type = read_i8_from(cur)?;
            Ok(InnerValue::Bin(read_bin(cur, len)?))
        }

        Marker::Reserved => Err(CodecError::Decode(
            "MessagePack decode error: reserved marker".into(),
        )),
    }
}

fn decode_array(
    cur: &mut std::io::Cursor<&[u8]>,
    interner: &Interner,
    len: usize,
    depth: usize,
) -> Result<InnerValue, CodecError> {
    let mut arr = Vec::with_capacity(len);
    for _ in 0..len {
        arr.push(decode_value(cur, interner, depth + 1)?);
    }
    Ok(InnerValue::List(arr))
}

fn decode_map(
    cur: &mut std::io::Cursor<&[u8]>,
    interner: &Interner,
    len: usize,
    depth: usize,
) -> Result<InnerValue, CodecError> {
    let mut map = new_map_wc(len);
    for _ in 0..len {
        // Read key — must be a string
        let key_marker = read_marker(cur)
            .map_err(|e| CodecError::Decode(format!("MessagePack decode error: {:?}", e)))?;

        let key_str = match key_marker {
            Marker::FixStr(klen) => read_str(cur, klen as usize)?,
            Marker::Str8 => {
                let klen = read_u8_from(cur)? as usize;
                read_str(cur, klen)?
            }
            Marker::Str16 => {
                let klen = read_u16_be(cur)? as usize;
                read_str(cur, klen)?
            }
            Marker::Str32 => {
                let klen = read_u32_be(cur)? as usize;
                read_str(cur, klen)?
            }
            _ => return Err(CodecError::Decode("Map keys must be strings".to_string())),
        };

        let interned_key = intern_string_key(interner, &key_str)?;
        let val = decode_value(cur, interner, depth + 1)?;
        map.insert(interned_key, val);
    }
    Ok(InnerValue::Map(map))
}

/// Encodes InnerValue to MessagePack bytes, de-interning keys.
///
/// Streams directly into the output buffer via `rmp_serde::to_vec`
/// over an `InternedRef` wrapper. No intermediate `rmpv::Value`
/// tree is built — map keys are de-interned to `&str` and written
/// in-place when each Map entry is serialised.
pub fn inner_to_msgpack(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec(&InternedRef { value, interner })
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))
}

/// Borrowed view of an `InnerValue` paired with the `Interner` used
/// to resolve its map keys. Implements `Serialize` so `rmp_serde` (or
/// any other serde format) can write it without an intermediate value
/// tree.
struct InternedRef<'a> {
    value: &'a InnerValue,
    interner: &'a Interner,
}

impl Serialize for InternedRef<'_> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Null => ser.serialize_unit(),
            Value::Bool(b) => ser.serialize_bool(*b),
            Value::Int(i) => ser.serialize_i64(*i),
            Value::F64(f) => ser.serialize_f64(*f),
            Value::Dec(d) => ser.serialize_str(&d.to_string()),
            Value::Big(b) => ser.serialize_str(&b.to_string()),
            Value::Str(s) => ser.serialize_str(s),
            Value::Bin(b) => ser.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = ser.serialize_seq(Some(l.len()))?;
                for el in l {
                    seq.serialize_element(&InternedRef {
                        value: el,
                        interner: self.interner,
                    })?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = ser.serialize_seq(Some(s.len()))?;
                for el in s {
                    seq.serialize_element(&InternedRef {
                        value: el,
                        interner: self.interner,
                    })?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = ser.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    // Borrow the interned key string without allocating
                    // a `UserKey(String)` per entry (hunt #7). The
                    // `with_str` closure scope covers the
                    // `serialize_entry` call so the `&str` stays live.
                    let r: Result<(), S::Error> = self
                        .interner
                        .with_str(k, |key_str| {
                            map.serialize_entry(
                                key_str,
                                &InternedRef {
                                    value: v,
                                    interner: self.interner,
                                },
                            )
                        })
                        .ok_or_else(|| {
                            serde::ser::Error::custom(format!(
                                "Interned key not found in interner: {:?}",
                                k
                            ))
                        })?;
                    r?;
                }
                map.end()
            }
        }
    }
}

/// Converts rmpv::Value to InnerValue, interning all string keys.
/// `depth` tracks nesting level; returns error if `MAX_MSGPACK_DEPTH` is exceeded.
#[cfg(test)]
fn rmpv_value_to_inner(
    rmpv_value: &RmpvValue,
    interner: &Interner,
    depth: usize,
) -> Result<InnerValue, CodecError> {
    if depth > MAX_MSGPACK_DEPTH {
        return Err(CodecError::Decode(format!(
            "MessagePack nesting depth exceeds {}",
            MAX_MSGPACK_DEPTH
        )));
    }
    match rmpv_value {
        RmpvValue::Nil => Ok(InnerValue::Null),
        RmpvValue::Boolean(b) => Ok(InnerValue::Bool(*b)),
        RmpvValue::Integer(n) => {
            if n.is_u64() {
                let u = n.as_u64().unwrap();
                if u <= i64::MAX as u64 {
                    Ok(InnerValue::Int(u as i64))
                } else {
                    Ok(InnerValue::Str(u.to_string()))
                }
            } else if n.is_i64() {
                Ok(InnerValue::Int(n.as_i64().unwrap()))
            } else {
                // Very large integers as string
                Ok(InnerValue::Str(n.to_string()))
            }
        }
        RmpvValue::F64(f) => Ok(InnerValue::F64(*f)),
        RmpvValue::F32(f) => Ok(InnerValue::F64(*f as f64)),
        RmpvValue::String(s) => {
            let s_str = s
                .as_str()
                .ok_or_else(|| CodecError::Decode("Invalid UTF-8 in string".to_string()))?;
            Ok(InnerValue::Str(s_str.to_string()))
        }
        RmpvValue::Binary(b) => Ok(InnerValue::Bin(b.clone())),
        RmpvValue::Array(arr) => {
            let converted: Result<Vec<InnerValue>, CodecError> = arr
                .iter()
                .map(|v| rmpv_value_to_inner(v, interner, depth + 1))
                .collect();
            Ok(InnerValue::List(converted?))
        }
        RmpvValue::Map(map) => {
            let mut converted = new_map_wc(map.len());
            for (key_val, val) in map {
                let key_str = match key_val {
                    RmpvValue::String(s) => s.as_str().ok_or_else(|| {
                        CodecError::Decode("Invalid UTF-8 in map key".to_string())
                    })?,
                    _ => return Err(CodecError::Decode("Map keys must be strings".to_string())),
                };

                let interned_key = intern_string_key(interner, key_str)?;

                let converted_val = rmpv_value_to_inner(val, interner, depth + 1)?;
                converted.insert(interned_key, converted_val);
            }
            Ok(InnerValue::Map(converted))
        }
        RmpvValue::Ext(_type, data) => {
            // Extension types - store as binary for now
            Ok(InnerValue::Bin(data.clone()))
        }
    }
}

// ============================================================================
// W3: byte-level storage-map merge encoder
//
// Produces bytes identical to `merge_inner_maps(old, set_map).to_bytes()` —
// see write_exec.rs:1022 for the reference merge — without decoding the full
// old record to an InnerValue tree. The algorithm patches the top-level map
// in-place at the byte level: old values not in set_map are copied verbatim;
// old values in set_map are replaced with `set_map[k].to_bytes()`; new keys
// from set_map are appended.
// ============================================================================

/// Byte-identical to `merge_inner_maps(InnerValue::from_bytes(old_bytes), set_map).to_bytes()`,
/// computed by patching the old record's top-level storage bytes in place.
///
/// # Contract (MUST hold for every input, or UPDATE is corrupt)
/// ```text
/// merge_storage_bytes(old.to_bytes()?, set_map)
///   == merge_inner_maps_ref(&old, set_map).to_bytes()?
/// ```
/// byte-for-byte.
///
/// # Algorithm (top-level map only — the merge is shallow)
/// 1. Parse the old top-level map header → `n_old` entries + cursor past the header.
/// 2. Walk all `n_old` entries, recording for each entry: the key's decoded u64
///    id, the raw key byte span, and the raw value byte span (using `skip_value`
///    from the lens to find the end of each value without decoding it).
/// 3. Determine which `set_map` keys are NEW (id not among old keys), preserving
///    `set_map` iteration order.
/// 4. `total = n_old + new_count`.
/// 5. Emit: fresh map header (FixMap if total≤15, Map16 if ≤65535, Map32 otherwise
///    — matching rmp_serde's choices exactly); then for each OLD entry in old order:
///    emit the raw key bytes, then if the key's id is in `set_map` emit
///    `set_map[id].to_bytes()` (the patched value), else emit the raw value bytes
///    verbatim; then for each NEW set_map entry in set_map order: emit the key as
///    `InternerKey::serialize` (msgpack bin — same as the storage codec writes keys)
///    plus `value.to_bytes()`.
///
/// # Why this is byte-identical to the tree merge
/// - **Value encoding context-free:** `v.to_bytes()` (rmp_serde of a `Value`) is
///   the exact byte sequence `v` occupies as a map value inside `to_bytes(whole_map)`.
/// - **Verbatim old value == re-encode:** old_bytes were produced by our encoder,
///   and `to_bytes(from_bytes(x)) == x` for all values our encoder emits (minimal
///   int widths, F64, str/bin — no u64>i64::MAX, no F32-from-our-encoder). Copying
///   the old value span is identical to the tree-merge re-encoding it.
/// - **Order:** IndexMap keeps old positions (values updated in place) and appends
///   new keys in insertion order → exactly the old-order-then-new-set-order we emit.
/// - **Header:** rmp_serde picks FixMap/Map16/Map32 by count — we match those exact
///   thresholds (≤15 / ≤65535 / else).
pub fn merge_storage_bytes(
    old_bytes: &[u8],
    set_map: &TMap<InternerKey, InnerValue>,
) -> Result<Bytes, CodecError> {
    // -------------------------------------------------------------------------
    // Step 1: parse the old top-level map header.
    // Keys are stored as msgpack bin (InternerKey::serialize); the lens helpers
    // handle the same markers our encoder emits.
    // -------------------------------------------------------------------------
    let mut pos = 0usize;
    let n_old = read_map_len_merge(old_bytes, &mut pos)?;

    // -------------------------------------------------------------------------
    // Step 2: walk the n_old entries, recording (id, key_start..key_end,
    // val_start..val_end) for each. We need the key id to look it up in
    // set_map, the key bytes to copy verbatim, and the value bytes to copy
    // verbatim when the key is not being patched.
    //
    // Entry format in storage:
    //   key   = bin8/bin16/bin32 header + LE id bytes (InternerKey::serialize)
    //   value = any msgpack value (skip_value handles all markers)
    // -------------------------------------------------------------------------
    struct Entry {
        id: u64,
        key_start: usize, // byte range of the full key (marker + len + payload)
        key_end: usize,
        val_start: usize, // byte range of the full value (marker + payload)
        val_end: usize,
    }

    let mut entries: Vec<Entry> = Vec::with_capacity(n_old);
    // Build a set of old ids for fast lookup: id → index in entries vec.
    // TFxMap<u64, usize> — standard workspace fast-hash map (no untrusted input risk).
    let mut old_ids: TFxMap<u64, usize> =
        TFxMap::with_capacity_and_hasher(n_old, THasher::default());

    for idx in 0..n_old {
        let key_start = pos;
        let id = read_bin_key_id(old_bytes, &mut pos)?;
        let key_end = pos;

        let val_start = pos;
        // Use the lens's skip_value to advance past the full value without decoding.
        skip_value(old_bytes, &mut pos, 0).map_err(|e| {
            CodecError::Decode(format!("merge_storage_bytes: skip_value error: {:?}", e))
        })?;
        let val_end = pos;

        old_ids.insert(id, idx);
        entries.push(Entry {
            id,
            key_start,
            key_end,
            val_start,
            val_end,
        });
    }

    // -------------------------------------------------------------------------
    // Step 3: determine new keys (in set_map iteration order) — keys whose id
    // is not already in the old record.
    // -------------------------------------------------------------------------
    let new_keys: Vec<(&InternerKey, &InnerValue)> = set_map
        .iter()
        .filter(|(k, _)| !old_ids.contains_key(&k.id()))
        .collect();

    let new_count = new_keys.len();

    // -------------------------------------------------------------------------
    // Step 4: total entry count.
    // -------------------------------------------------------------------------
    let total = n_old
        .checked_add(new_count)
        .ok_or_else(|| CodecError::Encode("merge_storage_bytes: entry count overflow".into()))?;

    // -------------------------------------------------------------------------
    // Step 5: emit the merged record.
    //
    // Capacity estimate: header (1-5 bytes) + original bytes + new entries
    // (rmp_serde of each new value; we over-estimate). No allocation in loop
    // for the verbatim-copy path — we write directly into buf.
    // -------------------------------------------------------------------------
    let mut buf: Vec<u8> = Vec::with_capacity(old_bytes.len() + 64 * new_count + 5);

    // Write the map header — match rmp_serde's exact thresholds.
    // rmp_serde uses: FixMap if len≤15, Map16 if len≤65535, Map32 otherwise.
    write_map_header(&mut buf, total)?;

    // --- Old entries (in their original order) ---
    for entry in &entries {
        // Copy the key bytes verbatim (bin marker + len + LE id payload).
        buf.extend_from_slice(&old_bytes[entry.key_start..entry.key_end]);

        // If this key is being overridden by set_map, emit the new value bytes;
        // otherwise copy the old value bytes verbatim (round-trip stable).
        if let Some(new_val) = set_map.get(&InternerKey::new(entry.id)) {
            let new_val_bytes = new_val.to_bytes().map_err(|e| {
                CodecError::Encode(format!("merge_storage_bytes: value encode error: {}", e))
            })?;
            buf.extend_from_slice(&new_val_bytes);
        } else {
            buf.extend_from_slice(&old_bytes[entry.val_start..entry.val_end]);
        }
    }

    // --- New entries (in set_map insertion order, filtered to truly new) ---
    for (key, val) in &new_keys {
        // Write the key as InternerKey::serialize → msgpack bin (variable-width LE).
        // This is the exact same encoding the storage codec uses for keys.
        let key_bytes = rmp_serde::to_vec(key).map_err(|e| {
            CodecError::Encode(format!("merge_storage_bytes: key encode error: {}", e))
        })?;
        buf.extend_from_slice(&key_bytes);

        // Write the value.
        let val_bytes = val.to_bytes().map_err(|e| {
            CodecError::Encode(format!("merge_storage_bytes: value encode error: {}", e))
        })?;
        buf.extend_from_slice(&val_bytes);
    }

    Ok(Bytes::from(buf))
}

/// Read the top-level msgpack map header, advancing `pos` past it.
/// Returns the entry count. Only the three map markers our encoder emits:
/// FixMap (0x80-0x8f), Map16 (0xde), Map32 (0xdf).
#[inline]
fn read_map_len_merge(buf: &[u8], pos: &mut usize) -> Result<usize, CodecError> {
    let p = *pos;
    let m = buf
        .get(p)
        .copied()
        .ok_or_else(|| CodecError::Decode("merge_storage_bytes: empty buffer".into()))?;
    *pos = p + 1;
    match m {
        0x80..=0x8f => Ok((m & 0x0f) as usize),
        0xde => {
            // Map16: 2 big-endian bytes follow
            let end = p + 3;
            if end > buf.len() {
                return Err(CodecError::Decode(
                    "merge_storage_bytes: truncated Map16 header".into(),
                ));
            }
            let n = u16::from_be_bytes([buf[p + 1], buf[p + 2]]) as usize;
            *pos = end;
            Ok(n)
        }
        0xdf => {
            // Map32: 4 big-endian bytes follow
            let end = p + 5;
            if end > buf.len() {
                return Err(CodecError::Decode(
                    "merge_storage_bytes: truncated Map32 header".into(),
                ));
            }
            let n = u32::from_be_bytes([buf[p + 1], buf[p + 2], buf[p + 3], buf[p + 4]]) as usize;
            *pos = end;
            Ok(n)
        }
        other => Err(CodecError::Decode(format!(
            "merge_storage_bytes: expected map marker, got {:#04x}",
            other
        ))),
    }
}

/// Read a bin-encoded InternerKey at `*pos`, returning the decoded u64 id and
/// advancing `pos` past both the bin header and the LE payload.
///
/// Storage keys are always msgpack bin (0xc4/0xc5/0xc6) with 1/2/4/8 payload
/// bytes encoding the id in little-endian — matching `InternerKey::serialize`.
#[inline]
fn read_bin_key_id(buf: &[u8], pos: &mut usize) -> Result<u64, CodecError> {
    let p = *pos;
    let m = buf
        .get(p)
        .copied()
        .ok_or_else(|| CodecError::Decode("merge_storage_bytes: truncated at key marker".into()))?;
    *pos = p + 1;

    let bin_len = match m {
        0xc4 => {
            // bin8: 1-byte length follows
            let lp = *pos;
            let l = buf.get(lp).copied().ok_or_else(|| {
                CodecError::Decode("merge_storage_bytes: truncated bin8 length".into())
            })? as usize;
            *pos = lp + 1;
            l
        }
        0xc5 => {
            // bin16: 2-byte BE length follows
            let lp = *pos;
            let end = lp + 2;
            if end > buf.len() {
                return Err(CodecError::Decode(
                    "merge_storage_bytes: truncated bin16 length".into(),
                ));
            }
            let l = u16::from_be_bytes([buf[lp], buf[lp + 1]]) as usize;
            *pos = end;
            l
        }
        0xc6 => {
            // bin32: 4-byte BE length follows
            let lp = *pos;
            let end = lp + 4;
            if end > buf.len() {
                return Err(CodecError::Decode(
                    "merge_storage_bytes: truncated bin32 length".into(),
                ));
            }
            let l = u32::from_be_bytes([buf[lp], buf[lp + 1], buf[lp + 2], buf[lp + 3]]) as usize;
            *pos = end;
            l
        }
        other => {
            return Err(CodecError::Decode(format!(
                "merge_storage_bytes: expected bin key marker, got {:#04x}",
                other
            )));
        }
    };

    // Read the LE id bytes (1, 2, 4, or 8).
    let payload_start = *pos;
    let payload_end = payload_start.checked_add(bin_len).ok_or_else(|| {
        CodecError::Decode("merge_storage_bytes: key payload length overflow".into())
    })?;
    if payload_end > buf.len() {
        return Err(CodecError::Decode(
            "merge_storage_bytes: truncated key payload".into(),
        ));
    }
    let payload = &buf[payload_start..payload_end];
    *pos = payload_end;

    let id = match bin_len {
        1 => payload[0] as u64,
        2 => u16::from_le_bytes([payload[0], payload[1]]) as u64,
        4 => u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64,
        8 => u64::from_le_bytes([
            payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
            payload[7],
        ]),
        _ => {
            return Err(CodecError::Decode(format!(
                "merge_storage_bytes: invalid bin key length {} (must be 1/2/4/8)",
                bin_len
            )));
        }
    };

    Ok(id)
}

/// Write a msgpack map header for `total` entries into `buf`, matching the
/// thresholds rmp_serde uses: FixMap if total≤15, Map16 if total≤65535,
/// Map32 otherwise.
///
/// `pub(crate)` so that the projection codec (`projection.rs`) can write
/// partial-field map headers using the same thresholds without duplicating
/// this 3-arm match.
#[inline]
pub(crate) fn write_map_header(buf: &mut Vec<u8>, total: usize) -> Result<(), CodecError> {
    if total <= 15 {
        // FixMap: 0x80 | len (single byte)
        buf.push(0x80 | total as u8);
    } else if total <= 65535 {
        // Map16: 0xde + 2 BE bytes
        buf.push(0xde);
        let n = total as u16;
        buf.extend_from_slice(&n.to_be_bytes());
    } else {
        // Map32: 0xdf + 4 BE bytes
        let n = u32::try_from(total).map_err(|_| {
            CodecError::Encode(format!(
                "merge_storage_bytes: map entry count {} exceeds Map32 limit",
                total
            ))
        })?;
        buf.push(0xdf);
        buf.extend_from_slice(&n.to_be_bytes());
    }
    Ok(())
}

// ============================================================================
// W2d: direct QueryValue -> id-keyed storage msgpack encoder
//
// Streaming Serialize that mirrors `Value<InternerKey>::serialize`
// (value.rs:61-98) marker-for-marker for every value variant, but interns each
// map key on the fly from the source `QueryValue` (string-keyed) and writes the
// key as `InternerKey::serialize` (minimal-width LE bin) — IDENTICAL to the
// reference path `query_value_to_inner_with(qv, f).to_bytes()`. The wire bytes
// are the storage format (WAL body verbatim, recovery decode target).
// ============================================================================

/// Encodes a `QueryValue` (string-keyed) directly to id-keyed storage MessagePack
/// bytes, interning each map key via `intern_key`.
///
/// This is the byte-identical fast path for the write cutover (W2d): it produces
/// exactly the same bytes as
/// `query_value_to_inner_with(qv, intern_key).unwrap().to_bytes().unwrap()`,
/// but without materialising the intermediate `InnerValue` tree — map keys are
/// interned and streamed straight into the output buffer.
///
/// The map KEY is serialised through [`InternerKey::serialize`] (minimal-width
/// LE bytes via `serialize_bytes`), matching [`Value::serialize`] for the
/// `Value<InternerKey>::Map` arm (value.rs:89-95). The source map's insertion
/// order is preserved.
pub fn query_value_to_storage_bytes<F>(qv: &QueryValue, intern_key: &F) -> Result<Bytes, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    let wrapper = QvInternedRef {
        value: qv,
        intern: intern_key,
    };
    rmp_serde::to_vec(&wrapper)
        .map(Bytes::from)
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))
}

/// Scratch-buffer variant of [`query_value_to_storage_bytes`].
///
/// Serialises `qv` into the provided `scratch` buffer (which is **cleared**
/// first), then returns an owned `Bytes` via zero-copy `Bytes::from(Vec)`.
/// The Vec is consumed (moved into Bytes without memcpy); `scratch` is left
/// empty and re-grows on the next call. The caller keeps ownership of `scratch`
/// for the loop variable; the key win over a bare `rmp_serde::to_vec` call is
/// that the function signature signals batch intent and the caller can
/// pre-allocate with `Vec::with_capacity`.
///
/// Compared to the previous `Bytes::copy_from_slice` variant, this eliminates
/// the +1 alloc + memcpy per row that was causing the L12 regression on N=1.
pub fn query_value_to_storage_bytes_into<F>(
    qv: &QueryValue,
    intern_key: &F,
    scratch: &mut Vec<u8>,
) -> Result<Bytes, CodecError>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    scratch.clear();
    let wrapper = QvInternedRef {
        value: qv,
        intern: intern_key,
    };
    rmp_serde::encode::write(&mut *scratch, &wrapper)
        .map_err(|e| CodecError::Encode(format!("MessagePack encode error: {}", e)))?;
    // Zero-copy: Bytes::from(Vec) takes ownership of the Vec's heap allocation
    // without memcpy (the Vec becomes the Bytes' backing store).
    // std::mem::take leaves `scratch` as an empty Vec (cap=0); the next
    // iteration's `write()` will re-allocate via Vec's doubling strategy.
    Ok(Bytes::from(std::mem::take(scratch)))
}

/// Borrowed `QueryValue` paired with a key-interning closure. Implements
/// `Serialize` so `rmp_serde` can stream it straight to the output buffer.
///
/// For every variant except `Map`, this mirrors
/// `Value<InternerKey>::serialize` (value.rs:61-98) exactly. For the `Map` arm
/// it iterates the source map in insertion order and, for each `(key_str, val)`,
/// interns `key_str` and serialises the resulting `InternerKey` as the entry key
/// (which dispatches to [`InternerKey::serialize`] → minimal-width LE bin),
/// matching the reference path's map-key encoding.
struct QvInternedRef<'a, F>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    value: &'a QueryValue,
    intern: &'a F,
}

impl<F> Serialize for QvInternedRef<'_, F>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.serialize_inner(ser)
    }
}

impl<F> QvInternedRef<'_, F>
where
    F: Fn(&str) -> Result<InternerKey, CodecError>,
{
    fn serialize_inner<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.value {
            Value::Null => ser.serialize_unit(),
            Value::Bool(b) => ser.serialize_bool(*b),
            Value::Int(i) => ser.serialize_i64(*i),
            Value::F64(f) => ser.serialize_f64(*f),
            Value::Dec(d) => ser.serialize_str(&d.to_string()),
            Value::Big(b) => ser.serialize_str(&b.to_string()),
            Value::Str(s) => ser.serialize_str(s),
            Value::Bin(b) => ser.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = ser.serialize_seq(Some(l.len()))?;
                for el in l {
                    seq.serialize_element(&Self::wrap(el, self.intern))?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = ser.serialize_seq(Some(s.len()))?;
                for el in s {
                    seq.serialize_element(&Self::wrap(el, self.intern))?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = ser.serialize_map(Some(m.len()))?;
                for (key_str, val) in m {
                    let ik = (self.intern)(key_str).map_err(serde::ser::Error::custom)?;
                    let entry_val = Self::wrap(val, self.intern);
                    map.serialize_entry(&ik, &entry_val)?;
                }
                map.end()
            }
        }
    }

    #[inline]
    fn wrap<'b>(value: &'b QueryValue, intern: &'b F) -> QvInternedRef<'b, F> {
        QvInternedRef { value, intern }
    }
}
