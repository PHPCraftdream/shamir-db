//! W3: byte-level storage-map merge encoder (`merge_storage_bytes`).

use crate::codecs::CodecError;
use crate::core::interner::InternerKey;
use crate::record_view::skip_value;
use crate::types::common::TMap;
use crate::types::value::InnerValue;
use bytes::Bytes;
use serde::ser::Serialize;
use shamir_collections::{TFxMap, THasher};

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

    // Cap preallocation by the remaining bytes: each entry consumes at
    // least two bytes (key marker + payload), so a declared count beyond
    // `remaining` cannot be backed by the body.
    let remaining = old_bytes.len().saturating_sub(pos);
    let prealloc = n_old.min(remaining);
    let mut entries: Vec<Entry> = Vec::with_capacity(prealloc);
    // Build a set of old ids for fast lookup: id → index in entries vec.
    // TFxMap<u64, usize> — standard workspace fast-hash map (no untrusted input risk).
    let mut old_ids: TFxMap<u64, usize> =
        TFxMap::with_capacity_and_hasher(prealloc, THasher::default());

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
            new_val
                .serialize(&mut rmp_serde::Serializer::new(&mut buf))
                .map_err(|e| {
                    CodecError::Encode(format!("merge_storage_bytes: value encode error: {}", e))
                })?;
        } else {
            buf.extend_from_slice(&old_bytes[entry.val_start..entry.val_end]);
        }
    }

    // --- New entries (in set_map insertion order, filtered to truly new) ---
    for (key, val) in &new_keys {
        // Write the key as InternerKey::serialize → msgpack bin (variable-width LE).
        // This is the exact same encoding the storage codec uses for keys.
        key.serialize(&mut rmp_serde::Serializer::new(&mut buf))
            .map_err(|e| {
                CodecError::Encode(format!("merge_storage_bytes: key encode error: {}", e))
            })?;

        val.serialize(&mut rmp_serde::Serializer::new(&mut buf))
            .map_err(|e| {
                CodecError::Encode(format!("merge_storage_bytes: value encode error: {}", e))
            })?;
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
