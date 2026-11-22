use std::fs::File;
use std::path::Path;

/// Why an offline maintenance lock could not be taken.
///
/// Contention (a live server or command holds the lock) is reported
/// separately from every other OS failure at the two stages of
/// `shamir_sync::durability::maintenance_lock::acquire_with`:
/// opening/creating the lock file, and the `try_lock_exclusive` attempt
/// itself. Permission problems on the lock file or the attempt must never
/// be mislabeled as "a server is running".
#[derive(Debug, thiserror::Error)]
pub(super) enum LockFailure {
    /// The lock file itself could not be opened or created.
    #[error("cannot open the offline maintenance lock: {0}")]
    Open(#[from] std::io::Error),
    /// Another process holds the exclusive lock.
    #[error("offline token command requires a stopped server")]
    Contention,
    /// The lock attempt failed for a non-contention OS reason.
    #[error("the offline maintenance lock could not be taken: {0}")]
    LockOther(#[source] std::io::Error),
}

/// True only for genuine OS lock contention: exactly the raw OS error fs4
/// 0.12's `try_lock_exclusive` returns when the lock is held
/// (`fs4::lock_contended_error()`: `EWOULDBLOCK` from `flock` on Unix,
/// `ERROR_LOCK_VIOLATION` from `LockFileEx` on Windows). Same-kind errors
/// with a different raw code are NOT contention; the error-kind fallback
/// covers only synthesized errors that carry no raw OS code at all.
pub(super) fn is_lock_contention(error: &std::io::Error) -> bool {
    let contended = fs4::lock_contended_error();
    match (error.raw_os_error(), contended.raw_os_error()) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => error.kind() == std::io::ErrorKind::WouldBlock,
    }
}

/// Take the shared offline maintenance lock, classifying the failure.
/// The returned file must outlive every store handle opened under it.
pub(super) fn acquire(data_dir: &Path) -> Result<File, LockFailure> {
    shamir_sync::durability::maintenance_lock::acquire_with(data_dir, |error| {
        if is_lock_contention(&error) {
            LockFailure::Contention
        } else {
            LockFailure::LockOther(error)
        }
    })
}
