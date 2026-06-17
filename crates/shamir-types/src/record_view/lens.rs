//! The zero-copy borrowing lens over canonical (id-keyed) MessagePack record
//! bytes produced by `InnerValue::to_bytes()`. One primary export:
//! [`RecordView`].
//!
//! The storage codec serialises `InternerKey` map keys as msgpack **`bin`**
//! (variable-width little-endian id bytes via `InternerKey::serialize` →
//! `serialize_bytes`). The lens matches keys by encoding the target field's
//! `u64` id to the same LE wire bytes (a la `eval_bytes::interned_key_bytes`)
//! and comparing against each `bin` key in the map.
//!
//! ## Marker coverage
//! The lens handles EXACTLY the marker set the production encoder emits and
//! the canonical decoder accepts: all int widths (`FixPos`/`FixNeg`/`U8`..
//! `U64`/`I8`..`I64`), `F32`/`F64`, `FixStr`/`Str8`/`Str16`/`Str32`,
//! `Bin8`/`Bin16`/`Bin32`, `FixArray`/`Array16`/`Array32`,
//! `FixMap`/`Map16`/`Map32`, all ext (`FixExt1`/`FixExt2`/`FixExt4`/
//! `FixExt8`/`FixExt16`/`Ext8`/`Ext16`/`Ext32`), `Null`/`True`/`False`.
//! Reserved markers return [`RecordViewError::ReservedMarker`].
//!
//! ## Untrusted-input safety
//! Every primitive returns [`Result`] / [`Option`] and bounds-checks every
//! read. The lens NEVER panics on malformed/truncated bytes — it surfaces a
//! [`RecordViewError`] (or `None` for single-field misses, matching the
//! `Option` return of [`RecordView::get`]).
//!
//! ## Forward compatibility
//! The API is shaped to sit behind the Stage-2 `RecordRef` trait
//! (`get` / `fields` / `get_path`) without reshaping — both `InnerValue` and
//! `RecordView` will implement it, with static dispatch.

use crate::core::interner::InternerKey;
use crate::record_view::record_value::{RawSeq, RecordValue};
use crate::types::common::THasher;
use shamir_collections::TFxMap;
use std::borrow::Cow;
use thiserror::Error;

/// Maximum nesting depth. Mirrors `messagepack::MAX_MSGPACK_DEPTH` so the lens
/// and the tree decoder agree on where "too deep" begins; deeper input returns
/// [`RecordViewError::DepthExceeded`] rather than overflowing the stack.
pub const MAX_MSGPACK_DEPTH: usize = 128;

/// Errors raised by the lens on malformed/truncated/unexpected input. The lens
/// is untrusted-input safe: it surfaces one of these (or `None` for a
/// single-field miss) and never panics.
#[derive(Error, Debug)]
pub enum RecordViewError {
    /// The buffer ended before a full marker+payload could be read.
    #[error("truncated MessagePack buffer at byte {0}")]
    Truncated(usize),
    /// The top-level marker (or a context requiring a map) was not a map.
    #[error("non-map marker where a map was expected (byte {got:#04x})")]
    NonMapTopLevel { got: u8 },
    /// A string payload was not valid UTF-8 (the canonical decoder rejects the
    /// same; the lens only validates when a `&str` is materialised).
    #[error("invalid UTF-8 in string at byte {0}")]
    InvalidUtf8(usize),
    /// A map key was not a `bin` marker (the storage codec encodes
    /// `InternerKey` as msgpack `bin`; encountering a non-bin key means the
    /// buffer is not in storage form).
    #[error("non-bin map key at byte {0}")]
    NonBinKey(usize),
    /// Nesting deeper than [`MAX_MSGPACK_DEPTH`].
    #[error("MessagePack nesting depth exceeds {0}")]
    DepthExceeded(usize),
    /// A reserved marker byte (`0xc1`) was encountered — the canonical decoder
    /// rejects these too.
    #[error("reserved MessagePack marker at byte {0}")]
    ReservedMarker(usize),
    /// An unknown marker byte outside the MessagePack spec.
    #[error("unknown MessagePack marker {got:#04x} at byte {at}")]
    UnknownMarker { got: u8, at: usize },
}

