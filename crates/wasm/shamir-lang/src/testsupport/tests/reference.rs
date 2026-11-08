//! Independent iterative reference MessagePack decoder for the M1 codec
//! vectors, written from first principles (no `rmp`) so a shared decoding
//! bug cannot hide. It handles exactly the guest runtime's supported subset
//! — nil, bool, signed/unsigned ints (u64 above `i64::MAX` rejects), f32
//! promoted to f64, f64 (non-finite rejects), str (fix/8/16/32), bin
//! (8/16/32), arrays and maps with string keys (duplicates reject) — and
//! rejects extension families, reserved markers, truncated input, trailing
//! bytes, over-deep nesting and the default-cap violations.
//!
//! Decoded values re-encode through the reference encoder in canonical form
//! (shortest widths, map keys sorted by UTF-8 payload bytes), mirroring the
//! guest encoder contract.
use crate::RuntimeCaps;

use super::super::msgpack::{enc_array, enc_bin, enc_f64, enc_i64, enc_map_sorted, enc_str};

/// Reference value tree a successful decode produces.
#[derive(Clone, Debug, PartialEq)]
pub enum RefValue {
    Nil,
    Bool(bool),
    Int(i64),
    F64(f64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<RefValue>),
    Map(Vec<(String, RefValue)>),
}

/// Canonical re-encoding of a decoded value (shortest widths, sorted keys).
pub fn enc_value(value: &RefValue) -> Vec<u8> {
    match value {
        RefValue::Nil => enc_nil_ref(),
        RefValue::Bool(true) => vec![0xc3],
        RefValue::Bool(false) => vec![0xc2],
        RefValue::Int(int) => enc_i64(*int),
        RefValue::F64(float) => enc_f64(*float),
        RefValue::Str(text) => enc_str(text),
        RefValue::Bin(bytes) => enc_bin(bytes),
        RefValue::Array(items) => enc_array(&items.iter().map(enc_value).collect::<Vec<Vec<u8>>>()),
        RefValue::Map(entries) => {
            let encoded: Vec<(Vec<u8>, Vec<u8>)> = entries
                .iter()
                .map(|(key, value)| (enc_str(key), enc_value(value)))
                .collect();
            enc_map_sorted(&encoded)
        }
    }
}

fn enc_nil_ref() -> Vec<u8> {
    vec![0xc0]
}

/// Cursor over the input bytes; every read checks the bound first.
struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    end: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            cursor: 0,
            end: bytes.len(),
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if self.end - self.cursor < count {
            return Err("trunc".to_string());
        }
        let slice = &self.bytes[self.cursor..self.cursor + count];
        self.cursor += count;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn be(&mut self, width: usize) -> Result<u64, String> {
        let mut raw = [0u8; 8];
        raw[8 - width..].copy_from_slice(self.take(width)?);
        Ok(u64::from_be_bytes(raw))
    }
}

/// Iterative decode of one root value under the default runtime caps.
/// Returns the value and asserts nothing; errors carry the same ASCII
/// class markers the guest codec raises.
pub fn decode_reference(input: &[u8]) -> Result<RefValue, String> {
    let caps = RuntimeCaps::default();
    if input.len() > caps.input_bytes as usize {
        return Err("input".to_string());
    }
    let mut reader = Reader::new(input);
    // the root envelope counts as a container frame, like the guest decoder
    let value = read_value(&mut reader, &caps, 1usize)?;
    if reader.cursor != reader.end {
        return Err("trail".to_string());
    }
    Ok(value)
}

/// String/bytes length header reader shared by the str and bin families.
fn read_len(reader: &mut Reader<'_>, width: usize, from_header: u64) -> Result<usize, String> {
    let raw = if width == 0 {
        from_header
    } else {
        reader.be(width)?
    };
    usize::try_from(raw).map_err(|_| "len".to_string())
}

fn read_string(
    reader: &mut Reader<'_>,
    caps: &RuntimeCaps,
    len: usize,
) -> Result<RefValue, String> {
    if len > caps.string_bytes as usize {
        return Err("str".to_string());
    }
    let raw = reader.take(len)?;
    let text = std::str::from_utf8(raw).map_err(|_| "utf8".to_string())?;
    Ok(RefValue::Str(text.to_string()))
}

fn read_bin(reader: &mut Reader<'_>, len: usize) -> Result<RefValue, String> {
    Ok(RefValue::Bin(reader.take(len)?.to_vec()))
}

/// Reads a string key and enforces the cumulative key-byte cap plus
/// duplicate-free insertion into `seen`.
fn read_map_key(
    reader: &mut Reader<'_>,
    caps: &RuntimeCaps,
    key_bytes: &mut u64,
    seen: &mut Vec<String>,
) -> Result<String, String> {
    let op = reader.byte()?;
    let len = match op {
        0xa0..=0xbf => read_len(reader, 0, u64::from(op & 0x1f))?,
        0xd9 => read_len(reader, 1, 0)?,
        0xda => read_len(reader, 2, 0)?,
        0xdb => read_len(reader, 4, 0)?,
        _ => return Err("key".to_string()),
    };
    *key_bytes += len as u64;
    if *key_bytes > caps.key_bytes {
        return Err("keys".to_string());
    }
    let RefValue::Str(key) = read_string(reader, caps, len)? else {
        return Err("key".to_string());
    };
    if seen.iter().any(|existing| existing == &key) {
        return Err("dup".to_string());
    }
    seen.push(key.clone());
    Ok(key)
}

