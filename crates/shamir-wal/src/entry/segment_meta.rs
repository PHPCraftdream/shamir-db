//! Sidecar metadata for a sealed WAL segment (task #500).
//!
//! Startup used to pay O(total on-disk WAL size): [`crate::SegmentSet::open`]
//! called `replay()` on EVERY sealed segment purely to compute its
//! `max_version` for version-based truncation. After a long downtime with
//! many un-truncated sealed segments, opening the database read + decoded
//! gigabytes to extract metadata per segment.
//!
//! This module adds a small, **purely additive, purely optional** sidecar
//! file — `NNNNNNNN.meta`, sitting next to the segment's `NNNNNNNN.wal` — that
//! records the maximum version and retention pin at seal time. A valid WMT3
//! sidecar supplies both without a full replay. A legacy WMT2 sidecar marks
//! the segment sealed but requires a WAL scan to determine its pin. The
//! segment's own `.wal` file format is UNTOUCHED: no in-file footer, no frame
//! the replay path must learn to skip, no format-version bump (this is exactly
//! why task #489's "format doesn't support backward-seek" deferral does NOT
//! apply — nothing seeks backward through existing data; we write new metadata
//! forward at seal time).
//!
//! ## Crash-safety / compatibility contract
//!
//! * **Absent → fall back to replay.** A segment sealed by a build predating
//!   this change, or one whose sidecar write was interrupted by a crash
//!   between the data fsync and the sidecar write, simply has no `.meta`.
//!   [`read_blocking`] returns `None`; the caller replays as before. No
//!   migration, no rejection of old segments.
//! * **Corrupt/torn → fall back to replay.** The sidecar carries its own
//!   CRC32 over its body. A bad magic, unknown version,
//!   short read, or CRC mismatch all return `None` (NOT a hard error) — a
//!   corrupt sidecar is indistinguishable in consequence from a missing one:
//!   we never trust bad data, and never abort a good `.wal` over a bad `.meta`.
//! * **Write order.** The sidecar is written and fsync'd only AFTER the
//!   segment's own data is durably fsync'd (see `SegmentSet::seal_and_rotate`).
//!   The write itself is atomic-rename (`.meta.tmp` → `.meta`) so a crash
//!   mid-write can leave at most a stray `.meta.tmp` (ignored on open), never a
//!   torn `.meta`.
//!
//! The sidecar is a cache of a fact already fully recoverable from the `.wal`;
//! it is never the source of truth. Deleting every `.meta` on disk changes
//! nothing but startup speed.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::wal_segment::wal_io_error;
use shamir_storage::error::DbResult;

#[cfg(test)]
use std::sync::atomic::Ordering;

/// Sidecar magic — distinct from `WAL2` so a sidecar can never be mistaken for
/// a data frame and vice versa.
/// WMT3 carries pin state. Valid WMT2 still marks an immutable seal but
/// cannot authorize deletion without scanning its WAL for unversioned frames.
const META_MAGIC: [u8; 4] = *b"WMT3";
const LEGACY_MAGIC: [u8; 4] = *b"WMT2";

/// Sidecar format version. Bump only on an incompatible layout change; an
/// unknown version reads as "absent" (fall back to replay), never a hard error.
const META_VERSION: u8 = 3;

/// On-disk layout: `[magic:4][version:1][max_version:8 LE][pin:1][crc32:4 LE]`.
pub(crate) const META_BODY_LEN: usize = 4 + 1 + 8 + 1;
pub(crate) const META_TOTAL_LEN: usize = META_BODY_LEN + 4;

/// Derive the sidecar path (`NNNNNNNN.meta`) for a segment file
/// (`NNNNNNNN.wal`). Same stem, `.meta` extension.
pub(crate) fn meta_path_for(seg_path: &Path) -> PathBuf {
    seg_path.with_extension("meta")
}

/// Derive the temp path used for the atomic-rename write of a sidecar.
fn meta_tmp_path_for(seg_path: &Path) -> PathBuf {
    seg_path.with_extension("meta.tmp")
}

