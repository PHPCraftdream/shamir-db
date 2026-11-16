//! Atomic stamp write: tmp, sync, rename, per-OS directory durability and
//! read-back verification (decision D4, design 2.4).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::format_stamp::codec::{decode, encode, STAMP_FILE_NAME, STAMP_TMP_NAME};
use crate::format_stamp::model::{CorruptKind, FormatStamp, StampError};

fn io_err(dir: &Path, source: std::io::Error) -> StampError {
    StampError::Io {
        dir: dir.to_path_buf(),
        source,
    }
}

fn corrupt(dir: &Path, kind: CorruptKind) -> StampError {
    StampError::Corrupt {
        dir: dir.to_path_buf(),
        kind,
    }
}

/// Unix: the rename's directory-entry update is flushed and the error is
/// propagated (strict; the stamp module does not use restore's log-only
/// `fsync_dir`).
#[cfg(unix)]
pub(crate) fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Windows: no directory flush exists; the final file is re-opened and
/// flushed best-effort (INFERRED NTFS metadata-journal push, design 2.4).
#[cfg(not(unix))]
pub(crate) fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let _ = dir;
    if let Ok(file) = fs::File::open(dir.join(STAMP_FILE_NAME)) {
        let _ = file.sync_all();
    }
    Ok(())
}

/// Write `stamp` to `<dir>/format.stamp` atomically (always under the
/// data-dir lock). A leftover tmp is removed, never reused. The written
/// stamp is read back and verified; a failure is a startup error.
pub fn write_stamp(dir: &Path, stamp: &FormatStamp) -> Result<(), StampError> {
    let frame = encode(stamp)?;
    let stamp_path = dir.join(STAMP_FILE_NAME);
    let tmp_path = dir.join(STAMP_TMP_NAME);
    match fs::remove_file(&tmp_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(dir, e)),
    }
    let mut tmp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| io_err(dir, e))?;
    tmp.write_all(&frame).map_err(|e| io_err(dir, e))?;
    tmp.sync_all().map_err(|e| io_err(dir, e))?;
    drop(tmp);
    fs::rename(&tmp_path, &stamp_path).map_err(|e| io_err(dir, e))?;
    fsync_dir(dir).map_err(|e| io_err(dir, e))?;
    verify_read_back(&stamp_path, dir, stamp.format_epoch())
}

/// D4 step 6: re-read the just-written stamp and verify checksum and epoch.
fn verify_read_back(path: &Path, dir: &Path, expected_epoch: u64) -> Result<(), StampError> {
    let bytes = fs::read(path).map_err(|e| io_err(dir, e))?;
    let decoded = decode(&bytes, dir)?;
    if decoded.format_epoch() != expected_epoch {
        return Err(corrupt(dir, CorruptKind::EpochMismatch));
    }
    Ok(())
}