/// Recursive value reader; `depth` is the nesting level a container read here
/// would occupy (the root value is level 1) and only containers are counted.
fn read_value(
    reader: &mut Reader<'_>,
    caps: &RuntimeCaps,
    depth: usize,
) -> Result<RefValue, String> {
    let op = reader.byte()?;
    match op {
        0xc0 => Ok(RefValue::Nil),
        0xc1 => Err("reserved".to_string()),
        0xc2 => Ok(RefValue::Bool(false)),
        0xc3 => Ok(RefValue::Bool(true)),
        0xc4..=0xc6 => {
            let len = match op {
                0xc4 => read_len(reader, 1, 0)?,
                0xc5 => read_len(reader, 2, 0)?,
                _ => read_len(reader, 4, 0)?,
            };
            read_bin(reader, len)
        }
        0xc7..=0xc9 | 0xd4..=0xd8 => Err("ext".to_string()),
        0xca => {
            let bits = reader.be(4)? as u32;
            float_value(f64::from(f32::from_bits(bits)))
        }
        0xcb => {
            let bits = reader.be(8)?;
            float_value(f64::from_bits(bits))
        }
        0xcc..=0xcf => {
            let raw = match op {
                0xcc => reader.be(1)?,
                0xcd => reader.be(2)?,
                0xce => reader.be(4)?,
                _ => reader.be(8)?,
            };
            int_from_u64(raw)
        }
        0xd0..=0xd3 => {
            let raw = match op {
                0xd0 => reader.be(1)? as i8 as i64,
                0xd1 => reader.be(2)? as i16 as i64,
                0xd2 => reader.be(4)? as i32 as i64,
                _ => reader.be(8)? as i64,
            };
            Ok(RefValue::Int(raw))
        }
        0x80..=0x8f => read_map(reader, caps, depth, usize::from(op & 0x0f)),
        0xde | 0xdf => {
            let count = match op {
                0xde => reader.be(2)?,
                _ => reader.be(4)?,
            };
            let count = usize::try_from(count).map_err(|_| "ent".to_string())?;
            if count > caps.entries as usize {
                return Err("ent".to_string());
            }
            read_map(reader, caps, depth, count)
        }
        0x90..=0x9f => read_array(reader, caps, depth, usize::from(op & 0x0f)),
        0xdc | 0xdd => {
            let count = match op {
                0xdc => reader.be(2)?,
                _ => reader.be(4)?,
            };
            let count = usize::try_from(count).map_err(|_| "ent".to_string())?;
            if count > caps.entries as usize {
                return Err("ent".to_string());
            }
            read_array(reader, caps, depth, count)
        }
        0xa0..=0xbf => {
            let len = usize::from(op & 0x1f);
            read_string(reader, caps, len)
        }
        0xd9..=0xdb => {
            let len = match op {
                0xd9 => read_len(reader, 1, 0)?,
                0xda => read_len(reader, 2, 0)?,
                _ => read_len(reader, 4, 0)?,
            };
            read_string(reader, caps, len)
        }
        _ if op < 0x80 => Ok(RefValue::Int(i64::from(op))),
        _ => Ok(RefValue::Int(i64::from(op as i8))),
    }
}

fn int_from_u64(raw: u64) -> Result<RefValue, String> {
    if raw > i64::MAX as u64 {
        return Err("u64".to_string());
    }
    Ok(RefValue::Int(raw as i64))
}

fn float_value(value: f64) -> Result<RefValue, String> {
    if !value.is_finite() {
        return Err("nonfin".to_string());
    }
    Ok(RefValue::F64(if value == 0.0 { 0.0 } else { value }))
}

fn read_array(
    reader: &mut Reader<'_>,
    caps: &RuntimeCaps,
    depth: usize,
    count: usize,
) -> Result<RefValue, String> {
    if depth > caps.depth as usize {
        return Err("depth".to_string());
    }
    let mut items = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        items.push(read_value(reader, caps, depth + 1)?);
    }
    Ok(RefValue::Array(items))
}

fn read_map(
    reader: &mut Reader<'_>,
    caps: &RuntimeCaps,
    depth: usize,
    pairs: usize,
) -> Result<RefValue, String> {
    if depth > caps.depth as usize {
        return Err("depth".to_string());
    }
    let mut entries = Vec::with_capacity(pairs.min(64));
    let mut key_bytes = 0u64;
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..pairs {
        let key = read_map_key(reader, caps, &mut key_bytes, &mut seen)?;
        let value = read_value(reader, caps, depth + 1)?;
        entries.push((key, value));
    }
    Ok(RefValue::Map(entries))
}
