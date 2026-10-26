use super::cursor::{borrow_bytes, borrow_str, read_be, read_u8};
use super::skip::skip_value;
use super::view::RecordView;
use super::{RecordViewError, MAX_MSGPACK_DEPTH};
use crate::record_view::record_value::{RawSeq, RecordValue};
use std::borrow::Cow;

/// Read ONE MessagePack value as a borrowed [`RecordValue`]. Decodes only the
/// matched value's marker — the lens's analogue of the canonical decoder's
/// `decode_value`, but zero-copy for scalars and lazy for aggregates.
///
/// `depth` bounds recursion (see [`skip_value`]).
pub(crate) fn read_value<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    depth: usize,
) -> Result<RecordValue<'a>, RecordViewError> {
    if depth > MAX_MSGPACK_DEPTH {
        return Err(RecordViewError::DepthExceeded(RecordViewError::depth_cap()));
    }
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        0xc0 => Ok(RecordValue::Null),
        0xc2 => Ok(RecordValue::Bool(false)),
        0xc3 => Ok(RecordValue::Bool(true)),

        // positive fixint / negative fixint.
        0x00..=0x7f => Ok(RecordValue::Int(m as i64)),
        0xe0..=0xff => Ok(RecordValue::Int((m as i8) as i64)),

        // unsigned ints — values beyond i64 are represented by the nonallocating Big variant.
        0xcc => Ok(RecordValue::Int(read_be::<1>(buf, pos)? as i64)),
        0xcd => Ok(RecordValue::Int(read_be::<2>(buf, pos)? as i64)),
        0xce => Ok(RecordValue::Int(read_be::<4>(buf, pos)? as i64)),
        0xcf => {
            let u = read_be::<8>(buf, pos)?;
            Ok(uint_to_record_value(u))
        }
        // signed ints.
        0xd0 => Ok(RecordValue::Int(read_be::<1>(buf, pos)? as u8 as i8 as i64)),
        0xd1 => Ok(RecordValue::Int(
            read_be::<2>(buf, pos)? as u16 as i16 as i64
        )),
        0xd2 => Ok(RecordValue::Int(
            read_be::<4>(buf, pos)? as u32 as i32 as i64
        )),
        0xd3 => Ok(RecordValue::Int(read_be::<8>(buf, pos)? as i64)),

        // floats — F32 widened to f64 (matches the tree decoder).
        0xca => Ok(RecordValue::F64(
            f32::from_bits(read_be::<4>(buf, pos)? as u32) as f64,
        )),
        0xcb => Ok(RecordValue::F64(f64::from_bits(read_be::<8>(buf, pos)?))),

        // strings — borrow the payload as &str (zero-copy).
        0xa0..=0xbf => {
            let len = (m & 0x1f) as usize;
            let s = borrow_str(buf, pos, len)?;
            Ok(RecordValue::Str(Cow::Borrowed(s)))
        }
        0xd9 => {
            let len = read_be::<1>(buf, pos)? as usize;
            let s = borrow_str(buf, pos, len)?;
            Ok(RecordValue::Str(Cow::Borrowed(s)))
        }
        0xda => {
            let len = read_be::<2>(buf, pos)? as usize;
            let s = borrow_str(buf, pos, len)?;
            Ok(RecordValue::Str(Cow::Borrowed(s)))
        }
        0xdb => {
            let len = read_be::<4>(buf, pos)? as usize;
            let s = borrow_str(buf, pos, len)?;
            Ok(RecordValue::Str(Cow::Borrowed(s)))
        }

        // binary — borrow the payload.
        0xc4 => {
            let len = read_be::<1>(buf, pos)? as usize;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, len)?))
        }
        0xc5 => {
            let len = read_be::<2>(buf, pos)? as usize;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, len)?))
        }
        0xc6 => {
            let len = read_be::<4>(buf, pos)? as usize;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, len)?))
        }

        // arrays — lazy RawSeq cursor over the element bytes.
        0x90..=0x9f => {
            let n = (m & 0x0f) as usize;
            let elements = borrow_seq_body(buf, pos, n, depth)?;
            Ok(RecordValue::Arr(RawSeq::new(elements, n)))
        }
        0xdc => {
            let n = read_be::<2>(buf, pos)? as usize;
            let elements = borrow_seq_body(buf, pos, n, depth)?;
            Ok(RecordValue::Arr(RawSeq::new(elements, n)))
        }
        0xdd => {
            let n = read_be::<4>(buf, pos)? as usize;
            let elements = borrow_seq_body(buf, pos, n, depth)?;
            Ok(RecordValue::Arr(RawSeq::new(elements, n)))
        }

        // maps — nested RecordView over the (key,value) bytes.
        0x80..=0x8f => {
            let n = (m & 0x0f) as usize;
            let body = borrow_map_body(buf, pos, n, depth)?;
            Ok(RecordValue::Map(RecordView::from_map_body(body, n)))
        }
        0xde => {
            let n = read_be::<2>(buf, pos)? as usize;
            let body = borrow_map_body(buf, pos, n, depth)?;
            Ok(RecordValue::Map(RecordView::from_map_body(body, n)))
        }
        0xdf => {
            let n = read_be::<4>(buf, pos)? as usize;
            let body = borrow_map_body(buf, pos, n, depth)?;
            Ok(RecordValue::Map(RecordView::from_map_body(body, n)))
        }

        // ext — borrow the payload as Bin (matches the tree decoder's collapse).
        // The type byte is skipped via checked_add (untrusted-input safe).
        0xd4 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, 1)?))
        }
        0xd5 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, 2)?))
        }
        0xd6 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, 4)?))
        }
        0xd7 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, 8)?))
        }
        0xd8 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, 16)?))
        }
        0xc7 => {
            let n = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, n)?))
        }
        0xc8 => {
            let n = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, n)?))
        }
        0xc9 => {
            let n = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(RecordValue::Bin(borrow_bytes(buf, pos, n)?))
        }

        0xc1 => Err(RecordViewError::ReservedMarker(start)),
    }
}