impl RecordViewError {
    /// Lift the canonical decoder's depth-cap constant into the error text —
    /// kept here so the two stay numerically identical without a cross-module
    /// const import (the decoder's is private by design).
    #[inline]
    fn depth_cap() -> usize {
        MAX_MSGPACK_DEPTH
    }
}

// ---------------------------------------------------------------------------
// Interned-key encoding — matches `InternerKey::serialize` (variable-width LE)
// and `eval_bytes::interned_key_bytes` exactly.
// ---------------------------------------------------------------------------

/// A minimal stack buffer for the variable-width interned key bytes.
/// Matches the encoding in `InternerKey::as_bytes_buf` / `eval_bytes::interned_key_bytes`.
struct KeyBuf([u8; 8], usize);

impl KeyBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0[..self.1]
    }
}

/// Encode a `u64` interned id to the same variable-width LE bytes that
/// `InternerKey::serialize` produces (1/2/4/8 bytes depending on magnitude).
#[inline]
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

// ---------------------------------------------------------------------------
// Low-level cursor primitives — read raw bytes with bounds checks. These are
// the inlined hot path; keeping them as standalone `&[u8]` + `&mut usize`
// functions (not a trait + `std::io::Cursor`) is what makes the scan cheap,
// matching the verified Stage-0 prototype.
// ---------------------------------------------------------------------------

/// Read one byte at `*pos`, advancing. Bounds-checked → [`RecordViewError`].
#[inline]
fn read_u8(buf: &[u8], pos: &mut usize) -> Result<u8, RecordViewError> {
    let p = *pos;
    let b = *buf.get(p).ok_or(RecordViewError::Truncated(p))?;
    *pos = p + 1;
    Ok(b)
}

/// Read `N` big-endian bytes as a `u64`. Bounds-checked.
#[inline]
fn read_be<const N: usize>(buf: &[u8], pos: &mut usize) -> Result<u64, RecordViewError> {
    let p = *pos;
    if p + N > buf.len() {
        return Err(RecordViewError::Truncated(p));
    }
    let mut v = 0u64;
    for i in 0..N {
        v = (v << 8) | buf[p + i] as u64;
    }
    *pos = p + N;
    Ok(v)
}

/// Read the map-header marker at `*pos` and return the entry count + the
/// updated cursor. Non-map markers → [`RecordViewError::NonMapTopLevel`].
#[inline]
fn read_map_len(buf: &[u8], pos: &mut usize) -> Result<usize, RecordViewError> {
    let m = read_u8(buf, pos)?;
    match m {
        0x80..=0x8f => Ok((m & 0x0f) as usize),
        0xde => Ok(read_be::<2>(buf, pos)? as usize),
        0xdf => Ok(read_be::<4>(buf, pos)? as usize),
        other => Err(RecordViewError::NonMapTopLevel { got: other }),
    }
}

/// Read the bin-header marker at `*pos` and return the byte length of the
/// binary payload. Non-bin markers → [`RecordViewError::NonBinKey`] (used
/// for key reads in the storage form where keys are `InternerKey` serialised
/// as msgpack `bin`).
#[inline]
fn read_bin_len(buf: &[u8], pos: &mut usize) -> Result<usize, RecordViewError> {
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        0xc4 => Ok(read_be::<1>(buf, pos)? as usize),
        0xc5 => Ok(read_be::<2>(buf, pos)? as usize),
        0xc6 => Ok(read_be::<4>(buf, pos)? as usize),
        _ => Err(RecordViewError::NonBinKey(start)),
    }
}

/// Read the str-header marker at `*pos` and return the byte length of the
/// string payload. Used for VALUE reads (not keys — keys are `bin` in the
/// storage form).
#[inline]
fn read_str_len(buf: &[u8], pos: &mut usize) -> Result<usize, RecordViewError> {
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        0xa0..=0xbf => Ok((m & 0x1f) as usize),
        0xd9 => Ok(read_be::<1>(buf, pos)? as usize),
        0xda => Ok(read_be::<2>(buf, pos)? as usize),
        0xdb => Ok(read_be::<4>(buf, pos)? as usize),
        _ => Err(RecordViewError::NonBinKey(start)),
    }
}

