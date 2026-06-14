//! MessagePack codec with on-the-fly key interning
//!
//! This module provides MessagePack encoding/decoding directly to/from InnerValue
//! without using UserValue (which is deprecated and for tests only).

use crate::codecs::interned::common::intern_string_key;
use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::new_map_wc;
use crate::types::value::{InnerValue, Value};
use rmp::decode::{read_marker, RmpRead};
use rmp::Marker;
#[cfg(test)]
use rmpv::Value as RmpvValue;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

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
