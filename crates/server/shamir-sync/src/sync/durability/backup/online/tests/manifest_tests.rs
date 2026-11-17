//! Manifest v2 codec and verification tests (CI only).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::online::manifest::{
    self, BackupKind, ConsistencySection, ManifestV2, RepoConsistency, WalDirEntry, WalSection,
    WalSegmentEntry,
};
use crate::sync::durability::backup::verify_manifest;

/// Unique temp dir per test; cleaned up best-effort.
fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "shamir_online_manifest_{}_{name}_{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    fs::write(path, bytes).expect("write");
}

fn entry(path: &Path, root: &Path) -> crate::sync::durability::backup::ManifestFileEntry {
    let (sha256, size_bytes) = hash_file(path).expect("hash");
    crate::sync::durability::backup::ManifestFileEntry {
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

fn sample_consistency() -> ConsistencySection {
    let mut per_repo = BTreeMap::new();
    per_repo.insert(
        "alpha".to_string(),
        RepoConsistency {
            wal_position: 7,
            store_dir: "repos/alpha".to_string(),
            schema_version: 1,
        },
    );
    ConsistencySection {
        per_repo,
        system_store_version: 42,
    }
}

fn sample_wal() -> WalSection {
    WalSection {
        dirs: vec![WalDirEntry {
            repo: "alpha".to_string(),
            dir: "repos/alpha.shamirwal".to_string(),
            segments: vec![
                WalSegmentEntry {
                    seq: 0,
                    file: "repos/alpha.shamirwal/00000000.wal".to_string(),
                    sidecar: Some("repos/alpha.shamirwal/00000000.meta".to_string()),
                    sha256: "aa".repeat(32),
                    size: 16,
                },
                WalSegmentEntry {
                    seq: 1,
                    file: "repos/alpha.shamirwal/00000001.wal".to_string(),
                    sidecar: None,
                    sha256: "bb".repeat(32),
                    size: 16,
                },
            ],
        }],
    }
}

#[test]
fn v1_manifest_decodes_with_offline_kind() {
    let raw = r#"{
  "format_version": 1,
  "created_at_unix_ns": 1,
  "files": []
}"#;
    let decoded = manifest::decode(raw).expect("decode v1");
    assert!(matches!(decoded.kind, BackupKind::Offline));
    assert!(decoded.consistency.is_none());
    assert!(decoded.wal.is_none());
}

#[test]
fn v2_round_trip_preserves_fields_and_unknown_keys() {
    let raw = r#"{
  "format_version": 2,
  "kind": "online",
  "created_at_unix_ns": 5,
  "future_field": { "x": [1, 2, 3] },
  "files": []
}"#;
    let decoded = manifest::decode(raw).expect("decode v2");
    assert!(matches!(decoded.kind, BackupKind::Online));
    assert_eq!(
        decoded.extra.0.get("future_field"),
        Some(&serde_json::json!({ "x": [1, 2, 3] }))
    );
    assert!(decoded.extra.0.contains_key("future_field"));

    let manifest_v2 = ManifestV2 {
        format_version: 2,
        kind: BackupKind::Online,
        created_at_unix_ns: 5,
        format_epoch: None,
        binary_version: None,
        consistency: Some(sample_consistency()),
        wal: Some(sample_wal()),
        files: Vec::new(),
        extra: decoded.extra.clone(),
    };
    let encoded = manifest::encode(&manifest_v2).expect("encode");
    assert!(encoded.contains("\"future_field\""));
    let redecoded = manifest::decode(&encoded).expect("redecode");
    assert!(redecoded.extra.0.contains_key("future_field"));
    assert!(matches!(redecoded.kind, BackupKind::Online));
}

#[test]
fn unsupported_version_is_refused() {
    let raw = r#"{ "format_version": 3, "created_at_unix_ns": 1, "files": [] }"#;
    assert!(manifest::decode(raw).is_err());
}

#[test]
fn encode_is_deterministic() {
    let first = manifest::encode(&manifest::build_v2(
        9,
        Vec::new(),
        None,
        None,
        sample_consistency(),
        sample_wal(),
    ))
    .expect("encode");
    let second = manifest::encode(&manifest::build_v2(
        9,
        Vec::new(),
        None,
        None,
        sample_consistency(),
        sample_wal(),
    ))
    .expect("encode");
    assert_eq!(first, second);
    assert!(first.contains("\"format_version\": 2"));
}

