//! Kind-first retention verifier tests (reviewer P3, brief 86 item 4,
//! CI only): `verify_retention_candidate` must refuse a v2-offline manifest
//! BEFORE any payload I/O, and still accept a valid v2-online snapshot.
//!
//! Red oracle of the first test: before the fix the candidate went straight
//! to the full v2 verifier, which walked and hashed the listed tree before
//! the kind check — the missing listed file produced `BackupError::Io`
//! (NotFound) instead of the kind refusal, so the first assertion fails.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::online::manifest::{
    self as online_manifest, BackupKind, ConsistencySection, RepoConsistency, WalDirEntry,
    WalSection, WalSegmentEntry,
};
use crate::sync::durability::backup::{verify_retention_candidate, BackupError, ManifestFileEntry};

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(path, bytes).expect("write");
}

fn entry(path: &Path, root: &Path) -> ManifestFileEntry {
    let (sha256, size_bytes) = hash_file(path).expect("hash");
    ManifestFileEntry {
        path: path
            .strip_prefix(root)
            .expect("relative")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        sha256,
        size_bytes,
    }
}

/// A v2-offline manifest whose only listed file does NOT exist on disk.
/// Before the kind-first fix the verifier read the tree first and failed
/// with a NotFound I/O error; after the fix the kind refusal comes first.
#[test]
fn offline_kind_is_refused_before_the_tree_is_read() {
    let dir = TempDir::new().expect("temp dir");
    let raw = r#"{
  "format_version": 2,
  "kind": "offline",
  "created_at_unix_ns": 1,
  "files": [
    {
      "path": "missing.bin",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "size_bytes": 1
    }
  ]
}"#;
    fs::write(dir.path().join("manifest.json"), raw).expect("write manifest");
    assert!(
        !dir.path().join("missing.bin").exists(),
        "precondition: the listed payload must be absent"
    );

    let error =
        verify_retention_candidate(dir.path(), raw.as_bytes(), &|| false).expect_err("must refuse");
    assert!(
        matches!(&error, BackupError::ManifestInvalid(message)
            if message.contains("kind is not online")),
        "expected the kind refusal before any payload I/O, got: {error}"
    );
}

/// The original v2 format check is retained: v1 (and unknown) formats
/// refuse typed before any payload I/O, with no v1 fallback. The fixture
/// first proves it deserializes as a v1 ManifestV2 (kind defaults to
/// offline), so the refusal below is the version check, not a decode gap.
#[test]
fn v1_fixture_deserializes_and_still_refuses_typed() {
    let dir = TempDir::new().expect("temp dir");
    let raw = r#"{
  "format_version": 1,
  "created_at_unix_ns": 1,
  "files": []
}"#;
    let decoded = online_manifest::decode(raw).expect("v1 fixture must deserialize");
    assert_eq!(decoded.format_version, 1);
    assert!(matches!(decoded.kind, BackupKind::Offline));
    assert_eq!(decoded.created_at_unix_ns, 1);
    assert!(decoded.files.is_empty());
    assert!(decoded.consistency.is_none());
    assert!(decoded.wal.is_none());

    let error =
        verify_retention_candidate(dir.path(), raw.as_bytes(), &|| false).expect_err("must refuse");
    assert!(
        matches!(
            &error,
            BackupError::UnsupportedManifestFormatVersion { found: 1, .. }
        ),
        "expected the original typed version refusal, got: {error}"
    );
}

/// A valid v2-online snapshot is still accepted end to end (the refusal
/// above must not have widened into rejecting genuine candidates).
#[test]
fn valid_v2_online_candidate_is_accepted() {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    let store = root.join("repos/alpha/store.bin");
    let wal0 = root.join("repos/alpha.shamirwal/00000000.wal");
    let wal0_meta = root.join("repos/alpha.shamirwal/00000000.meta");
    write_file(&store, b"store-bytes");
    write_file(&wal0, b"wal-segment-0");
    write_file(&wal0_meta, b"meta-0");

    let files = vec![
        entry(&store, root),
        entry(&wal0, root),
        entry(&wal0_meta, root),
    ];
    let mut per_repo = BTreeMap::new();
    per_repo.insert(
        "alpha".to_string(),
        RepoConsistency {
            wal_position: 1,
            store_dir: "repos/alpha".to_string(),
            schema_version: 1,
        },
    );
    let consistency = ConsistencySection {
        per_repo,
        system_store_version: 3,
    };
    let wal = WalSection {
        dirs: vec![WalDirEntry {
            repo: "alpha".to_string(),
            dir: "repos/alpha.shamirwal".to_string(),
            segments: vec![WalSegmentEntry {
                seq: 0,
                file: files[1].path.clone(),
                sidecar: Some(files[2].path.clone()),
                sha256: files[1].sha256.clone(),
                size: files[1].size_bytes,
            }],
        }],
    };
    let built = online_manifest::build_v2(11, files, None, None, consistency, wal);
    let encoded = online_manifest::encode(&built).expect("encode");
    assert!(matches!(built.kind, BackupKind::Online));
    fs::write(root.join("manifest.json"), &encoded).expect("write manifest");
    online_manifest::write_backup_meta(root).expect("write meta");

    verify_retention_candidate(root, encoded.as_bytes(), &|| false)
        .expect("a valid v2-online candidate must be accepted");
}
