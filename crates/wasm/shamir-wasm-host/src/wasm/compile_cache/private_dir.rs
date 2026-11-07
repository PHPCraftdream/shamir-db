use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(unix)]
use std::path::Component;
#[cfg(windows)]
use std::path::Component as WindowsComponent;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use shamir_private_dir::{create_private_dir, verify_private_dir};

#[cfg(unix)]
pub(crate) fn prepare(data_dir: &Path) -> io::Result<PathBuf> {
    let data = fs::canonicalize(data_dir)?;
    let uid = rustix::process::geteuid().as_raw();
    let metadata = fs::symlink_metadata(&data)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(permission("untrusted data directory"));
    }
    let mut ancestor = data.as_path();
    loop {
        let meta = fs::symlink_metadata(ancestor)?;
        let mode = meta.mode();
        if (meta.uid() != 0 && meta.uid() != uid) || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
            return Err(permission("untrusted data directory ancestor"));
        }
        if ancestor == Path::new("/") {
            break;
        }
        ancestor = ancestor
            .parent()
            .ok_or_else(|| permission("invalid data path"))?;
    }
    let cache = ensure_private(&data.join("cache"), uid)?;
    ensure_private(&cache.join("wasm"), uid)
}

/// Windows disk tier (DEC-3 Q5-c): the same trust contract as the Unix
/// `prepare`, enforced through `shamir-private-dir`. `data_dir` itself is
/// canonicalized and checked (real directory, not a reparse point); every
/// component below it that this code creates (`cache`, `cache/wasm`) goes
/// through `create_private_dir` when missing and `verify_private_dir`
/// when present.
#[cfg(windows)]
pub(crate) fn prepare(data_dir: &Path) -> io::Result<PathBuf> {
    let data = fs::canonicalize(data_dir)?;
    let metadata = fs::symlink_metadata(&data)?;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(permission("untrusted data directory"));
    }
    let cache = ensure_private(&data.join("cache"))?;
    ensure_private(&cache.join("wasm"))
}

/// Creates-or-verifies one private directory level on Windows and returns
/// its canonical path (rejecting any `..` component that a canonicalized
/// junction or mapped path might smuggle in).
#[cfg(windows)]
fn ensure_private(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => verify_private_dir(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_private_dir(path)?,
        Err(error) => return Err(error),
    }
    let canonical = fs::canonicalize(path)?;
    if canonical
        .components()
        .any(|part| matches!(part, WindowsComponent::ParentDir))
    {
        return Err(permission("invalid cache directory"));
    }
    Ok(canonical)
}

#[cfg(unix)]
fn ensure_private(path: &Path, uid: u32) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != uid
                || metadata.mode() & 0o077 != 0
            {
                return Err(permission("untrusted cache directory"));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(path)?;
            let metadata = fs::symlink_metadata(path)?;
            if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
                return Err(permission("cache directory permissions are not private"));
            }
        }
        Err(error) => return Err(error),
    }
    let canonical = fs::canonicalize(path)?;
    if canonical
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(permission("invalid cache directory"));
    }
    Ok(canonical)
}

fn permission(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