/// Serialise the sidecar body + CRC into a fixed-size buffer.
pub(crate) fn encode(max_version: u64, pinned: bool) -> [u8; META_TOTAL_LEN] {
    let mut buf = [0u8; META_TOTAL_LEN];
    buf[0..4].copy_from_slice(&META_MAGIC);
    buf[4] = META_VERSION;
    buf[5..13].copy_from_slice(&max_version.to_le_bytes());
    buf[13] = u8::from(pinned);
    let crc = crc32fast::hash(&buf[0..META_BODY_LEN]);
    buf[14..18].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// Write the sidecar for `seg_path` recording `max_version` and the pin, atomically and
/// durably. Writes `NNNNNNNN.meta.tmp`, fsyncs it, then renames it over
/// `NNNNNNNN.meta` (an atomic FS op). MUST be called only AFTER the segment's
/// own data has been fsync'd, so the sidecar can never claim durability the
/// data does not have.
///
/// Blocking (file I/O); the caller wraps this in `spawn_blocking`.
pub(crate) fn write_blocking(seg_path: &Path, max_version: u64, pinned: bool) -> DbResult<()> {
    let tmp = meta_tmp_path_for(seg_path);
    let final_path = meta_path_for(seg_path);
    let bytes = encode(max_version, pinned);
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| wal_io_error(format!("segment meta open {tmp:?}"), e))?;
        f.write_all(&bytes)
            .map_err(|e| wal_io_error(format!("segment meta write {tmp:?}"), e))?;
        f.sync_all()
            .map_err(|e| wal_io_error(format!("segment meta fsync {tmp:?}"), e))?;
    }
    std::fs::rename(&tmp, &final_path)
        .map_err(|e| wal_io_error(format!("segment meta rename {tmp:?} -> {final_path:?}"), e))
}

/// Read a valid sidecar's maximum and optional pin. `None` pin identifies a
/// WMT2 seal whose contents must be scanned before truncation. Returns `None`
/// for absent or malformed sidecars; genuine I/O errors surface.
///
/// Blocking (file I/O); the caller wraps this in `spawn_blocking`.
pub(crate) fn read_blocking(seg_path: &Path) -> DbResult<Option<(u64, Option<bool>)>> {
    let path = meta_path_for(seg_path);
    let mut f = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // A torn atomic rename cannot produce this, but on Windows a
        // delete-pending sidecar can. Treat as absent → replay fallback.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(None),
        Err(e) => return Err(wal_io_error(format!("segment meta open {path:?}"), e)),
    };
    let mut buf = Vec::with_capacity(META_TOTAL_LEN + 1);
    Read::by_ref(&mut f)
        .take((META_TOTAL_LEN + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| wal_io_error(format!("segment meta read {path:?}"), e))?;
    Ok(decode(&buf))
}

/// Parse + validate sidecar bytes. `None` on any malformation (see
/// [`read_blocking`]). Split out so tests can exercise it directly.
pub(crate) fn decode(buf: &[u8]) -> Option<(u64, Option<bool>)> {
    let legacy = buf.len() == 17 && buf[0..4] == LEGACY_MAGIC && buf[4] == 2;
    if legacy {
        let crc = u32::from_le_bytes(buf[13..17].try_into().ok()?);
        if crc32fast::hash(&buf[..13]) != crc {
            return None;
        }
        return Some((u64::from_le_bytes(buf[5..13].try_into().ok()?), None));
    }
    if buf.len() != META_TOTAL_LEN || buf[0..4] != META_MAGIC || buf[4] != META_VERSION {
        return None;
    }
    let pinned = match buf[13] {
        0 => false,
        1 => true,
        _ => return None,
    };
    let crc = u32::from_le_bytes(buf[14..18].try_into().ok()?);
    if crc32fast::hash(&buf[..META_BODY_LEN]) != crc {
        return None;
    }
    Some((
        u64::from_le_bytes(buf[5..13].try_into().ok()?),
        Some(pinned),
    ))
}

/// RV-WAL-A025886E: deterministic test seam — when armed, `remove_blocking`
/// becomes a no-op (models a stubborn unlink failure, e.g. a held file).
#[cfg(test)]
static REMOVE_FAIL_FOR_TEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Arm/disarm the `remove_blocking` failure seam (tests only).
#[cfg(test)]
pub(crate) fn set_remove_fail_for_test(on: bool) {
    REMOVE_FAIL_FOR_TEST.store(on, Ordering::Release);
}

/// Best-effort removal of a segment's sidecar. A missing sidecar is fine; any
/// other error is logged and swallowed rather than treated as fatal — the
/// sidecar is only ever a cache (see the module doc's crash-safety contract),
/// so a failed removal can never corrupt data, though it CAN leave a stale
/// value for a future `open` to fall back on (whether that's harmless depends
/// on `context`; the caller supplies an accurate description). Blocking;
/// caller wraps in `spawn_blocking`.
pub(crate) fn remove_blocking(seg_path: &Path, context: &str) {
    let path = meta_path_for(seg_path);
    #[cfg(test)]
    if REMOVE_FAIL_FOR_TEST.load(Ordering::Acquire) {
        log::warn!("segment meta remove {path:?} injected failure (test seam); {context}");
        return;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!("segment meta remove {path:?} failed ({e}); {context}");
        }
    }
}
