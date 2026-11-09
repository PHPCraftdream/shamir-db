use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use fs4::fs_std::FileExt;

/// Shared ownership boundary for server startup and offline maintenance.
/// The returned file must outlive every repository handle opened under it.
pub fn acquire(data_dir: &Path) -> io::Result<File> {
    acquire_with(data_dir, |error| error)
}

pub fn acquire_with<E: From<io::Error>>(
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
