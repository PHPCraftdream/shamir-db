use super::cursor::{borrow_str, read_str_len};
use super::read_value::read_value;
use crate::core::interner::InternerKey;
use crate::record_view::record_value::RecordValue;
use shamir_collections::TFxMap;

// ---------------------------------------------------------------------------
// The lens itself.
// ---------------------------------------------------------------------------

/// A lazy offset-index mapping interned field id → `(start, end)` byte span
/// of the value within the map body, plus a back-reference to that body. Built once via
/// [`RecordView::index`] so N field lookups are O(fields) amortised instead of
/// O(fields^2) — keyed by `u64` (the interned id).
///
/// Single-shot [`RecordView::get`] does NOT build this (a linear scan is
/// cheaper for one field). Consumers reading several fields should build an
/// index once and probe it. Offsets are full-width `usize` so a record larger
/// than 4 GiB can never wrap a span.
#[derive(Debug, Clone)]
pub struct FieldIndex<'a> {
    body: &'a [u8],
    offsets: TFxMap<u64, (usize, usize)>,
}

impl<'a> FieldIndex<'a> {
    /// Internal constructor — `body` is the map body, `offsets` maps each
    /// interned field id to the `(start, end)` byte span (within `body`) of
    /// its value.
    #[inline]
    pub(super) fn new(body: &'a [u8], offsets: TFxMap<u64, (usize, usize)>) -> Self {
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
        let &(off, _) = self.offsets.get(&field_id.id())?;
        let mut pos = off;
        read_value(self.body, &mut pos, 0).ok()
    }

    /// O(1) probe by raw `u64` id.
    #[inline]
    pub fn get_by_id(&self, id: u64) -> Option<RecordValue<'a>> {
        let &(off, _) = self.offsets.get(&id)?;
        let mut pos = off;
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
        let &(off, _) = self.offsets.get(&field_id.id())?;
        let mut pos = off;
        let vlen = read_str_len(self.body, &mut pos).ok()?;
        borrow_str(self.body, &mut pos, vlen).ok()
    }

    /// O(1) probe returning the precomputed raw msgpack byte span of the value
    /// (`None` if the field is absent) — the span end was recorded during the
    /// single index-building pass, so even nested containers cost no traversal
    /// here. (RV-TYPES-93D11F4F)
    pub fn value_bytes(&self, field_id: InternerKey) -> Option<&'a [u8]> {
        let &(start, end) = self.offsets.get(&field_id.id())?;
        Some(&self.body[start..end])
    }
}
