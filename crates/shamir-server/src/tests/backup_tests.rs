use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::backup::{
    backup, utc_timestamp, verify_manifest, BackupError, Manifest, ManifestFileEntry,
    MANIFEST_FILE_NAME, MANIFEST_FORMAT_VERSION,
};

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

// ----------------------------------------------------------------------------
// RI-11: manifest write + verify_manifest
// ----------------------------------------------------------------------------

#[test]
fn backup_writes_manifest_with_matching_hashes() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();

    fs::write(src.path().join("a.txt"), b"hello").unwrap();
    fs::write(src.path().join("b.bin"), vec![0u8; 1024]).unwrap();
    let nested = src.path().join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("c.txt"), b"world").unwrap();

    let report = backup(src.path(), dst.path()).unwrap();
    assert_eq!(
        report.manifest_path,
        report.dest_dir.join(MANIFEST_FILE_NAME)
    );
    assert!(report.manifest_path.exists());

    let raw = fs::read(&report.manifest_path).unwrap();
    let manifest: Manifest = serde_json::from_slice(&raw).unwrap();
    assert_eq!(manifest.format_version, 1);
    assert!(manifest.created_at_unix_ns > 0);
    assert_eq!(
        manifest.files.len(),
        3,
        "manifest must list every copied file"
    );

    // manifest.json itself must NOT be in its own files list.
    assert!(!manifest.files.iter().any(|f| f.path == MANIFEST_FILE_NAME));

    // Every entry's hash/size must match the actual copied file.
    for entry in &manifest.files {
        let contents = fs::read(report.dest_dir.join(&entry.path)).unwrap();
        assert_eq!(
            entry.size_bytes,
            contents.len() as u64,
            "size mismatch for {}",
            entry.path
        );
        let actual = hex::encode(shamir_connect::common::crypto::sha256(&contents));
        assert_eq!(entry.sha256, actual, "sha256 mismatch for {}", entry.path);
        // Paths must be forward-slash separated, even on Windows.
        assert!(
            !entry.path.contains('\\'),
            "path must not contain backslashes: {}",
            entry.path
        );
    }

    // Sanity: verify_manifest accepts this freshly-written, untampered snapshot.
    let report2 = verify_manifest(&report.dest_dir).expect("valid snapshot must verify");
    assert_eq!(report2.files_checked, 3);
}

#[test]
fn verify_manifest_rejects_tampered_file() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    fs::write(src.path().join("a.txt"), b"hello").unwrap();

    let report = backup(src.path(), dst.path()).unwrap();

    // Mutate one byte of the copied file post-backup.
    let target = report.dest_dir.join("a.txt");
    fs::write(&target, b"hEllo").unwrap();

    let err = verify_manifest(&report.dest_dir).unwrap_err();
    assert!(
        matches!(err, BackupError::ChecksumMismatch { .. }),
        "expected ChecksumMismatch, got {err:?}"
    );
}

#[test]
fn verify_manifest_rejects_missing_manifest() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"hello").unwrap();

    let err = verify_manifest(dir.path()).unwrap_err();
    assert!(
        matches!(err, BackupError::ManifestMissing(_)),
        "expected ManifestMissing, got {err:?}"
    );
}

#[test]
fn verify_manifest_rejects_extra_unmanifested_file() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    fs::write(src.path().join("a.txt"), b"hello").unwrap();

    let report = backup(src.path(), dst.path()).unwrap();

    // Drop an extra file into the snapshot dir that the manifest never saw.
    fs::write(report.dest_dir.join("intruder.txt"), b"surprise").unwrap();

    let err = verify_manifest(&report.dest_dir).unwrap_err();
    assert!(
        matches!(err, BackupError::UnmanifestedFile(_)),
        "expected UnmanifestedFile, got {err:?}"
    );
}

// ----------------------------------------------------------------------------
// CR-B7: manifest hardening — format_version, path traversal, duplicates,
// backup destination-inside-source.
// ----------------------------------------------------------------------------

