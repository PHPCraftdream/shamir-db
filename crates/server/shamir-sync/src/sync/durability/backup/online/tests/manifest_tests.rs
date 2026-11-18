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
use crate::sync::durability::backup::{verify_manifest, BackupError};

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

// ---- v2 verify: report metadata, stamp cross-check, path confinement ----

/// Test-scoped temp-dir guard: removes the directory when dropped, so a
/// panicking test cannot leak its fixture.
struct DirGuard(PathBuf);

impl DirGuard {
    fn new(name: &str) -> Self {
        Self(temp_dir(name))
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for DirGuard {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

fn write_stamp(dir: &Path, epoch: u64) {
    let stamp = crate::format_stamp::FormatStamp::with_epoch(epoch, "test", 0);
    let bytes = crate::format_stamp::codec::encode(&stamp).expect("encode stamp");
    fs::write(dir.join(crate::format_stamp::STAMP_FILE_NAME), bytes).expect("write stamp");
}

/// Hash `rel` (relative, forward-slash) into the manifest `files` array so
/// the on-disk file stays accounted for.
fn add_file_entry(dir: &Path, rel: &str) {
    let abs = dir.join(rel);
    let (sha256, size_bytes) = hash_file(&abs).expect("hash");
    set_manifest_fields(dir, |decoded| {
        decoded
            .files
            .push(crate::sync::durability::backup::ManifestFileEntry {
                path: rel.to_string(),
                sha256,
                size_bytes,
            })
    });
}

fn set_manifest_fields(dir: &Path, edit: impl FnOnce(&mut ManifestV2)) {
    let raw = fs::read_to_string(dir.join("manifest.json")).expect("read manifest");
    let mut decoded = manifest::decode(&raw).expect("decode manifest");
    edit(&mut decoded);
    fs::write(
        dir.join("manifest.json"),
        manifest::encode(&decoded).expect("encode"),
    )
    .expect("write manifest");
}

fn wal_segment_paths() -> [&'static str; 2] {
    ["repos/alpha/store.bin", "backup.meta.json"]
}

#[test]
fn v2_verify_report_carries_kind_epoch_version_and_positions() {
    let dir = DirGuard::new("verify_report");
    build_snapshot(&dir.0);
    write_stamp(&dir.0, 3);
    add_file_entry(&dir.0, "format.stamp");
    set_manifest_fields(&dir.0, |decoded| {
        decoded.format_epoch = Some(3);
        decoded.binary_version = Some("9.9.9".to_string());
    });
    let report = verify_manifest(&dir.0).expect("matching epoch must verify");
    assert!(matches!(report.kind, BackupKind::Online));
    assert_eq!(report.format_epoch, Some(3));
    assert_eq!(report.binary_version.as_deref(), Some("9.9.9"));
    let consistency = report.consistency.as_ref().expect("consistency");
    let alpha = consistency.per_repo.get("alpha").expect("alpha");
    assert_eq!(alpha.wal_position, 2);
    assert_eq!(alpha.store_dir, "repos/alpha");
    assert_eq!(alpha.schema_version, 1);
    assert_eq!(consistency.system_store_version, 3);
    assert_eq!(report.files_checked, 5);
}

#[test]
fn v1_verify_report_is_offline_with_no_v2_fields() {
    let dir = DirGuard::new("verify_report_v1");
    let file = dir.0.join("data.bin");
    write_file(&file, b"v1-report");
    let (sha256, size_bytes) = hash_file(&file).expect("hash");
    let raw = format!(
        r#"{{ "format_version": 1, "created_at_unix_ns": 1, "files": [
            {{ "path": "data.bin", "sha256": "{sha256}", "size_bytes": {size_bytes} }} ] }}"#
    );
    fs::write(dir.0.join("manifest.json"), raw).expect("write v1 manifest");
    let report = verify_manifest(&dir.0).expect("v1 must verify");
    assert!(matches!(report.kind, BackupKind::Offline));
    assert!(report.format_epoch.is_none());
    assert!(report.binary_version.is_none());
    assert!(report.consistency.is_none());
}

