//! Sidecar metadata for a sealed WAL segment (task #500).
//!
//! Startup used to pay O(total on-disk WAL size): [`crate::SegmentSet::open`]
//! called `replay()` on EVERY sealed segment purely to compute its
//! `max_version` (the single number that drives version-based truncation).
//! After a long downtime with many un-truncated sealed segments, opening the
//! database read + decoded gigabytes just to extract one `u64` per segment.
//!
//! This module adds a small, **purely additive, purely optional** sidecar
//! file — `NNNNNNNN.meta`, sitting next to the segment's `NNNNNNNN.wal` — that
//! records that one number at seal time. On open, [`read_blocking`] reads
//! the sidecar directly (a ~24-byte read) and skips the full replay. The
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
//!   CRC32 over (magic ‖ version ‖ max_version). A bad magic, unknown version,
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
/// RV-WAL-A025886E: WMT2. A WMT2 sidecar means "this segment is sealed and
/// its bytes never change" — `SegmentSet::open` treats the highest segment
/// carrying one as sealed. Legacy WMT1 sidecars are never trusted: they read
/// as absent and the segment falls back to replay.
const META_MAGIC: [u8; 4] = *b"WMT2";

/// Sidecar format version. Bump only on an incompatible layout change; an
/// unknown version reads as "absent" (fall back to replay), never a hard error.
const META_VERSION: u8 = 2;

/// On-disk layout: `[magic:4][version:1][max_version:8 LE][crc32:4 LE]`.
/// The CRC covers the first 13 bytes (magic ‖ version ‖ max_version).
pub(crate) const META_BODY_LEN: usize = 4 + 1 + 8;
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
pub(crate) fn encode(max_version: u64) -> [u8; META_TOTAL_LEN] {
    let mut buf = [0u8; META_TOTAL_LEN];
    buf[0..4].copy_from_slice(&META_MAGIC);
    buf[4] = META_VERSION;
    buf[5..13].copy_from_slice(&max_version.to_le_bytes());
    let crc = crc32fast::hash(&buf[0..META_BODY_LEN]);
    buf[13..17].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// Write the sidecar for `seg_path` recording `max_version`, atomically and
/// durably. Writes `NNNNNNNN.meta.tmp`, fsyncs it, then renames it over
/// `NNNNNNNN.meta` (an atomic FS op). MUST be called only AFTER the segment's
/// own data has been fsync'd, so the sidecar can never claim durability the
/// data does not have.
///
/// Blocking (file I/O); the caller wraps this in `spawn_blocking`.
pub(crate) fn write_blocking(seg_path: &Path, max_version: u64) -> DbResult<()> {
    let tmp = meta_tmp_path_for(seg_path);
    let final_path = meta_path_for(seg_path);
    let bytes = encode(max_version);
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

/// Read the sidecar for `seg_path`, returning `Some(max_version)` iff a
/// well-formed sidecar is present. Returns `None` for ANY of: absent file,
/// short read, bad magic, unknown version, or CRC mismatch (a corrupt/torn
/// sidecar is treated exactly like a missing one — the caller falls back to a
/// full replay). Only a genuine I/O error other than "not found" surfaces as
/// `Err`.
///
/// Blocking (file I/O); the caller wraps this in `spawn_blocking`.
pub(crate) fn read_blocking(seg_path: &Path) -> DbResult<Option<u64>> {
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
    f.read_to_end(&mut buf)
        .map_err(|e| wal_io_error(format!("segment meta read {path:?}"), e))?;
    Ok(decode(&buf))
}

/// Parse + validate sidecar bytes. `None` on any malformation (see
/// [`read_blocking`]). Split out so tests can exercise it directly.
pub(crate) fn decode(buf: &[u8]) -> Option<u64> {
    if buf.len() != META_TOTAL_LEN {
        return None; // truncated, empty, or trailing garbage
    }
    if buf[0..4] != META_MAGIC {
        return None;
    }
    if buf[4] != META_VERSION {
        return None;
    }
    let crc_stored = u32::from_le_bytes([buf[13], buf[14], buf[15], buf[16]]);
    if crc32fast::hash(&buf[0..META_BODY_LEN]) != crc_stored {
        return None; // torn / bit-rotted sidecar
    }
    Some(u64::from_le_bytes([
        buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11], buf[12],
    ]))
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
