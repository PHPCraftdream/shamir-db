use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::backup::{backup, utc_timestamp, BackupError};

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