/// Borrow `len` bytes starting at `*pos`, advancing the cursor.
#[inline]
fn borrow_bytes<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    len: usize,
) -> Result<&'a [u8], RecordViewError> {
    let p = *pos;
    let end = p.checked_add(len).ok_or(RecordViewError::Truncated(p))?;
    if end > buf.len() {
        return Err(RecordViewError::Truncated(p));
    }
    let slice = &buf[p..end];
    *pos = end;
    Ok(slice)
}

/// Borrow `len` bytes as a `&str`, validating UTF-8.
#[inline]
fn borrow_str<'a>(buf: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a str, RecordViewError> {
    let bytes = borrow_bytes(buf, pos, len)?;
    std::str::from_utf8(bytes).map_err(|_| RecordViewError::InvalidUtf8(*pos - len))
}

// ---------------------------------------------------------------------------
// The skip + read primitives over the FULL marker set.
// ---------------------------------------------------------------------------

/// Skip ONE MessagePack value (any type) starting at `*pos`. Self-delimiting:
/// reads the marker, advances past the value's payload, recursing into nested
/// maps/arrays/ext. This is the O(1)-skip the lens uses to pass over fields it
/// does not match. Bounds-checked at every step; never panics.
///
/// `depth` bounds recursion — deeper than [`MAX_MSGPACK_DEPTH`] returns
/// [`RecordViewError::DepthExceeded`] (matches the canonical decoder's guard).
pub(crate) fn skip_value(buf: &[u8], pos: &mut usize, depth: usize) -> Result<(), RecordViewError> {
    if depth > MAX_MSGPACK_DEPTH {
        return Err(RecordViewError::DepthExceeded(RecordViewError::depth_cap()));
    }
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        // positive fixint / negative fixint / nil / true / false — payload
        // already consumed (single-byte marker).
        0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => Ok(()),
        // reserved marker — the canonical decoder rejects it; so do we.
        0xc1 => Err(RecordViewError::ReservedMarker(start)),
        // unsigned / signed ints with explicit width.
        0xcc | 0xd0 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xcd | 0xd1 => {
            *pos = pos
                .checked_add(2)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xce | 0xd2 => {
            *pos = pos
                .checked_add(4)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xcf | 0xd3 => {
            *pos = pos
                .checked_add(8)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // floats.
        0xca => {
            *pos = pos
                .checked_add(4)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xcb => {
            *pos = pos
                .checked_add(8)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // strings.
        0xa0..=0xbf => {
            *pos = pos
                .checked_add((m & 0x1f) as usize)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd9 => {
            let len = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xda => {
            let len = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xdb => {
            let len = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // binary.
        0xc4 => {
            let len = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc5 => {
            let len = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc6 => {
            let len = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // arrays — skip each element.
        0x90..=0x9f => skip_seq(buf, pos, (m & 0x0f) as usize, depth),
        0xdc => {
            let n = read_be::<2>(buf, pos)? as usize;
            skip_seq(buf, pos, n, depth)
        }
        0xdd => {
            let n = read_be::<4>(buf, pos)? as usize;
            skip_seq(buf, pos, n, depth)
        }
        // maps — skip each (key, value) pair.
        0x80..=0x8f => skip_map(buf, pos, (m & 0x0f) as usize, depth),
        0xde => {
            let n = read_be::<2>(buf, pos)? as usize;
            skip_map(buf, pos, n, depth)
        }
        0xdf => {
            let n = read_be::<4>(buf, pos)? as usize;
            skip_map(buf, pos, n, depth)
        }
        // fixext — type byte + fixed-length payload.
        0xd4 => {
            *pos = pos
                .checked_add(1 + 1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd5 => {
            *pos = pos
                .checked_add(1 + 2)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd6 => {
            *pos = pos
                .checked_add(1 + 4)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd7 => {
            *pos = pos
                .checked_add(1 + 8)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd8 => {
            *pos = pos
                .checked_add(1 + 16)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // ext — length-prefixed payload + type byte.
        0xc7 => {
            let n = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1 + n)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc8 => {
            let n = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1 + n)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc9 => {
            let n = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1 + n)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
    }?;
    // Post-skip bounds check: any fixed-width arm above (`*pos += k`) may have
    // overrun a truncated buffer; catch it here in one place.
    if *pos > buf.len() {
        return Err(RecordViewError::Truncated(start));
    }
    Ok(())
}

#[inline]
fn skip_seq(buf: &[u8], pos: &mut usize, n: usize, depth: usize) -> Result<(), RecordViewError> {
    for _ in 0..n {
        skip_value(buf, pos, depth + 1)?;
    }
    Ok(())
}

#[inline]
fn skip_map(buf: &[u8], pos: &mut usize, n: usize, depth: usize) -> Result<(), RecordViewError> {
    for _ in 0..n {
        skip_value(buf, pos, depth + 1)?; // key
        skip_value(buf, pos, depth + 1)?; // value
    }
    Ok(())
}

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

        // unsigned ints — U64 > i64::MAX mirrors the tree's Str mapping.
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
/// `i64`, else `Str(decimal)` (owned — the bytes are a raw u64, not text).
/// See the [`record_value`](crate::record_view::record_value) module docs.
#[inline]
fn uint_to_record_value(u: u64) -> RecordValue<'static> {
    if let Ok(i) = i64::try_from(u) {
        RecordValue::Int(i)
    } else {
        RecordValue::Str(Cow::Owned(u.to_string()))
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

// ---------------------------------------------------------------------------
// The lens itself.
// ---------------------------------------------------------------------------

/// A lazy offset-index mapping interned field id → byte offset of the value
/// within the map body, plus a back-reference to that body. Built once via
/// [`RecordView::index`] so N field lookups are O(fields) amortised instead of
/// O(fields^2) — keyed by `u64` (the interned id).
///
/// Single-shot [`RecordView::get`] does NOT build this (a linear scan is
/// cheaper for one field). Consumers reading several fields should build an
/// index once and probe it. Keep the offsets a handful of bytes — `u32` is
/// ample for any record that fits in 4 GiB.
#[derive(Debug, Clone)]
pub struct FieldIndex<'a> {
    body: &'a [u8],
    offsets: TFxMap<u64, u32>,
}

impl<'a> FieldIndex<'a> {
    /// Internal constructor — `body` is the map body, `offsets` maps each
    /// interned field id to the byte offset (within `body`) of its value's
    /// marker.
    #[inline]
    fn new(body: &'a [u8], offsets: TFxMap<u64, u32>) -> Self {
        Self { body, offsets }
    }

    /// Number of indexed fields.
    #[inline]
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// `true` iff no fields were indexed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// O(1) probe — decode the value at the indexed offset, or `None` if the
    /// field is absent. Cheaper than [`RecordView::get`] for multi-field
    /// access patterns (one hash probe + one marker decode vs a linear scan).
    #[inline]
    pub fn get(&self, field_id: InternerKey) -> Option<RecordValue<'a>> {
        let &off = self.offsets.get(&field_id.id())?;
        let mut pos = off as usize;
        read_value(self.body, &mut pos, 0).ok()
    }

    /// O(1) probe by raw `u64` id.
    #[inline]
    pub fn get_by_id(&self, id: u64) -> Option<RecordValue<'a>> {
        let &off = self.offsets.get(&id)?;
        let mut pos = off as usize;
        read_value(self.body, &mut pos, 0).ok()
    }

    /// O(1) probe returning the typed int, mirroring
    /// [`RecordView::get_int`].
    #[inline]
    pub fn get_int(&self, field_id: InternerKey) -> Option<i64> {
        self.get(field_id).and_then(|v| v.as_int())
    }

    /// O(1) probe returning the borrowed str (real string markers only — see
    /// [`RecordView::get_str`]).
    #[inline]
    pub fn get_str(&self, field_id: InternerKey) -> Option<&'a str> {
        // Require a real str marker at the indexed offset (not the U64 edge).
        let &off = self.offsets.get(&field_id.id())?;
        let mut pos = off as usize;
        let vlen = read_str_len(self.body, &mut pos).ok()?;
        borrow_str(self.body, &mut pos, vlen).ok()
    }
}

/// A zero-copy borrowing cursor over canonical (id-keyed) MessagePack record
/// bytes produced by `InnerValue::to_bytes()`. Read fields on demand without
/// materialising an `InnerValue` tree — see the [module docs](super) and
/// `docs/perf/record-view-migration.md` §9.
///
/// Map keys in the storage form are `InternerKey` serialised as msgpack `bin`
/// (variable-width LE bytes). The lens matches keys by encoding the target
/// field's id to the same wire bytes and comparing against each `bin` key.
///
/// Construct with [`RecordView::new`] (validates the top-level map header) or
/// [`RecordView::from_map_body`] (internal — used for nested-map values whose
/// header has already been consumed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordView<'a> {
    /// The slice starting *at the first map entry's key marker* (the map
    /// header is consumed at construction and the entry count is stored in
    /// `n_entries`). For a top-level view this is `buf` after the header.
    body: &'a [u8],
    /// Number of `(key, value)` entries in the map (from the header).
    n_entries: usize,
}

impl<'a> RecordView<'a> {
    /// Construct a lens over `buf`, validating cheaply that the top-level
    /// marker is a map (header read only — no full scan, no tree decode).
    /// Stored msgpack records are maps with bin keys (interned ids); a non-map
    /// top level is a programming/storage error and returns
    /// [`RecordViewError::NonMapTopLevel`].
    #[inline]
    pub fn new(buf: &'a [u8]) -> Result<Self, RecordViewError> {
        let mut pos = 0usize;
        let n_entries = read_map_len(buf, &mut pos)?;
        let body = buf.get(pos..).ok_or(RecordViewError::Truncated(pos))?;
        Ok(Self { body, n_entries })
    }

    /// Internal constructor for a nested map whose header has already been
    /// consumed — `body` points at the first entry's key marker, `n_entries`
    /// is the map's entry count.
    #[inline]
    fn from_map_body(body: &'a [u8], n_entries: usize) -> Self {
        Self { body, n_entries }
    }

    /// The raw buffer slice (map body — starts at the first entry's key
    /// marker). Exposed for parity tests that need to re-decode via the
    /// canonical path; not on the Stage-2 `RecordRef` surface.
    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.body
    }

    /// Number of entries in the top-level map (from the header — no scan).
    #[inline]
    pub fn len(&self) -> usize {
        self.n_entries
    }

    /// `true` iff the header reported zero entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n_entries == 0
    }

    /// Scan the top-level map for the entry whose `bin` key bytes match the
    /// encoded `InternerKey` for `field_id`, returning the value decoded as a
    /// borrowed [`RecordValue`]. Decodes only the matched value's marker —
    /// non-matching values are skipped with O(1) [`skip_value`] calls.
    /// Returns `None` on miss.
    ///
    /// On a malformed buffer the scan terminates and returns `None` — the lens
    /// is untrusted-input safe and never panics. Use
    /// [`get_with_err`](Self::get_with_err) if you need to distinguish
    /// "missing" from "malformed".
    #[inline]
    pub fn get(&self, field_id: InternerKey) -> Option<RecordValue<'a>> {
        self.get_with_err(field_id).ok().flatten()
    }

    /// Convenience: look up by raw `u64` id instead of `InternerKey`.
    #[inline]
    pub fn get_by_id(&self, id: u64) -> Option<RecordValue<'a>> {
        self.get(InternerKey::new(id))
    }

    /// Like [`get`](Self::get) but surfaces the error: `Ok(None)` = field
    /// absent, `Ok(Some(v))` = found, `Err(_)` = malformed buffer.
    #[inline]
    pub fn get_with_err(
        &self,
        field_id: InternerKey,
    ) -> Result<Option<RecordValue<'a>>, RecordViewError> {
        let target = interned_key_bytes(field_id.id());
        let target_bytes = target.as_ref();
        let mut pos = 0usize;
        for _ in 0..self.n_entries {
            let klen = read_bin_len(self.body, &mut pos)?;
            let kstart = pos;
            let kend = kstart
                .checked_add(klen)
                .ok_or(RecordViewError::Truncated(kstart))?;
            if kend > self.body.len() {
                return Err(RecordViewError::Truncated(kstart));
            }
            pos = kend;
            if klen == target_bytes.len() && self.body[kstart..kend] == *target_bytes {
                // match — decode just this value.
                let v = read_value(self.body, &mut pos, 0)?;
                return Ok(Some(v));
            }
            // miss — skip the value entirely (O(1)).
            skip_value(self.body, &mut pos, 0)?;
        }
        Ok(None)
    }

    /// `Some(i)` iff `field_id` is present and decodes to an integer (any width).
    #[inline]
    pub fn get_int(&self, field_id: InternerKey) -> Option<i64> {
        self.get(field_id).and_then(|v| v.as_int())
    }

    /// `Some(f)` iff `field_id` is present and decodes to a float (`F32` widened
    /// or `F64`).
    #[inline]
    pub fn get_f64(&self, field_id: InternerKey) -> Option<f64> {
        self.get(field_id).and_then(|v| v.as_f64())
    }

    /// `Some(b)` iff `field_id` is present and decodes to a bool.
    #[inline]
    pub fn get_bool(&self, field_id: InternerKey) -> Option<bool> {
        self.get(field_id).and_then(|v| v.as_bool())
    }

    /// `Some(s)` iff `field_id` is present, decodes to a string, and the value
    /// bytes are a real string marker (borrows the payload — zero-copy). Returns
    /// `None` for the U64 > `i64::MAX` edge (whose value bytes are a raw u64,
    /// not a string — there is no borrowable `&str` in the buffer); use
    /// [`get_str_owned`](Self::get_str_owned) for that case.
    #[inline]
    pub fn get_str(&self, field_id: InternerKey) -> Option<&'a str> {
        // Scan manually so we can require the value marker to be a real str
        // marker (read_value would map the U64 edge to an Owned Cow).
        let target = interned_key_bytes(field_id.id());
        let target_bytes = target.as_ref();
        let mut pos = 0usize;
        for _ in 0..self.n_entries {
            let klen = match read_bin_len(self.body, &mut pos) {
                Ok(n) => n,
                Err(_) => return None,
            };
            let kstart = pos;
            let kend = kstart.checked_add(klen)?;
            if kend > self.body.len() {
                return None;
            }
            pos = kend;
            if klen == target_bytes.len() && self.body[kstart..kend] == *target_bytes {
                // match — read the value as a string marker specifically.
                let vlen = read_str_len(self.body, &mut pos).ok()?;
                return borrow_str(self.body, &mut pos, vlen).ok();
            }
            skip_value(self.body, &mut pos, 0).ok()?;
        }
        None
    }

    /// Like [`get_str`](Self::get_str) but returns a `Cow`, accommodating the
    /// U64 > `i64::MAX` edge (which owns its decimal text). Prefer
    /// [`get_str`](Self::get_str) when the field is a real string field.
    #[inline]
    pub fn get_str_owned(&self, field_id: InternerKey) -> Option<Cow<'a, str>> {
        self.get(field_id).and_then(|v| match v {
            RecordValue::Str(s) => Some(s),
            _ => None,
        })
    }

    /// `Some(b)` iff `field_id` is present and decodes to binary / an ext payload.
    #[inline]
    pub fn get_bytes(&self, field_id: InternerKey) -> Option<&'a [u8]> {
        self.get(field_id).and_then(|v| v.as_bytes())
    }

    /// Filter-eval on BYTES — the cheapest, hottest case (`WHERE city =
    /// 'NYC'`). Scan to `field_id`, compare its raw string value bytes to
    /// `literal` without constructing a `String` or decoding a typed value:
    /// one scan + one slice compare, zero allocation.
    ///
    /// Returns `false` on miss or non-string value (the row simply does not
    /// match the predicate). On a malformed buffer the scan terminates and
    /// returns `false` — untrusted-input safe.
    #[inline]
    pub fn match_str_eq(&self, field_id: InternerKey, literal: &[u8]) -> bool {
        let target = interned_key_bytes(field_id.id());
        let target_bytes = target.as_ref();
        let mut pos = 0usize;
        for _ in 0..self.n_entries {
            let klen = match read_bin_len(self.body, &mut pos) {
                Ok(n) => n,
                Err(_) => return false,
            };
            let kstart = pos;
            let kend = match kstart.checked_add(klen) {
                Some(end) => end,
                None => return false,
            };
            if kend > self.body.len() {
                return false;
            }
            pos = kend;
            if klen == target_bytes.len() && self.body[kstart..kend] == *target_bytes {
                // match — compare the value's string bytes directly (no decode).
                let vlen = match read_str_len(self.body, &mut pos) {
                    Ok(n) => n,
                    Err(_) => return false,
                };
                let vstart = pos;
                let vend = match vstart.checked_add(vlen) {
                    Some(e) => e,
                    None => return false,
                };
                if vend > self.body.len() {
                    return false;
                }
                return vlen == literal.len() && self.body[vstart..vend] == *literal;
            }
            // miss — skip the value.
            if skip_value(self.body, &mut pos, 0).is_err() {
                return false;
            }
        }
        false
    }

    /// Lazy walk over the top-level map's `(key, value)` entries. Each
    /// `(InternerKey, borrowed value)` pair is decoded on demand; the cursor
    /// advances with O(1) skips. Iteration stops at the first malformed entry
    /// (residual dropped — never panics). This is the projection primitive
    /// (e.g. `SELECT a, b, c` over a wide record).
    pub fn fields(&self) -> impl Iterator<Item = (InternerKey, RecordValue<'a>)> {
        let mut pos = 0usize;
        let mut remaining = self.n_entries;
        let body = self.body;
        std::iter::from_fn(move || {
            if remaining == 0 {
                return None;
            }
            // Read the key (bin marker + payload bytes → InternerKey).
            let klen = match read_bin_len(body, &mut pos) {
                Ok(n) => n,
                Err(_) => return None,
            };
            let kstart = pos;
            let kend = match kstart.checked_add(klen) {
                Some(e) => e,
                None => return None,
            };
            if kend > body.len() {
                return None;
            }
            let key_bytes = &body[kstart..kend];
            pos = kend;
            // Decode the interned id from LE bytes.
            let id = match klen {
                1 => key_bytes[0] as u64,
                2 => u16::from_le_bytes([key_bytes[0], key_bytes[1]]) as u64,
                4 => u32::from_le_bytes([key_bytes[0], key_bytes[1], key_bytes[2], key_bytes[3]])
                    as u64,
                8 => u64::from_le_bytes([
                    key_bytes[0],
                    key_bytes[1],
                    key_bytes[2],
                    key_bytes[3],
                    key_bytes[4],
                    key_bytes[5],
                    key_bytes[6],
                    key_bytes[7],
                ]),
                _ => return None, // invalid key length
            };
            let key = InternerKey::new(id);
            // Read the value.
            let val = match read_value(body, &mut pos, 0) {
                Ok(v) => v,
                Err(_) => return None,
            };
            remaining -= 1;
            Some((key, val))
        })
    }

    /// Nested path access — descend through maps using interned ids. At each
    /// step, `get` the field by id; if its value is a nested `Map`, recurse
    /// into it. Returns `None` on any miss *or* when a path component resolves
    /// to a non-map value (you cannot descend through a scalar/array/bin). An
    /// empty `path` is vacuous and returns `None`.
    ///
    /// This is the lens's analogue of `InnerValue::Map { a: Map { b: Map { c: .. } } }`
    /// projection — zero-copy at every level.
    pub fn get_path(&self, path: &[InternerKey]) -> Option<RecordValue<'a>> {
        if path.is_empty() {
            return None;
        }
        let mut current = self.get(path[0].clone())?;
        for component in &path[1..] {
            current = match current {
                RecordValue::Map(ref m) => m.get(component.clone())?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Build (or reuse) a lazy offset-index mapping interned field id → byte
    /// offset of the value within the map body. After the first multi-field
    /// access, subsequent lookups are O(1) hash probes instead of O(fields)
    /// scans — so reading N fields is O(fields) amortised, not O(fields^2).
    ///
    /// Single-shot [`get`](Self::get) does NOT build this (the scan is cheaper
    /// for one field). Consumers reading several fields should call
    /// [`index`](Self::index) once and probe it. Uses [`THasher`] per the
    /// workspace default.
    pub fn index(&self) -> FieldIndex<'a> {
        let mut map: TFxMap<u64, u32> =
            TFxMap::with_capacity_and_hasher(self.n_entries, THasher::default());
        let mut pos = 0usize;
        for _ in 0..self.n_entries {
            let klen = match read_bin_len(self.body, &mut pos) {
                Ok(n) => n,
                Err(_) => break,
            };
            let kstart = pos;
            let kend = match kstart.checked_add(klen) {
                Some(e) => e,
                None => break,
            };
            if kend > self.body.len() {
                break;
            }
            let key_bytes = &self.body[kstart..kend];
            pos = kend;
            // Decode the interned id from LE bytes.
            let id = match klen {
                1 => key_bytes[0] as u64,
                2 => u16::from_le_bytes([key_bytes[0], key_bytes[1]]) as u64,
                4 => u32::from_le_bytes([key_bytes[0], key_bytes[1], key_bytes[2], key_bytes[3]])
                    as u64,
                8 => u64::from_le_bytes([
                    key_bytes[0],
                    key_bytes[1],
                    key_bytes[2],
                    key_bytes[3],
                    key_bytes[4],
                    key_bytes[5],
                    key_bytes[6],
                    key_bytes[7],
                ]),
                _ => break,
            };
            // Record the value's offset, then skip it.
            let value_offset = pos as u32;
            if skip_value(self.body, &mut pos, 0).is_err() {
                break;
            }
            map.insert(id, value_offset);
        }
        FieldIndex::new(self.body, map)
    }

    /// Return the raw msgpack byte slice for a single **top-level** field's
    /// value. Returns `None` on miss or malformed buffer. The returned slice is
    /// a valid standalone msgpack value suitable for
    /// `InnerValue::from_bytes()` — including any nested map subtree, which
    /// decodes back to a `Value<InternerKey>` tree identical to the one the
    /// full-record decoder would have produced for that field.
    ///
    /// This is the public projection surface for the per-row decode-prune optimisation
    /// on the aggregate / GROUP BY read path: callers extract only the referenced
    /// top-level fields and skip decoding the wide unreferenced ones. The wrapper
    /// is deliberately restricted to a single top-level id (not an arbitrary
    /// path) so it does not widen the lens's internal navigation surface —
    /// `value_bytes_at` below remains the `pub(crate)` multi-segment primitive.
    #[inline]
    pub fn field_value_bytes(&self, id: InternerKey) -> Option<&'a [u8]> {
        self.value_bytes_at(std::slice::from_ref(&id))
    }

    /// Return the raw msgpack byte slice for the value at `path` (navigating
    /// through nested maps). Returns `None` on miss, path-through-non-map, or
    /// malformed buffer. The returned slice is a valid standalone msgpack value
    /// suitable for `InnerValue::from_bytes()`.
    pub(crate) fn value_bytes_at(&self, path: &[InternerKey]) -> Option<&'a [u8]> {
        if path.is_empty() {
            return None;
        }
        // Find the value bytes for the first path segment in this map.
        let (first, rest) = path.split_first()?;
        let target = interned_key_bytes(first.id());
        let target_bytes = target.as_ref();
        let mut pos = 0usize;
        for _ in 0..self.n_entries {
            let klen = read_bin_len(self.body, &mut pos).ok()?;
            let kstart = pos;
            let kend = kstart.checked_add(klen)?;
            if kend > self.body.len() {
                return None;
            }
            pos = kend;
            if klen == target_bytes.len() && self.body[kstart..kend] == *target_bytes {
                // Found the key. Record the value's start, then figure out
                // whether we need to descend or return the bytes.
                if rest.is_empty() {
                    // Terminal segment — capture the full value byte range.
                    let val_start = pos;
                    skip_value(self.body, &mut pos, 0).ok()?;
                    return Some(&self.body[val_start..pos]);
                }
                // Non-terminal — the value must be a map; descend into it.
                let val = read_value(self.body, &mut pos, 0).ok()?;
                match val {
                    RecordValue::Map(nested) => return nested.value_bytes_at(rest),
                    _ => return None,
                }
            }
            // Miss — skip the value.
            skip_value(self.body, &mut pos, 0).ok()?;
        }
        None
    }
}
