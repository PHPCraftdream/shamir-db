//! Staged presence re-check (restore step 3b): after the source copy, before
//! replay / invalidation / swap, every verified-manifest entry must exist as
//! a regular file under the staged copy. The copy walks the SOURCE tree, not
//! the verified list, so a deletion that raced the copy (e.g. the snapshot
//! removed mid-restore) would otherwise publish an INCOMPLETE target with no
//! error. Listing comparison only — no re-hash: the staged bytes are the
//! copy of the already-verified bytes. `symlink_metadata` is deliberately
//! non-following, so a staged symlink is refused rather than dereferenced.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::sync::durability::backup::ManifestFileEntry;

use super::RestoreError;

/// Verify that every manifest-listed file exists as a regular file under the
/// staged temp dir. First failure is typed: [`RestoreError::StagedFileMissing`]
/// for an absent / non-regular entry, [`RestoreError::ManifestVerification`]
/// for a hostile manifest-controlled path, [`RestoreError::Io`] for any other
/// metadata error (never claim "missing" without proof).
pub(super) fn verify_staged_files(
    temp_dir: &Path,
    files: &[ManifestFileEntry],
) -> Result<(), RestoreError> {
    for entry in files {
        let staged = staged_path(temp_dir, &entry.path)?;
        match fs::symlink_metadata(&staged) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(RestoreError::StagedFileMissing(staged)),
            // A vanished parent directory surfaces as NotADirectory on both
            // platforms; anything else is not proof of absence.
            Err(e) if e.kind() == ErrorKind::NotFound || e.kind() == ErrorKind::NotADirectory => {
                return Err(RestoreError::StagedFileMissing(staged))
            }
            Err(e) => return Err(RestoreError::Io(e)),
        }
    }
    Ok(())
}

/// Join a normalized manifest entry path under the staged temp dir,
/// re-deriving the safety the verifiers enforce: strict `/` split (empty and
/// `.` / `..` components refused, so absolute paths cannot slip through),
/// and on Windows additionally backslash separators, drive/UNC prefixes and
/// NTFS alternate-data-stream selectors (a `:` in a component). A literal
/// backslash is a legal Unix file name and stays allowed there.
fn staged_path(temp_dir: &Path, entry_path: &str) -> Result<PathBuf, RestoreError> {
    let refuse = || {
        RestoreError::ManifestVerification(super::super::BackupError::ManifestInvalid(format!(
            "unsafe manifest entry path: {entry_path}"
        )))
    };
    let mut staged = temp_dir.to_path_buf();
    for component in entry_path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(refuse());
        }
        if cfg!(windows) && (component.contains('\\') || component.contains(':')) {
            return Err(refuse());
        }
        staged.push(component);
    }
    Ok(staged)
}
