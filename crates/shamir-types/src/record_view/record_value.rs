//! Borrowed scalar/aggregate value produced by the [`RecordView`](super::RecordView)
//! lens over canonical (id-keyed) MessagePack record bytes.
//!
//! One primary export: [`RecordValue`]. Variants mirror the marker set the
//! production encoder/decoder pair (`codecs::interned::messagepack`) accepts,
//! with the same value-shape decisions:
//!
//! * all integer widths collapse to `Int(i64)` (positive/negative fixint,
//!   `u8`/`u16`/`u32`/`u64`, `i8`/`i16`/`i32`/`i64`) — exactly as the tree
//!   decoder's `decode_value` does;
//! * `F32` and `F64` collapse to `F64(f64)` (the tree stores both as `F64`);
//! * strings (`FixStr`/`Str8`/`Str16`/`Str32`) borrow the payload bytes as a
//!   `&str` — zero-copy, no `String` allocation;
//! * `Bin8`/`Bin16`/`Bin32` and all ext types borrow the payload as `&[u8]`
//!   (the tree decoder stores ext payloads as `Bin`, so the lens does too);
//! * arrays and maps are exposed lazily as [`RawSeq`] / nested
//!   [`RecordView`](super::RecordView) cursors over the raw value bytes — no
//!   element materialisation;
//! * `Null` / `True` / `False` are decoded inline.
//!
//! ## The U64 > `i64::MAX` edge
//! The tree decoder maps a `u64` that does not fit in `i64` to
//! `InnerValue::Str(decimal_string)` (see `messagepack.rs::decode_value`,
//! `Marker::U64` arm). The lens mirrors that mapping so `RecordView::get`
//! agrees with `msgpack_to_inner` for *every* input the encoder can produce.
//! Because the decimal text does not exist in the buffer (the bytes are a raw
//! `u64`), this single edge case must own its text: the [`Str`] variant is
//! therefore a `Cow<'a, str>` — borrowed for the overwhelmingly common path
//! (real string fields) and owned only for this synthetic decimal. No other
//! scalar variant allocates.
//!
//! [`Str`]: RecordValue::Str

use super::RecordView;
use std::borrow::Cow;

/// A borrowed, lazily-decoded view over a contiguous MessagePack sequence
/// (array or set) — the raw value bytes plus the element count. Element
/// materialisation is deferred to the consumer via [`RawSeq::iter`].
///
/// This is the aggregate analogue of [`RecordView`] for arrays: a thin cursor
/// that skips/reads elements on demand instead of building a `Vec`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawSeq<'a> {
    /// The slice starting *at the first element* (the array marker + length
    /// header has already been consumed by the producer).
    elements: &'a [u8],
    /// Number of elements encoded in `elements`.
    len: usize,
}

impl<'a> RawSeq<'a> {
    /// Build a raw-sequence cursor. `elements` must point at the first
    /// element's marker (the array header is not included).
    #[inline]
    pub(crate) fn new(elements: &'a [u8], len: usize) -> Self {
        Self { elements, len }
    }

    /// Number of elements in the sequence (from the array header — no scan).
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` iff the header reported zero elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The raw element bytes (starting at the first element's marker).
    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.elements
    }

    /// Lazy walk over the sequence's elements. Each element is decoded on
    /// demand via [`crate::record_view::read_value`]; the cursor advances
    /// through the buffer with O(1) skips. Iteration stops at the first
    /// malformed/truncated element (the residual is dropped silently — the
    /// lens never panics on untrusted bytes).
    pub fn iter(&self) -> RawSeqIter<'a> {
        // `self.elements: &'a [u8]` is `Copy`; bind it by value so the
        // iterator's lifetime is `'a` (the buffer's), not tied to `&self`.
        RawSeqIter {
            elements: self.elements,
            pos: 0,
            remaining: self.len,
        }
    }
}

/// Lazy iterator over a [`RawSeq`]'s elements. See [`RawSeq::iter`].
#[derive(Debug)]
pub struct RawSeqIter<'a> {
    elements: &'a [u8],
    pos: usize,
    remaining: usize,
}

impl<'a> Iterator for RawSeqIter<'a> {
    type Item = RecordValue<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        match super::lens::read_value(self.elements, &mut self.pos, 0) {
            Ok(v) => {
                self.remaining -= 1;
                Some(v)
            }
            Err(_) => {
                self.remaining = 0;
                None
            }
        }
    }
}

/// A borrowed value decoded from canonical MessagePack bytes — produced by
/// [`RecordView::get`](super::RecordView::get) and friends. See the
/// [module docs](self) for the marker→variant mapping and the U64 edge case.
///
/// `PartialEq` is derived so parity tests (and Stage-2 consumers) can compare
/// lens values directly. `f64` equality is bit-wise (the lens and the tree
/// decoder store identical bits for `F32`/`F64`, so this is sound for
/// round-tripped data).
#[derive(Debug, PartialEq)]
pub enum RecordValue<'a> {
    /// `Null` (`0xc0`).
    Null,
    /// `True` (`0xc3`) / `False` (`0xc2`).
    Bool(bool),
    /// Any integer width (fixint±, `u8`/`u16`/`u32`/`u64` ≤ `i64::MAX`,
    /// `i8`/`i16`/`i32`/`i64`).
    Int(i64),
    /// `F32` (widened) / `F64`.
    F64(f64),
    /// `FixStr`/`Str8`/`Str16`/`Str32`. Borrows the payload for real string
    /// fields; owns the decimal text only for the U64 > `i64::MAX` edge (see
    /// [module docs](self)).
    Str(Cow<'a, str>),
    /// `Bin8`/`Bin16`/`Bin32` and all ext-type payloads (the tree decoder
    /// collapses ext to `Bin`, so the lens does too).
    Bin(&'a [u8]),
    /// `FixArray`/`Array16`/`Array32` — a lazy cursor over the element bytes.
    Arr(RawSeq<'a>),
    /// `FixMap`/`Map16`/`Map32` — a nested record lens over the map's
    /// `(key, value)` bytes.
    Map(RecordView<'a>),
}

impl<'a> RecordValue<'a> {
    /// `Some(i)` iff this is [`RecordValue::Int`], else `None`.
    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            RecordValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// `Some(f)` iff this is [`RecordValue::F64`], else `None`.
    #[inline]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            RecordValue::F64(f) => Some(*f),
            _ => None,
        }
    }

    /// `Some(b)` iff this is [`RecordValue::Bool`], else `None`.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            RecordValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// `Some(s)` iff this is [`RecordValue::Str`], else `None`.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            RecordValue::Str(s) => Some(s.as_ref()),
            _ => None,
        }
    }

    /// `Some(b)` iff this is [`RecordValue::Bin`], else `None`.
    #[inline]
    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        match self {
            RecordValue::Bin(b) => Some(*b),
            _ => None,
        }
    }
}