#[test]
fn v2_verify_refuses_manifest_stamp_epoch_mismatch() {
    let dir = DirGuard::new("verify_epoch_mismatch");
    build_snapshot(&dir.0);
    write_stamp(&dir.0, 4);
    add_file_entry(&dir.0, "format.stamp");
    set_manifest_fields(&dir.0, |decoded| decoded.format_epoch = Some(3));
    let error = verify_manifest(&dir.0).expect_err("mismatched epoch must fail");
    assert!(
        matches!(
            error,
            BackupError::ManifestStampEpochMismatch {
                manifest_epoch: 3,
                stamp_epoch: 4
            }
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_missing_epoch_with_valid_stamp() {
    // A valid snapshot stamp with an absent manifest `format_epoch` refuses
    // typed: a manifest/stamp pair that cannot be proven consistent is
    // corrupt (the producer wires the epoch into `build_v2`).
    let dir = DirGuard::new("verify_epoch_absent");
    build_snapshot(&dir.0);
    write_stamp(&dir.0, 3);
    add_file_entry(&dir.0, "format.stamp");
    let error = verify_manifest(&dir.0).expect_err("absent epoch with stamp must fail");
    assert!(
        matches!(error, BackupError::MissingManifestEpoch { stamp_epoch: 3 }),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_corrupt_stamp() {
    let dir = DirGuard::new("verify_stamp_corrupt");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| decoded.format_epoch = Some(3));
    fs::write(
        dir.0.join(crate::format_stamp::STAMP_FILE_NAME),
        b"not-a-stamp",
    )
    .expect("corrupt stamp");
    let error = verify_manifest(&dir.0).expect_err("corrupt stamp must fail");
    assert!(
        matches!(error, BackupError::InvalidSnapshotStamp(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_tolerates_epoch_without_stamp() {
    let dir = DirGuard::new("verify_epoch_no_stamp");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| decoded.format_epoch = Some(3));
    assert!(
        verify_manifest(&dir.0).is_ok(),
        "absent stamp keeps today's rule"
    );
}

#[test]
fn v2_verify_refuses_noncanonical_wal_segment_path() {
    // A segment file that is not the canonical direct
    // `<wal-dir>/<seq:08>.wal` name refuses typed — whether it points
    // outside the declared dir or merely renames the segment.
    for seg in wal_segment_paths() {
        let dir = DirGuard::new("verify_wal_outside");
        build_snapshot(&dir.0);
        set_manifest_fields(&dir.0, |decoded| {
            let wal = decoded.wal.as_mut().expect("wal");
            wal.dirs[0].segments[0].file = seg.to_string();
        });
        let error = verify_manifest(&dir.0).expect_err("non-canonical wal name must fail");
        assert!(
            matches!(error, BackupError::NonCanonicalWalName { .. }),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn v2_verify_refuses_non_canonical_wal_segment_name() {
    let dir = DirGuard::new("verify_wal_noncanonical");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let wal = decoded.wal.as_mut().expect("wal");
        wal.dirs[0].segments[1].file = "repos/alpha.shamirwal/1.wal".to_string();
    });
    let error = verify_manifest(&dir.0).expect_err("non-canonical name must fail");
    assert!(
        matches!(error, BackupError::NonCanonicalWalName { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_undeclared_direct_wal_file() {
    // Replay reads every direct `.wal` under the declared dir; an on-disk
    // segment the listing omits must refuse.
    let dir = DirGuard::new("verify_wal_undeclared");
    build_snapshot(&dir.0);
    write_file(&dir.0.join("repos/alpha.shamirwal/00000002.wal"), b"sneaky");
    let error = verify_manifest(&dir.0).expect_err("undeclared wal file must fail");
    assert!(
        matches!(error, BackupError::UndeclaredWalFile { ref file, .. } if file == "00000002.wal"),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_wal_listing_without_segments() {
    // Even a `V*` of zero produces an empty segment file, so a wal dir
    // listing with zero segments is a truncated capture and refuses typed.
    let dir = DirGuard::new("verify_wal_empty_listing");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let wal = decoded.wal.as_mut().expect("wal");
        wal.dirs[0].segments.clear();
    });
    let error = verify_manifest(&dir.0).expect_err("empty wal listing must fail");
    assert!(
        matches!(
            error,
            BackupError::WalListingEmpty {
                ref repo,
                ref dir
            } if repo == "alpha" && dir == "repos/alpha.shamirwal"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_wal_listing_missing_for_captured_repo() {
    // Captured repos and listed wal dirs must be a bijection: a repo in
    // `consistency.per_repo` with no wal dir listing refuses typed.
    let dir = DirGuard::new("verify_wal_missing_repo");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let consistency = decoded.consistency.as_mut().expect("consistency");
        consistency.per_repo.insert(
            "beta".to_string(),
            RepoConsistency {
                wal_position: 0,
                store_dir: "repos/beta".to_string(),
                schema_version: 1,
            },
        );
    });
    let error = verify_manifest(&dir.0).expect_err("missing wal listing must fail");
    assert!(
        matches!(
            error,
            BackupError::WalListingMissingForRepo { ref repo } if repo == "beta"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_wal_listing_for_uncaptured_repo() {
    // A wal dir listing naming a repo absent from `consistency.per_repo`
    // refuses typed (bijection, other direction).
    let dir = DirGuard::new("verify_wal_uncaptured_repo");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let consistency = decoded.consistency.as_mut().expect("consistency");
        consistency.per_repo.remove("alpha");
    });
    let error = verify_manifest(&dir.0).expect_err("uncaptured wal repo must fail");
    assert!(
        matches!(
            error,
            BackupError::WalListingForUncapturedRepo { ref repo } if repo == "alpha"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_tolerates_harmless_ancillary_files_in_wal_dir() {
    // A direct undeclared file in a declared wal dir refuses ONLY when it
    // is a numeric `.wal` file. Fully hashed ancillary files
    // (`NNNNNNNN.meta.tmp`) and an orphan `NNNNNNNN.meta` the WAL treats as
    // harmless are tolerated — once accounted in the manifest `files`
    // array, which the global file-set check still enforces.
    let dir = DirGuard::new("verify_wal_ancillary");
    build_snapshot(&dir.0);
    write_file(
        &dir.0.join("repos/alpha.shamirwal/00000000.meta.tmp"),
        b"tmp",
    );
    write_file(
        &dir.0.join("repos/alpha.shamirwal/00000007.meta"),
        b"orphan",
    );
    add_file_entry(&dir.0, "repos/alpha.shamirwal/00000000.meta.tmp");
    add_file_entry(&dir.0, "repos/alpha.shamirwal/00000007.meta");
    assert!(
        verify_manifest(&dir.0).is_ok(),
        "hashed ancillary files must not refuse the wal-dir scan"
    );
}

#[test]
fn v2_verify_refuses_wal_dir_not_store_dir_shamirwal() {
    // The listed wal dir must be the canonical `<store_dir>.shamirwal`
    // sibling of the repo's `store_dir` — replay derives the WAL directory
    // exactly this way, so a deviation refuses typed at verify time. No
    // physical store directory is required for this refusal.
    let dir = DirGuard::new("verify_wal_path_mismatch");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let consistency = decoded.consistency.as_mut().expect("consistency");
        consistency
            .per_repo
            .get_mut("alpha")
            .expect("alpha")
            .store_dir = "repos/gamma".to_string();
    });
    let error = verify_manifest(&dir.0).expect_err("wal dir/store_dir mismatch must fail");
    assert!(
        matches!(
            error,
            BackupError::WalListingPathMismatch {
                ref repo,
                ref found,
                ref expected
            } if repo == "alpha"
                && found == "repos/alpha.shamirwal"
                && expected == "repos/gamma.shamirwal"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_duplicate_wal_dir() {
    let dir = DirGuard::new("verify_wal_dup_dir");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let wal = decoded.wal.as_mut().expect("wal");
        let clone = wal.dirs[0].clone();
        wal.dirs.push(clone);
    });
    let error = verify_manifest(&dir.0).expect_err("duplicate wal dir must fail");
    assert!(
        matches!(error, BackupError::DuplicateWalDir { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_unlisted_wal_file() {
    let dir = DirGuard::new("verify_wal_unlisted");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let drop_path = "repos/alpha.shamirwal/00000001.wal".to_string();
        decoded.files.retain(|entry| entry.path != drop_path);
    });
    let error = verify_manifest(&dir.0).expect_err("unlisted wal file must fail");
    assert!(
        matches!(error, BackupError::WalFileUnmanifested(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_unconfined_wal_segment_path() {
    // Windows drive letter + absolute path smuggled through a forward-slash
    // manifest: must be refused BEFORE any open (the target does not exist).
    let dir = DirGuard::new("verify_wal_unconfined");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let wal = decoded.wal.as_mut().expect("wal");
        wal.dirs[0].segments[0].file = "C:/evil/00000000.wal".to_string();
    });
    let error = verify_manifest(&dir.0).expect_err("drive-letter path must fail");
    assert!(
        matches!(error, BackupError::UnconfinedManifestPath(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_verify_refuses_unsafe_consistency_store_dir() {
    let dir = DirGuard::new("verify_store_dir");
    build_snapshot(&dir.0);
    set_manifest_fields(&dir.0, |decoded| {
        let consistency = decoded.consistency.as_mut().expect("consistency");
        consistency
            .per_repo
            .get_mut("alpha")
            .expect("alpha")
            .store_dir = "../elsewhere".to_string();
    });
    let error = verify_manifest(&dir.0).expect_err("unsafe store_dir must fail");
    assert!(
        matches!(error, BackupError::UnsafeStoreDir { ref repo, .. } if repo == "alpha"),
        "unexpected error: {error}"
    );
}
