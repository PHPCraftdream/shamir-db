//! Recreation of Fjall `lock` artifacts inside the STAGED restore copy.
//!
//! The online producer omits every Fjall file named `lock`
//! (`online/copy_engine.rs` `SKIP_FILE_NAMES`), but pinned Fjall recovery
//! (`LockedFileGuard::try_acquire`) opens the existing `lock` read+write
//! WITHOUT create, so a staged store without the artifact cannot open.
//! This module recreates it BEFORE any staged backend is opened, only in
//! directories fingerprinted as genuine Fjall stores: a `version` marker
//! starting with `FJL\x03` (fjall `file::MAGIC_BYTES`, V3) AND a
//! `keyspaces` subdirectory. A store's subtree is skipped once
//! fingerprinted — a Fjall store never nests another one — and the whole
//! traversal is iterative, symlink-refusing, and bounded to `stage`.
//!
//! The artifact is created empty with `create_new` (atomic winner; the
//! `AlreadyExists` loser validates the existing file as ordinary instead).
//! It is never created or validated through an open backend: the caller
//! runs this before any staged Fjall open, so no live holder can race.

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;

use super::RestoreError;

/// `fjall::file::MAGIC_BYTES` (`b"FJL"`) + `FormatVersion::V3`.
const FJALL_V3_MAGIC: [u8; 4] = [b'F', b'J', b'L', 3];
const FJALL_LOCK_FILE: &str = "lock";
const FJALL_KEYSPACES_DIR: &str = "keyspaces";
const FJALL_VERSION_FILE: &str = "version";

/// Recreate the empty `lock` artifact in every Fjall store under `stage`.
pub(super) fn recreate_staged_fjall_locks(stage: &Path) -> Result<(), RestoreError> {
    let mut stack = vec![stage.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if is_fjall_store_dir(&dir)? {
            // A Fjall store never nests another one; skip its subtree.
            ensure_lock_file(&dir)?;
            continue;
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                return Err(RestoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "staged restore tree must contain ordinary files only; \
                         symlinked entry: {}",
                        entry.path().display()
                    ),
                )));
            }
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Ok(())
}

/// Fingerprint check: `version` header `FJL\x03` (read bounded to the
/// 4 magic bytes, never the whole file) AND a `keyspaces` directory.
pub(super) fn is_fjall_store_dir(dir: &Path) -> Result<bool, RestoreError> {
    if !dir.join(FJALL_KEYSPACES_DIR).is_dir() {
        return Ok(false);
    }
    let mut file = match fs::File::open(dir.join(FJALL_VERSION_FILE)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(RestoreError::Io(e)),
    };
    let mut header = [0u8; 4];
    match file.read_exact(&mut header) {
        Ok(()) => Ok(header == FJALL_V3_MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(RestoreError::Io(e)),
    }
}

/// Require that `dir` is a genuine Fjall store (fingerprint above), so a
/// normal boot cannot silently initialize a fake empty directory as a
/// store. Typed refusal naming `role` otherwise.
pub(super) fn require_fjall_store(dir: &Path, role: &str) -> Result<(), RestoreError> {
    if !dir.is_dir() {
        return Err(RestoreError::Consistency {
            repo: role.to_string(),
            reason: format!(
                "expected staged Fjall store directory is missing: {}",
                dir.display()
            ),
        });
    }
    if !is_fjall_store_dir(dir)? {
        return Err(RestoreError::Consistency {
            repo: role.to_string(),
            reason: format!(
                "staged directory {} is not a genuine Fjall store \
                 (missing FJL v3 version marker or keyspaces directory)",
                dir.display()
            ),
        });
    }
    Ok(())
}

fn ensure_lock_file(dir: &Path) -> Result<(), RestoreError> {
    let lock_path = dir.join(FJALL_LOCK_FILE);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(&lock_path)?;
            if meta.is_symlink() || !meta.is_file() {
                return Err(RestoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "staged fjall lock artifact is not an ordinary file: {}",
                        lock_path.display()
                    ),
                )));
            }
            Ok(())
        }
        Err(e) => Err(RestoreError::Io(e)),
    }
}
