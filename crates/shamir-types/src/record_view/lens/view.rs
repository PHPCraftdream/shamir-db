use super::cursor::{borrow_str, interned_key_bytes, read_bin_len, read_map_len, read_str_len};
use super::field_index::FieldIndex;
use super::read_value::read_value;
use super::skip::skip_value;
use super::RecordViewError;
use crate::core::interner::InternerKey;
use crate::record_view::record_value::RecordValue;
use crate::types::common::THasher;
use shamir_collections::TFxMap;
use std::borrow::Cow;

/// A zero-copy borrowing cursor over canonical (id-keyed) MessagePack record
/// bytes produced by `InnerValue::to_bytes()`. Read fields on demand without
/// materialising an `InnerValue` tree — see the [module docs](super) and
/// `docs/dev-artifacts/perf/record-view-migration.md` §9.
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
    pub(super) fn from_map_body(body: &'a [u8], n_entries: usize) -> Self {
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

    /// Remaining bytes in the map body, bounding header-derived allocations.
    #[inline]
    pub fn body_len(&self) -> usize {
        self.body.len()
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
        // Cap prealloc by body size: `n_entries` is untrusted (raw header
        // count, never validated) and each wire entry consumes ≥ 2 bytes
        // (bin8 key header alone) — same bounded-prealloc rule as the msgpack
        // decoders.
        let prealloc = self.n_entries.min(self.body.len() / 2);
        let mut map: TFxMap<u64, (usize, usize)> =
            TFxMap::with_capacity_and_hasher(prealloc, THasher::default());
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
            // Record the value's span, then skip it.
            let value_offset = pos;
            if skip_value(self.body, &mut pos, 0).is_err() {
                break;
            }
            map.insert(id, (value_offset, pos));
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

    /// Single-pass byte-range walk over the top-level map entries: calls `f`
    /// with each `(key, raw value byte slice)` in record order — O(fields)
    /// total, no per-field re-scan and no value decode. `Err` entries
    /// (malformed) terminate the walk.
    pub(crate) fn for_each_field_bytes(&self, f: &mut dyn FnMut(InternerKey, &[u8])) {
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
                _ => break, // invalid key length
            };
            let val_start = pos;
            if skip_value(self.body, &mut pos, 0).is_err() {
                break;
            }
            f(InternerKey::new(id), &self.body[val_start..pos]);
        }
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