/// Build a small real snapshot on disk with a consistent manifest.
fn build_snapshot(dir: &Path) {
    let store = dir.join("repos/alpha/store.bin");
    write_file(&store, b"store-bytes");
    let wal0 = dir.join("repos/alpha.shamirwal/00000000.wal");
    let wal0_meta = dir.join("repos/alpha.shamirwal/00000000.meta");
    let wal1 = dir.join("repos/alpha.shamirwal/00000001.wal");
    write_file(&wal0, b"wal-segment-0");
    write_file(&wal0_meta, b"meta-0");
    write_file(&wal1, b"wal-segment-1");

    let files = vec![
        entry(&store, dir),
        entry(&wal0, dir),
        entry(&wal0_meta, dir),
        entry(&wal1, dir),
    ];
    let mut per_repo = BTreeMap::new();
    per_repo.insert(
        "alpha".to_string(),
        RepoConsistency {
            wal_position: 2,
            store_dir: "repos/alpha".to_string(),
            schema_version: 1,
        },
    );
    let consistency = ConsistencySection {
        per_repo,
        system_store_version: 3,
    };
    let wal0_entry = crate::sync::durability::backup::ManifestFileEntry {
        path: "repos/alpha.shamirwal/00000000.wal".to_string(),
        sha256: files[1].sha256.clone(),
        size_bytes: files[1].size_bytes,
    };
    let wal1_entry = crate::sync::durability::backup::ManifestFileEntry {
        path: "repos/alpha.shamirwal/00000001.wal".to_string(),
        sha256: files[3].sha256.clone(),
        size_bytes: files[3].size_bytes,
    };
    let wal = WalSection {
        dirs: vec![WalDirEntry {
            repo: "alpha".to_string(),
            dir: "repos/alpha.shamirwal".to_string(),
            segments: vec![
                WalSegmentEntry {
                    seq: 0,
                    file: wal0_entry.path.clone(),
                    sidecar: Some("repos/alpha.shamirwal/00000000.meta".to_string()),
                    sha256: wal0_entry.sha256.clone(),
                    size: wal0_entry.size_bytes,
                },
                WalSegmentEntry {
                    seq: 1,
                    file: wal1_entry.path.clone(),
                    sidecar: None,
                    sha256: wal1_entry.sha256.clone(),
                    size: wal1_entry.size_bytes,
                },
            ],
        }],
    };
    let built = manifest::build_v2(11, files, None, None, consistency, wal);
    let encoded = manifest::encode(&built).expect("encode");
    fs::write(dir.join("manifest.json"), encoded).expect("write manifest");
    manifest::write_backup_meta(dir).expect("write meta");
}

#[test]
fn v2_snapshot_verifies_and_v1_still_verifies() {
    let dir = temp_dir("verify_v2");
    build_snapshot(&dir);
    assert!(verify_manifest(&dir).is_ok(), "v2 snapshot must verify");
    let _ = fs::remove_dir_all(&dir);

    let dir = temp_dir("verify_v1");
    let file = dir.join("data.bin");
    write_file(&file, b"v1-data");
    let raw = format!(
        r#"{{
  "format_version": 1,
  "created_at_unix_ns": 1,
  "files": [
    {{
      "path": "data.bin",
      "sha256": "{}",
      "size_bytes": {}
    }}
  ]
}}"#,
        hash_file(&file).expect("hash").0,
        file.metadata().expect("meta").len()
    );
    fs::write(dir.join("manifest.json"), raw).expect("write v1 manifest");
    assert!(
        verify_manifest(&dir).is_ok(),
        "v1 snapshot must still verify"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn v2_verify_refusals() {
    // Checksum mismatch.
    let dir = temp_dir("verify_corrupt");
    build_snapshot(&dir);
    fs::write(dir.join("repos/alpha/store.bin"), b"tampered").expect("tamper");
    assert!(verify_manifest(&dir).is_err());
    let _ = fs::remove_dir_all(&dir);

    // WAL seq gap.
    let dir = temp_dir("verify_gap");
    build_snapshot(&dir);
    let raw = fs::read_to_string(dir.join("manifest.json")).expect("read");
    let raw = raw.replace("\"seq\": 1", "\"seq\": 7");
    fs::write(dir.join("manifest.json"), raw).expect("rewrite");
    let error = verify_manifest(&dir).expect_err("gap must fail");
    assert!(
        format!("{error}").contains("wal sequence gap"),
        "unexpected error: {error}"
    );
    let _ = fs::remove_dir_all(&dir);

    // Unmanifested extra file.
    let dir = temp_dir("verify_extra");
    build_snapshot(&dir);
    write_file(&dir.join("repos/alpha/extra.bin"), b"extra");
    assert!(verify_manifest(&dir).is_err());
    let _ = fs::remove_dir_all(&dir);

    // Unsafe path (`..`).
    let dir = temp_dir("verify_unsafe");
    let raw = r#"{
  "format_version": 2,
  "kind": "offline",
  "created_at_unix_ns": 1,
  "files": [
    {
      "path": "../escape.bin",
      "sha256": "00",
      "size_bytes": 1
    }
  ]
}"#;
    fs::write(dir.join("manifest.json"), raw).expect("write");
    let error = verify_manifest(&dir).expect_err("unsafe path must fail");
    assert!(
        format!("{error}").contains("unsafe manifest entry path"),
        "unexpected error: {error}"
    );
    let _ = fs::remove_dir_all(&dir);

    // Duplicate entry.
    let dir = temp_dir("verify_dup");
    let file = dir.join("data.bin");
    write_file(&file, b"dup");
    let (sha256, size_bytes) = hash_file(&file).expect("hash");
    let raw = format!(
        r#"{{
  "format_version": 2,
  "kind": "offline",
  "created_at_unix_ns": 1,
  "files": [
    {{ "path": "data.bin", "sha256": "{sha256}", "size_bytes": {size_bytes} }},
    {{ "path": "data.bin", "sha256": "{sha256}", "size_bytes": {size_bytes} }}
  ]
}}"#
    );
    fs::write(dir.join("manifest.json"), raw).expect("write");
    let error = verify_manifest(&dir).expect_err("duplicate must fail");
    assert!(
        format!("{error}").contains("duplicate manifest entry"),
        "unexpected error: {error}"
    );
    let _ = fs::remove_dir_all(&dir);
}
