//! Regression for the restore staged presence re-check
//! (`restore/staged_listing.rs`): a manifest-listed file that disappears
//! from the snapshot between verification and the copy step must refuse the
//! restore with the typed [`RestoreError::StagedFileMissing`] naming that
//! file, leaving the target `data_dir` untouched and NO stage or
//! pre-restore-backup sibling behind. The managed variant also proves the
//! cross-process read lease: retention's non-blocking exclusive attempt is
//! refused while restore holds the snapshot, and free again right after the
//! typed refusal. Unmanaged and managed snapshots alike need no database
//! handle: the refusal happens right after the copy, before any store is
//! opened. Pure file APIs, no runtime.
//!
//! The seam: `restore_impl`'s `before_copy` hook runs exactly where a
//! concurrent deleter would strike — after the snapshot verified clean and
//! before the source walk — deterministically reproducing the race that in
//! production is a scheduled retention pass (or any other deleter) removing
//! the snapshot mid-restore.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::sync::durability::backup::offline::{Manifest, ManifestFileEntry};
use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::restore::{restore_impl, RestoreError};
use crate::sync::durability::backup::snapshot_lease;

const LISTED_REL: &str = "users/keeper.bin";
const TARGET_BYTES: &[u8] = b"pre-restore-live-marker";

/// Minimal VALID v1 offline snapshot: one file under a directory, manifest
/// listing exactly that file (verify also refuses any unmanifested file, so
/// the set must match exactly).
fn build_v1_snapshot(snapshot: &Path) {
    fs::create_dir_all(snapshot.join("users")).expect("snapshot users dir");
    fs::write(snapshot.join(LISTED_REL), b"kept-bytes").expect("write listed file");

    let (sha256, size_bytes) = hash_file(&snapshot.join(LISTED_REL)).expect("hash listed file");
    let manifest = Manifest {
        format_version: 1,
        created_at_unix_ns: 1_700_000_000_000,
        files: vec![ManifestFileEntry {
            path: LISTED_REL.to_string(),
            sha256,
            size_bytes,
        }],
    };
    let json = serde_json::to_vec_pretty(&manifest).expect("encode manifest");
    fs::write(snapshot.join("manifest.json"), json).expect("write manifest");
}

/// The typed refusal must name the missing manifest-listed file.
fn assert_staged_file_missing(err: RestoreError) {
    match err {
        RestoreError::StagedFileMissing(path) => {
            assert!(
                path.ends_with(Path::new(LISTED_REL)),
                "the typed error must name the missing manifest-listed file, got {path:?}"
            );
        }
        other => panic!("expected RestoreError::StagedFileMissing, got: {other:?}"),
    }
}

/// After a refusal that never reached the swap: target sentinel intact, no
/// staged temp dir and no pre-restore backup sibling left behind.
fn assert_target_and_siblings_clean(root: &Path, data_dir: &Path) {
    assert_eq!(
        fs::read(data_dir.join("live.bin")).expect("read target sentinel"),
        TARGET_BYTES,
        "data_dir must be untouched by the refused restore"
    );
    let siblings: Vec<String> = fs::read_dir(root)
        .expect("read restore parent")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        siblings
            .iter()
            .all(|name| !name.contains("restore_tmp") && !name.contains("pre_restore_backup")),
        "no stage or backup siblings may remain after the refusal, found: {siblings:?}"
    );
}

#[test]
fn missing_staged_file_refuses_typed_leaves_target_and_siblings_clean() {
    let root = TempDir::new().expect("temp root");
    let snapshot = root.path().join("snapshot");
    let data_dir = root.path().join("data_dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::write(data_dir.join("live.bin"), TARGET_BYTES).expect("write target sentinel");

    build_v1_snapshot(&snapshot);

    // No `--force`: the legacy probe must run and pass (target is stopped —
    // a plain `Ok` probe stands in for the server's liveness wrapper).
    let err = restore_impl(
        &snapshot,
        &data_dir,
        false,
        |_data_dir| Ok(()),
        || {
            // The seam: verification and the stamp gate have already passed;
            // delete the listed file's directory from the snapshot before
            // the copy walk enumerates it.
            fs::remove_dir_all(snapshot.join("users")).expect("remove listed dir");
        },
    )
    .expect_err("restore must refuse when a manifest-listed file is missing from staging");

    assert_staged_file_missing(err);
    assert_target_and_siblings_clean(root.path(), &data_dir);
}

#[test]
fn managed_snapshot_lease_blocks_retention_until_the_refusal_releases_it() {
    let root = TempDir::new().expect("temp root");
    let managed = root.path().join("managed");
    // A managed root holds timestamp-named snapshots; provision forces the
    // root detection even without the retention boot path.
    let snapshot = managed.join("20200101_000001");
    let data_dir = root.path().join("data_dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::write(data_dir.join("live.bin"), TARGET_BYTES).expect("write target sentinel");

    build_v1_snapshot(&snapshot);
    snapshot_lease::provision(&managed).expect("provision managed root");

    let err = restore_impl(
        &snapshot,
        &data_dir,
        false,
        |_data_dir| Ok(()),
        || {
            // While restore holds the shared source lease, retention's
            // NON-BLOCKING exclusive attempt must refuse (Ok(None) =
            // contention, not an error).
            assert!(
                matches!(snapshot_lease::try_exclusive(&managed), Ok(None)),
                "try_exclusive must refuse while restore holds the shared lease"
            );
            fs::remove_dir_all(snapshot.join("users")).expect("remove listed dir");
        },
    )
    .expect_err("restore must refuse when a manifest-listed file is missing from staging");

    // The typed refusal dropped the source lease: the next retention pass
    // acquires the exclusive lease and may delete the snapshot.
    let exclusive = snapshot_lease::try_exclusive(&managed)
        .expect("try_exclusive must not error after the refusal")
        .expect("lease must be free once the refusal dropped the source lease");
    drop(exclusive);

    assert_staged_file_missing(err);
    assert_target_and_siblings_clean(root.path(), &data_dir);
}
