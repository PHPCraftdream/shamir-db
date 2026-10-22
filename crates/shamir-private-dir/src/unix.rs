//! Unix implementation: mode-`0700` real directory owned by the euid.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

/// Creates `path` (one level; the parent must exist) as a `0700` directory
/// owned by the current euid. An already existing path is accepted only if
/// [`verify_private_dir`] passes — a pre-planted wider directory is an
/// error, never silently adopted.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            verify_private_dir(path)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            verify_private_dir(path)
        }
        Err(error) => Err(error),
    }
}

/// Verifies `path` is a real directory (no symlink component at the final
/// component), owned by the current euid, with mode `& 0o077 == 0` — no
/// group or other access at all. Any violation is
/// [`io::ErrorKind::PermissionDenied`] naming what failed.
pub fn verify_private_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(denied("path is a symlink"));
    }
    if !metadata.is_dir() {
        return Err(denied("path is not a directory"));
    }
    let uid = rustix::process::geteuid().as_raw();
    if metadata.uid() != uid {
        return Err(denied("directory is owned by another user"));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(denied("directory is group or other accessible"));
    }
    Ok(())
}

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
