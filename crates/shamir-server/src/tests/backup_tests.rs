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

// ----------------------------------------------------------------------------
// CR-C4: streaming SHA-256 must produce byte-identical digests/sizes to a
// whole-file fs::read + sha256(&contents) computation, for a file large
// enough to span many fixed-size buffer reads (a few MiB, deterministic
// content so the test is reproducible).
// ----------------------------------------------------------------------------

/// Deterministic (not random) multi-MiB content: a repeating byte pattern
/// derived from the running index, long enough to span several 1 MiB
/// streaming-buffer reads.
fn deterministic_large_content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 256) as u8).collect()
}

// ----------------------------------------------------------------------------
// F-19 (#812): backup()'s destination directory (and, since copy_dir_recursive
// now fsyncs every directory it writes into, its nested subdirectories too)
// gets its own directory-entry fsync, closing the "fsync'd the file, not the
// directory" gap for `backup()`'s plain creates (F-12/#802 only closed this
// for `restore()`'s renames). `fsync_dir` is intentionally log-only on
// failure (see its doc in `restore.rs`) and there is no test-double/mock in
// this codebase for observing an fsync syscall happen — per the brief and
// KNOWN_LIMITATIONS.md §9's own "Test scope note", the portable regression
// guard is a happy-path test proving `backup()` still succeeds (without
// erroring) with the new directory-fsync calls engaged on the return path.
// ----------------------------------------------------------------------------

#[test]
fn backup_fsyncs_dest_dir_after_success() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();

    fs::write(src.path().join("a.txt"), b"hello").unwrap();
    fs::write(src.path().join("b.bin"), vec![7u8; 2048]).unwrap();

    // No error must occur — `fsync_dir(&dest_dir)` runs on the success path,
    // after `write_manifest`, and is log-only on failure so it must never
    // cause `backup()` itself to fail.
    let report = backup(src.path(), dst.path()).expect("backup must succeed with dest_dir fsync");

    // The directory (and everything written into it) must still be exactly
    // as `copy_dir_recursive`/`write_manifest` left it — the fsync call is
    // side-effect-free from the caller's perspective.
    assert!(report.dest_dir.join("a.txt").exists());
    assert!(report.dest_dir.join("b.bin").exists());
    assert!(report.manifest_path.exists());
    assert_eq!(fs::read(report.dest_dir.join("a.txt")).unwrap(), b"hello");
}

#[test]
fn backup_fsyncs_nested_subdirectories_without_error() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();

    // A fake table dir with multiple nested levels — exercises
    // `copy_dir_recursive`'s per-recursion-level `fsync_dir(dst)` call, not
    // just the single top-level `dest_dir` fsync in `backup()` itself.
    fs::write(src.path().join("top.txt"), b"top-level").unwrap();
    let level1 = src.path().join("table_a");
    fs::create_dir_all(&level1).unwrap();
    fs::write(level1.join("segment.sst"), vec![1u8; 512]).unwrap();
    let level2 = level1.join("journal");
    fs::create_dir_all(&level2).unwrap();
    fs::write(level2.join("wal.jnl"), vec![2u8; 256]).unwrap();
    let level3 = level2.join("deeper");
    fs::create_dir_all(&level3).unwrap();
    fs::write(level3.join("leaf.bin"), vec![3u8; 128]).unwrap();

    let report = backup(src.path(), dst.path()).expect("backup with nested subdirs must succeed");

    assert!(report.dest_dir.join("top.txt").exists());
    assert!(report.dest_dir.join("table_a/segment.sst").exists());
    assert!(report.dest_dir.join("table_a/journal/wal.jnl").exists());
    assert!(report
        .dest_dir
        .join("table_a/journal/deeper/leaf.bin")
        .exists());
    assert_eq!(report.files_copied, 4);

    // The manifest must list every nested file with a root-relative,
    // forward-slash path (regression: nested-dir fsync must not disturb the
    // existing manifest-collection walk).
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&report.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.files.len(), 4);
    assert!(manifest
        .files
        .iter()
        .any(|f| f.path == "table_a/journal/deeper/leaf.bin"));
}

#[test]
fn backup_manifest_hash_matches_streamed_large_file() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();

    // ~6 MiB: several full 1 MiB buffer passes plus a partial tail read.
    let big_len = 6 * 1024 * 1024 + 12_345;
    let big_content = deterministic_large_content(big_len);
    fs::write(src.path().join("big.bin"), &big_content).unwrap();

    let expected_sha256 = hex::encode(shamir_connect::common::crypto::sha256(&big_content));

    let report = backup(src.path(), dst.path()).unwrap();
    let manifest_raw = fs::read(&report.manifest_path).unwrap();
    let manifest: Manifest = serde_json::from_slice(&manifest_raw).unwrap();

    let entry = manifest
        .files
        .iter()
        .find(|f| f.path == "big.bin")
        .expect("big.bin must be in the manifest");
    assert_eq!(
        entry.size_bytes, big_len as u64,
        "streamed size_bytes must match whole-file length"
    );
    assert_eq!(
        entry.sha256, expected_sha256,
        "streamed collect_manifest_entries digest must match whole-file sha256"
    );

    // verify_manifest() re-hashes every file via its OWN streaming path —
    // must also agree bit-for-bit with the whole-file digest.
    let verify_report = verify_manifest(&report.dest_dir).expect("snapshot must verify");
    assert_eq!(verify_report.files_checked, 1);
    assert_eq!(verify_report.total_bytes, big_len as u64);
}
