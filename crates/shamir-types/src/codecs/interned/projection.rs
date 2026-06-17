//! Server-side id-keyed msgpack projection for the S-read pass-through path.
//!
//! [`record_view_to_id_msgpack`] extracts a SUBSET of fields from a stored
//! id-keyed record and emits them as a new id-keyed msgpack map, without
//! de-interning any key. The returned bytes are wire-ready for the client,
//! which performs de-interning using the shared interner dictionary.
//!
//! Value bytes are copied **verbatim** from the source record via
//! [`RecordView::field_value_bytes`] — no decode, no re-encode.
//!
//! # Use case
//! `SELECT a, c` over a wide record: the engine projects only the requested
//! field ids, wraps them in a fresh map header, and returns the result.
//! `SELECT *` uses the raw storage bytes directly — this function covers the
//! PROJECTION (subset) path only.

use crate::codecs::interned::messagepack::write_map_header;
use crate::codecs::CodecError;
use crate::core::interner::InternerKey;
use crate::record_view::RecordView;
use bytes::Bytes;

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// Emit an id-keyed msgpack MAP containing only the fields from `view` whose
/// ids appear in `selected_ids`, in `selected_ids` order.
///
/// For each id in `selected_ids`:
/// - If the field is present in `view` (via [`RecordView::field_value_bytes`]),
///   emit: the key as `InternerKey::serialize` (msgpack bin, same as the storage
///   codec) + the value bytes **verbatim** from the source record.
/// - If the field is absent, it is silently skipped.
///
/// The map header (FixMap/Map16/Map32) is written for the count of *present*
/// ids (absent ids do not contribute to the count).
///
/// The output is a valid standalone msgpack map that can be decoded by the
/// client's interner-aware decoder.
pub fn record_view_to_id_msgpack(
    view: &RecordView<'_>,
    selected_ids: &[InternerKey],
) -> Result<Bytes, CodecError> {
    // First pass: collect (serialised key bytes, value bytes) for each present
    // id, in selected_ids order. We need the count before writing the map header.
    //
    // InternerKey::serialize → rmp_serde → msgpack bin (variable-width LE).
    // We replicate the bin8/bin16/bin32 header manually to avoid pulling in
    // rmp_serde here (and to stay allocation-free for the header itself).
    //
    // Two-pass: collect pairs first, then emit map header + pairs.
    // This avoids a second scan of `view` and keeps the map count exact.

    // Reserve capacity: up to selected_ids.len() entries, each entry ≤
    // 2 (bin8 hdr) + 8 (key payload) bytes for the key, plus the value span.
    // Exact value size is unknown upfront — we over-estimate with the old-bytes
    // size as an upper bound. Use a simple Vec; no heap pressure for typical
    // projection widths (1-20 fields).
    let mut pairs: Vec<(KeyBytes, &[u8])> = Vec::with_capacity(selected_ids.len());

    for id in selected_ids {
        if let Some(val_bytes) = view.field_value_bytes(id.clone()) {
            pairs.push((encode_key(id), val_bytes));
        }
        // Absent ids: silently skip (per spec).
    }

    let present_count = pairs.len();

    // Estimate output capacity: 5 (max map header) + sum of key+value sizes.
    let value_total: usize = pairs
        .iter()
        .map(|(kb, vb)| kb.as_slice().len() + vb.len())
        .sum();
    let mut buf: Vec<u8> = Vec::with_capacity(5 + value_total);

    // Write the map header for the number of PRESENT entries.
    write_map_header(&mut buf, present_count)?;

    // Write each (key, value) pair.
    for (key_bytes, val_bytes) in pairs {
        buf.extend_from_slice(key_bytes.as_slice());
        buf.extend_from_slice(val_bytes);
    }

    Ok(Bytes::from(buf))
}

// ---------------------------------------------------------------------------
// Key encoding
// ---------------------------------------------------------------------------

/// Encode one `InternerKey` as a msgpack `bin8` value, matching the byte-level
/// output of `rmp_serde::to_vec(&key)` / `InternerKey::serialize` exactly.
///
/// `InternerKey::as_bytes_buf` returns 1, 2, 4, or 8 payload bytes (all ≤ 255),
/// so the header is always `bin8` (0xc4 + 1-byte length). This matches the
/// exact wire encoding the storage codec uses for map keys.
///
/// Wire layout: `0xc4` | `payload_len as u8` | LE id bytes (1/2/4/8 bytes).
#[inline]
fn encode_key(id: &InternerKey) -> KeyBytes {
    let (payload, payload_len) = id.as_bytes_buf();
    // payload_len ∈ {1, 2, 4, 8} — always ≤ 255, so bin8 header is sufficient.
    // Total: 2 (marker + length byte) + payload_len ≤ 10 bytes.
    let total = 2 + payload_len;
    let mut buf = [0u8; 16];
    buf[0] = 0xc4; // bin8 marker
    buf[1] = payload_len as u8; // 1-byte length (BE = same as plain u8)
    buf[2..total].copy_from_slice(&payload[..payload_len]);
    KeyBytes { buf, len: total }
}

/// A stack-allocated buffer holding an encoded msgpack bin key. `as_ref()`
/// returns the valid byte slice (first `len` bytes).
struct KeyBytes {
    buf: [u8; 16],
    len: usize,
}

impl KeyBytes {
    /// Borrow the valid encoded bytes (first `self.len` bytes of the stack buffer).
    #[inline]
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
