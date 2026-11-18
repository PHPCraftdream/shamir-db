//! A v2 ONLINE manifest whose `format_epoch` disagrees with the snapshot's
//! copied `format.stamp` ([`RestoreError::ManifestVerification`] wrapping
//! [`BackupError::ManifestStampEpochMismatch`]) is refused as corrupt BEFORE
//! any stage sibling is created, leaving the existing `data_dir` untouched.
//! The caller-supplied probe is bypassed for online snapshots (production
//! uses a read-only target guard), so the legacy callable here must panic if
//! ever invoked. Pure file APIs, no store and no runtime.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::format_stamp::{write_stamp, FormatStamp, CURRENT_FORMAT_EPOCH};
use crate::sync::durability::backup::offline::BackupError;
use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::online::manifest::{self, ConsistencySection, WalSection};
use crate::sync::durability::backup::restore::{restore_with_probe, RestoreError};

/// Verify-valid online snapshot whose manifest epoch deliberately differs
/// from the stamp epoch (stamp at the current epoch, manifest one ahead).
fn build_mismatched_online_snapshot(snapshot: &Path) -> u64 {
    fs::create_dir_all(snapshot).expect("snapshot dir");
    write_stamp(
        snapshot,
        &FormatStamp::with_epoch(
            CURRENT_FORMAT_EPOCH,
            "stamp-mismatch-test",
            1_700_000_000_000,
        ),
    )
    .expect("write stamp");
    fs::write(snapshot.join("snapshot.bin"), b"online-copy-marker").expect("write marker");

    let files = ["format.stamp", "snapshot.bin"]
        .into_iter()
        .map(|name| {
            let path = snapshot.join(name);
            let (sha256, size_bytes) = hash_file(&path).expect("hash");
            crate::sync::durability::backup::ManifestFileEntry {
                path: name.to_string(),
                sha256,
                size_bytes,
            }
        })
        .collect::<Vec<_>>();

    let manifest_epoch = CURRENT_FORMAT_EPOCH + 1;
    let built = manifest::build_v2(
        1,
        files,
        Some(manifest_epoch),
        Some("stamp-mismatch-test".to_string()),
        ConsistencySection {
            per_repo: std::collections::BTreeMap::new(),
            system_store_version: 0,
        },
        WalSection { dirs: Vec::new() },
    );
    manifest::write_manifest(
        snapshot,
        &manifest::encode(&built).expect("encode manifest"),
    )
    .expect("write manifest");
    manifest::write_backup_meta(snapshot).expect("write backup meta");
    manifest_epoch
}

#[test]
fn online_stamp_epoch_mismatch_refused_before_stage_probe_unused_target_untouched() {
    let root = TempDir::new().expect("temp root");
    let snapshot = root.path().join("snapshot");
    let data_dir = root.path().join("data_dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    const TARGET_BYTES: &[u8] = b"pre-restore-live-marker";
    fs::write(data_dir.join("live.bin"), TARGET_BYTES).expect("write target marker");

    let manifest_epoch = build_mismatched_online_snapshot(&snapshot);

    // Production online restore bypasses the caller's probe (read-only target
    // guard); under force=false the legacy callable must never be invoked.
    let error = restore_with_probe(&snapshot, &data_dir, false, |_| {
        panic!("legacy probe must be bypassed for a v2 online snapshot");
    })
    .expect_err("stamp/manifest epoch mismatch must refuse the restore");

    match error {
        RestoreError::ManifestVerification(BackupError::ManifestStampEpochMismatch {
            manifest_epoch: found,
            stamp_epoch,
        }) => {
            assert_eq!(found, manifest_epoch, "manifest epoch reported");
            assert_eq!(stamp_epoch, CURRENT_FORMAT_EPOCH, "stamp epoch reported");
        }
        other => panic!("expected ManifestStampEpochMismatch, got: {other:?}"),
    }

    // data_dir byte-identical; refusal decided before staging, so the parent
    // holds exactly the two fixture directories (no .restore_tmp_/.pre_restore_backup_).
    assert_eq!(
        fs::read(data_dir.join("live.bin")).expect("read target marker"),
        TARGET_BYTES,
        "existing data_dir contents must be untouched"
    );
    assert_eq!(
        fs::read_dir(root.path()).expect("read parent").count(),
        2,
        "no stage or backup sibling may be created"
    );
}

/// P1 regression (R2a 3b-2 review): a TRUNCATED (unparseable) manifest that
/// still sits in an online-looking snapshot must refuse via the fallible
/// classifier BEFORE any probe callback — previously it silently classified
/// as offline and the legacy server probe (which mutates the target on
/// refusal) ran against a corrupt source. The legacy callable here panics,
/// proving it is never invoked; the existing `data_dir` stays byte-identical
/// and no stage/backup sibling is created.
#[test]
fn truncated_manifest_refuses_before_legacy_probe_target_untouched() {
    let root = TempDir::new().expect("temp root");
    let snapshot = root.path().join("snapshot");
    let data_dir = root.path().join("data_dir");
    fs::create_dir_all(&snapshot).expect("snapshot dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    const TARGET_BYTES: &[u8] = b"pre-restore-live-marker";
    fs::write(data_dir.join("live.bin"), TARGET_BYTES).expect("write target marker");

    // A v2 online manifest cut off mid-JSON: identifiable in spirit, but
    // unparseable, hence unclassifiable — must refuse fail-closed.
    fs::write(
        snapshot.join("manifest.json"),
        "{\"format_version\":2,\"kind\":\"online\",\"created_at_unix_ns\":1",
    )
    .expect("write truncated manifest");

    let error = restore_with_probe(&snapshot, &data_dir, false, |_| {
        panic!("legacy probe must never run for an unclassifiable manifest");
    })
    .expect_err("truncated manifest must refuse the restore");

    match error {
        RestoreError::ManifestVerification(BackupError::ManifestInvalid(_)) => {}
        other => panic!("expected ManifestVerification(ManifestInvalid), got: {other:?}"),
    }

    assert_eq!(
        fs::read(data_dir.join("live.bin")).expect("read target marker"),
        TARGET_BYTES,
        "existing data_dir contents must be untouched"
    );
    assert_eq!(
        fs::read_dir(root.path()).expect("read parent").count(),
        2,
        "no stage or backup sibling may be created"
    );
}