/// Materialise a `u64` as the canonical decoder would: `Int` if it fits in
/// `i64`, else `Big(u64)` without allocating. The lens preserves the exact
/// unsigned value; materialization constructs `InnerValue::Big`.
/// See the [`record_value`](crate::record_view::record_value) module docs.
#[inline]
fn uint_to_record_value(u: u64) -> RecordValue<'static> {
    if let Ok(i) = i64::try_from(u) {
        RecordValue::Int(i)
    } else {
        RecordValue::Big(u)
    }
}

/// Validate and borrow the contiguous body of an array of `n` elements
/// (starting at the first element's marker). Returns a slice that exactly
/// covers those elements so [`RawSeq`] can re-walk them lazily.
fn borrow_seq_body<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    n: usize,
    depth: usize,
) -> Result<&'a [u8], RecordViewError> {
    let start = *pos;
    for _ in 0..n {
        skip_value(buf, pos, depth + 1)?;
    }
    if *pos > buf.len() {
        return Err(RecordViewError::Truncated(start));
    }
    Ok(&buf[start..*pos])
}

/// Validate and borrow the contiguous body of a map of `n` entries (starting
/// at the first key's marker). Returns a slice covering all `(key, value)`
/// pairs so a nested [`RecordView`] can re-walk them.
fn borrow_map_body<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    n: usize,
    depth: usize,
) -> Result<&'a [u8], RecordViewError> {
    let start = *pos;
    for _ in 0..n {
        skip_value(buf, pos, depth + 1)?; // key
        skip_value(buf, pos, depth + 1)?; // value
    }
    if *pos > buf.len() {
        return Err(RecordViewError::Truncated(start));
    }
    Ok(&buf[start..*pos])
}
