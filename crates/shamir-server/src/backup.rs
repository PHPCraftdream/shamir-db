//! On-demand backup of the server's `data_dir` to a snapshot directory.
//!
//! v1: stop-and-copy. Operator stops the server, runs
//! `shamir-server backup --from <data_dir> --to <dest>` which recursively
//! copies every file under `data_dir` (redb files + JSON registries +
//! audit log + rotated audit files + TLS PEMs). The destination is
//! `<dest>/<timestamp>/` so a single `--to` can collect daily snapshots.
//!
//! Why stop-and-copy instead of redb's online `Database::backup()`:
//! redb 3.x doesn't expose that API on `Database` directly — copying the
//! file while the writer is paused (i.e. server stopped) is the simplest
//! path. redb's per-page CRC32 + atomic-commit design also means a copy
//! taken during a quiescent window between commits is recoverable as the
//! pre-commit state on the next open.
//!
//! Future enhancement (P2): in-process snapshot via `system_store`'s
//! existing redb handle so an `admin` op can trigger a backup without
//! downtime. Not done here because it requires a wire-protocol change.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup io: {0}")]
    Io(#[from] std::io::Error),
    #[error("source dir does not exist: {0}")]
    SourceMissing(PathBuf),
    #[error("source is not a directory: {0}")]
    SourceNotDir(PathBuf),
}

/// Outcome of a successful backup.
#[derive(Debug)]
pub struct BackupReport {
    /// The actual destination directory created (`<to>/<timestamp>`).
    pub dest_dir: PathBuf,
    /// Total bytes copied.
    pub bytes_copied: u64,
    /// Number of files copied.
    pub files_copied: u64,
}

/// Recursively copy `from` → `to/<timestamp>/`. The timestamp is
/// `YYYYMMDD_HHMMSS` in UTC (sortable, human-readable).
///
/// `to` is created if missing. The timestamped subdirectory MUST NOT
/// already exist (we refuse to overwrite — operator should pick a new
/// `--to`).
pub fn backup(from: &Path, to: &Path) -> Result<BackupReport, BackupError> {
    if !from.exists() {
        return Err(BackupError::SourceMissing(from.to_path_buf()));
    }
    if !from.is_dir() {
        return Err(BackupError::SourceNotDir(from.to_path_buf()));
    }

    let stamp = utc_timestamp();
    let dest_dir = to.join(stamp);
    if dest_dir.exists() {
        return Err(BackupError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", dest_dir.display()),
        )));
    }
    fs::create_dir_all(&dest_dir)?;

    let mut bytes = 0u64;
    let mut files = 0u64;
    copy_dir_recursive(from, &dest_dir, &mut bytes, &mut files)?;

    Ok(BackupReport {
        dest_dir,
        bytes_copied: bytes,
        files_copied: files,
    })
}

/// `YYYYMMDD_HHMMSS` UTC. Pure-Rust without `chrono` to keep deps light.
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

/// Convert a unix epoch second into (year, month, day, hour, minute, second)
/// in UTC. Algorithm: civil_from_days from Howard Hinnant's date library
/// (public domain). Adapted to Rust ints.
fn unix_to_ymd_hms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let s = (secs % 86_400) as u32;
    let h = s / 3600;
    let mi = (s % 3600) / 60;
    let se = s % 60;

    // Hinnant's civil_from_days, valid for any day in [-12687428, 11248737].
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (if mo <= 2 { y + 1 } else { y }) as i32;
    (y, mo, d, h, mi, se)
}

fn copy_dir_recursive(
    src: &Path,
    dst: &Path,
    bytes: &mut u64,
    files: &mut u64,
) -> std::io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            fs::create_dir_all(&target)?;
            copy_dir_recursive(&path, &target, bytes, files)?;
        } else if ft.is_file() {
            let n = fs::copy(&path, &target)?;
            *bytes += n;
            *files += 1;
        } else {
            // Symlinks / sockets / fifos: skip with a warning. A redb file
            // shouldn't normally be one of these.
            tracing::warn!(path = %path.display(), "backup: skipping non-regular file");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn backup_copies_all_files() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();

        // Populate src with a few files + one nested dir.
        fs::write(src.path().join("a.txt"), b"hello").unwrap();
        fs::write(src.path().join("b.bin"), vec![0u8; 1024]).unwrap();
        let nested = src.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("c.txt"), b"world").unwrap();

        let report = backup(src.path(), dst.path()).unwrap();
        assert_eq!(report.files_copied, 3);
        assert_eq!(report.bytes_copied, 5 + 1024 + 5);
        assert!(report.dest_dir.starts_with(dst.path()));
        assert!(report.dest_dir.join("a.txt").exists());
        assert!(report.dest_dir.join("b.bin").exists());
        assert!(report.dest_dir.join("nested/c.txt").exists());
    }

    #[test]
    fn backup_refuses_existing_dest() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        // Pre-create the timestamped dir to force collision.
        let stamp = utc_timestamp();
        fs::create_dir_all(dst.path().join(stamp)).unwrap();
        let err = backup(src.path(), dst.path()).unwrap_err();
        assert!(matches!(err, BackupError::Io(_)));
    }

    #[test]
    fn backup_refuses_missing_source() {
        let dst = TempDir::new().unwrap();
        let err = backup(Path::new("/nonexistent/path/123"), dst.path()).unwrap_err();
        assert!(matches!(err, BackupError::SourceMissing(_)));
    }

    #[test]
    fn timestamp_is_sortable() {
        // Two stamps generated 1 second apart should sort correctly.
        let a = utc_timestamp();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let b = utc_timestamp();
        assert!(b > a, "timestamps must sort chronologically: {a} >= {b}");
    }
}