/// Hand-write a `manifest.json` whose `files` list is exactly `entries`,
/// bypassing `write_manifest` entirely — lets tests construct manifests
/// that a real backup would never produce (path traversal, duplicates,
/// unknown format_version) to exercise `verify_manifest`'s hardening.
fn write_hand_crafted_manifest(dir: &Path, format_version: u32, files: Vec<ManifestFileEntry>) {
    let manifest = Manifest {
        format_version,
        created_at_unix_ns: 1,
        files,
    };
    let json = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(dir.join(MANIFEST_FILE_NAME), json).unwrap();
}

#[test]
fn verify_manifest_rejects_parent_dir_escape_path() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"hello").unwrap();

    write_hand_crafted_manifest(
        dir.path(),
        MANIFEST_FORMAT_VERSION,
        vec![ManifestFileEntry {
            path: "../escape.txt".to_string(),
            sha256: "0".repeat(64),
            size_bytes: 0,
        }],
    );

    let err = verify_manifest(dir.path()).unwrap_err();
    assert!(
        matches!(err, BackupError::UnsafeManifestPath(_)),
        "expected UnsafeManifestPath for a '..'-containing entry, got {err:?}"
    );
}

#[test]
fn verify_manifest_rejects_absolute_path() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"hello").unwrap();

    let absolute = if cfg!(windows) {
        "C:\\Windows\\win.ini".to_string()
    } else {
        "/etc/passwd".to_string()
    };
    write_hand_crafted_manifest(
        dir.path(),
        MANIFEST_FORMAT_VERSION,
        vec![ManifestFileEntry {
            path: absolute,
            sha256: "0".repeat(64),
            size_bytes: 0,
        }],
    );

    let err = verify_manifest(dir.path()).unwrap_err();
    assert!(
        matches!(err, BackupError::UnsafeManifestPath(_)),
        "expected UnsafeManifestPath for an absolute entry path, got {err:?}"
    );
}

#[test]
fn verify_manifest_rejects_duplicate_entries() {
    let dir = TempDir::new().unwrap();
    let contents = b"hello";
    fs::write(dir.path().join("a.txt"), contents).unwrap();
    let sha256 = hex::encode(shamir_connect::common::crypto::sha256(contents));

    write_hand_crafted_manifest(
        dir.path(),
        MANIFEST_FORMAT_VERSION,
        vec![
            ManifestFileEntry {
                path: "a.txt".to_string(),
                sha256: sha256.clone(),
                size_bytes: contents.len() as u64,
            },
            ManifestFileEntry {
                path: "a.txt".to_string(),
                sha256,
                size_bytes: contents.len() as u64,
            },
        ],
    );

    let err = verify_manifest(dir.path()).unwrap_err();
    assert!(
        matches!(err, BackupError::DuplicateManifestEntry(_)),
        "expected DuplicateManifestEntry, got {err:?}"
    );
}

#[test]
fn verify_manifest_rejects_unknown_format_version() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.txt"), b"hello").unwrap();

    write_hand_crafted_manifest(dir.path(), 999, vec![]);

    let err = verify_manifest(dir.path()).unwrap_err();
    match err {
        BackupError::UnsupportedManifestFormatVersion { found, expected } => {
            assert_eq!(found, 999);
            assert_eq!(expected, MANIFEST_FORMAT_VERSION);
        }
        other => panic!("expected UnsupportedManifestFormatVersion, got {other:?}"),
    }
}

#[test]
fn backup_rejects_destination_inside_source_subdir() {
    let src = TempDir::new().unwrap();
    fs::write(src.path().join("a.txt"), b"hello").unwrap();
    let to = src.path().join("subdir");

    let err = backup(src.path(), &to).unwrap_err();
    assert!(
        matches!(err, BackupError::DestinationInsideSource { .. }),
        "expected DestinationInsideSource for a subdir of source, got {err:?}"
    );
}

#[test]
fn backup_rejects_destination_equal_to_source() {
    let src = TempDir::new().unwrap();
    fs::write(src.path().join("a.txt"), b"hello").unwrap();

    let err = backup(src.path(), src.path()).unwrap_err();
    assert!(
        matches!(err, BackupError::DestinationInsideSource { .. }),
        "expected DestinationInsideSource when to == from, got {err:?}"
    );
}
