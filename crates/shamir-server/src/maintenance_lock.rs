use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use fs4::fs_std::FileExt;

/// Shared ownership boundary for server startup and offline maintenance.
/// The returned file must outlive every repository handle opened under it.
pub(crate) fn acquire(data_dir: &Path) -> io::Result<File> {
    acquire_with(data_dir, |error| error)
}

/// Preserve startup's distinction between file-open I/O and lock refusal.
pub(crate) fn acquire_server(data_dir: &Path) -> Result<File, crate::server::BootError> {
    acquire_with(data_dir, |_| {
        crate::server::BootError::AlreadyRunning(data_dir.to_path_buf())
    })
}

fn acquire_with<E: From<io::Error>>(
    data_dir: &Path,
    lock_error: impl FnOnce(io::Error) -> E,
) -> Result<File, E> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join(".shamir.lock"))?;
    file.try_lock_exclusive().map_err(lock_error)?;
    // Diagnostic only: failure to refresh the PID must not release ownership.
    // Never truncate before acquiring: a losing contender must not erase it.
    if file.set_len(0).is_ok() {
        let _ = write!(file, "{}", std::process::id()).and_then(|()| file.flush());
    }
    Ok(file)
}
