#[cfg(unix)]
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};

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

#[cfg(not(unix))]
pub(crate) fn prepare(_data_dir: &Path) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk cache disabled on this platform",
    ))
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

#[cfg(unix)]
fn permission(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
