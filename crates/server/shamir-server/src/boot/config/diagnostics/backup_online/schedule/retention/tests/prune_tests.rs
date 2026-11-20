use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shamir_private_dir::create_private_dir;
use tempfile::TempDir;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{
    build_v2, encode, write_backup_meta, write_manifest, ConsistencySection, RepoConsistency,
    WalDirEntry, WalSection, WalSegmentEntry, BACKUP_META_FILE_NAME,
};
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{verify_manifest, ManifestFileEntry, MANIFEST_FILE_NAME};

use super::super::budget::{Budget, Refusal, ROOT_ENTRIES};
use super::super::prune::prune_inner;
use super::super::{prune, RetentionOutcome};

const OWNER: &str = "retention-test-owner";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn private_root() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = private_root_in(temp.path());
    (temp, root)
}

fn private_root_in(parent: &Path) -> PathBuf {
    let root = parent.join("private");
    create_private_dir(&root).expect("private root");
    root.canonicalize().expect("canonical root")
}

fn copy(root: &Path, name: &str, time: u128, owner: Option<&str>) -> PathBuf {
    let dir = root.join(name);
    let wal_dir = "store.shamirwal";
    let wal_file = "store.shamirwal/00000000.wal";
    fs::create_dir_all(dir.join(wal_dir)).expect("WAL dir");
    fs::write(dir.join(wal_file), []).expect("WAL segment");
    let manifest = build_v2(
        time,
        vec![ManifestFileEntry {
            path: wal_file.to_owned(),
            sha256: EMPTY_SHA256.to_owned(),
            size_bytes: 0,
        }],
        None,
        None,
        ConsistencySection {
            per_repo: [(
                "repo".to_owned(),
                RepoConsistency {
                    wal_position: 0,
                    store_dir: "store".to_owned(),
                    schema_version: 0,
                },
            )]
            .into(),
            system_store_version: 0,
        },
        WalSection {
            dirs: vec![WalDirEntry {
                repo: "repo".to_owned(),
                dir: wal_dir.to_owned(),
                segments: vec![WalSegmentEntry {
                    seq: 0,
                    file: wal_file.to_owned(),
                    sidecar: None,
                    sha256: EMPTY_SHA256.to_owned(),
                    size: 0,
                }],
            }],
        },
    );
    write_manifest(&dir, &encode(&manifest).expect("encode manifest")).expect("write manifest");
    write_backup_meta(&dir).expect("sidecar");
    if let Some(owner) = owner {
        let mut value = sidecar(&dir);
        value["scheduled_retention"] = json!({"version": 1, "owner": owner});
        fs::write(
            dir.join(BACKUP_META_FILE_NAME),
            serde_json::to_vec(&value).expect("encode sidecar"),
        )
        .expect("write sidecar");
    }
    verify_manifest(&dir).expect("valid fixture");
    dir
}

fn sidecar(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join(BACKUP_META_FILE_NAME)).expect("read sidecar"))
        .expect("decode sidecar")
}

fn retain(root: &Path, fresh: &Path, keep_last: usize) -> RetentionOutcome {
    prune(
        root,
        fresh,
        OWNER,
        &BackupRetentionConfig {
            keep_last,
            max_age_days: Some(1),
        },
        &CancelFlag::default(),
    )
}

#[test]
fn keeps_foreign_unverified_and_unknown_entries_even_when_old() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let other = copy(&root, "20200101_000002", 2, Some("other-owner"));
    let admin = copy(&root, "20200101_000003", 3, None);
    let wrong_name = copy(&root, "20200230_000004", 4, Some(OWNER));
    let corrupt = copy(&root, "20200101_000005", 5, Some(OWNER));
    fs::write(corrupt.join("store.shamirwal/00000000.wal"), b"corrupt").expect("damage WAL");
    assert!(verify_manifest(&corrupt).is_err());
    let unknown = copy(&root, "20200101_000006", 6, Some(OWNER));
    fs::create_dir(unknown.join("empty-foreign-subtree")).expect("unknown directory");
    verify_manifest(&unknown).expect("verifier ignores empty directories");
    let extra_file = copy(&root, "20200101_000007", 7, Some(OWNER));
    fs::write(extra_file.join("operator-notes"), b"notes").expect("extra file");
    let foreign = root.join("20200101_000008");
    fs::write(&foreign, b"foreign").expect("foreign file");
    let floor = copy(&root, "20200101_000009", 9, Some(OWNER));
    let fresh = copy(&root, "20200101_000010", 10, None);
    let original_sidecar = sidecar(&fresh);
    let original_manifest = fs::read(fresh.join(MANIFEST_FILE_NAME)).expect("read manifest");

    let outcome = retain(&root, &fresh, 2);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(7))
    );
    assert!(!old.exists());
    for path in [
        other,
        admin,
        wrong_name,
        corrupt,
        unknown,
        extra_file,
        foreign,
        floor,
        fresh.clone(),
    ] {
        assert!(path.exists(), "preserved {path:?}");
    }
    assert_eq!(
        fs::read(fresh.join(MANIFEST_FILE_NAME)).expect("manifest"),
        original_manifest
    );
    let mut tagged = sidecar(&fresh);
    assert_eq!(
        tagged
            .as_object_mut()
            .expect("object")
            .remove("scheduled_retention"),
        Some(json!({"version": 1, "owner": OWNER}))
    );
    assert_eq!(tagged, original_sidecar);
    verify_manifest(&fresh).expect("still verifies after tagging");
}

#[test]
fn keeps_skewed_fresh_copy_outside_floor_and_breaks_ties_by_name() {
    let (_temp, root) = private_root();
    let keep = copy(&root, "20200101_000003", 2, Some(OWNER));
    let delete = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000001", 1, None);
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(0))
    );
    assert!(!delete.exists());
    assert!(keep.exists());
    assert!(fresh.exists());
}

#[test]
fn unverifiable_fresh_copy_prevents_tagging_and_pruning() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let _newer = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000003", 3, None);
    fs::write(fresh.join("store.shamirwal/00000000.wal"), b"bad").expect("damage WAL");
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 1, Some(1))
    );
    assert!(old.exists());
    assert!(sidecar(&fresh).get("scheduled_retention").is_none());
}

#[test]
fn refuses_foreign_fresh_sidecar_and_preserves_original_fields() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, Some("other-owner"));
    let original = sidecar(&fresh);
    let outcome = retain(&root, &fresh, 1);
    assert_eq!((outcome.deleted, outcome.failed), (0, 1));
    assert!(old.exists());
    assert_eq!(sidecar(&fresh), original);
}

#[test]
fn incomplete_root_scan_has_no_gauge_and_deletes_nothing() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, None);
    for index in 0..ROOT_ENTRIES - 1 {
        fs::write(root.join(format!("foreign-{index}")), []).expect("foreign entry");
    }
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 1, None)
    );
    assert!(old.exists());
}

#[test]
fn over_budget_manifest_aborts_the_pass_before_pruning() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let large = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000003", 3, None);
    fs::OpenOptions::new()
        .write(true)
        .open(large.join(MANIFEST_FILE_NAME))
        .expect("open manifest")
        .set_len((16 << 20) + 1)
        .expect("oversize manifest");
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 1, None)
    );
    assert!(old.exists());
}

#[test]
fn pass_budget_charges_unknown_regular_files_before_classification() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let unknown = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000003", 3, None);
    fs::write(unknown.join("unknown"), vec![0u8; 8193]).expect("unknown payload");
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, 8192);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig {
            keep_last: 1,
            max_age_days: None,
        },
        &mut budget,
        &mut outcome,
    );
    assert!(matches!(refusal, Err(Refusal::Budget)));
    assert_eq!(outcome.deleted, 0);
    assert_eq!(outcome.untouched, None);
    assert!(old.exists());
    assert!(unknown.join("unknown").exists());
    assert!(sidecar(&fresh).get("scheduled_retention").is_none());
}

#[test]
fn cancellation_before_scan_does_not_tag_or_delete() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, None);
    let cancel = CancelFlag::default();
    cancel.cancel();
    let outcome = prune(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig {
            keep_last: 1,
            max_age_days: None,
        },
        &cancel,
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, None)
    );
    assert!(old.exists());
    assert!(sidecar(&fresh).get("scheduled_retention").is_none());
}

#[cfg(unix)]
#[test]
fn rejects_linked_trees_control_files_and_sockets() {
    // AF_UNIX addresses have a small platform limit; TMPDIR on CI need not.
    let temp = tempfile::tempdir_in("/tmp").expect("short Unix tempdir");
    let root = private_root_in(temp.path());
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).expect("outside directory");
    let link = copy(&root, "20200101_000001", 1, Some(OWNER));
    symlink(&outside, link.join("out")).expect("symlink");
    let socket = copy(&root, "20200101_000002", 2, Some(OWNER));
    // The listening handle stays alive during the scan.
    let socket_path = socket.join("s");
    let listener = UnixListener::bind(&socket_path).expect("Unix socket");
    let control = root.join("20200101_000003");
    fs::create_dir(&control).expect("candidate directory");
    fs::write(
        control.join(BACKUP_META_FILE_NAME),
        fs::read(link.join(BACKUP_META_FILE_NAME)).expect("source tag"),
    )
    .expect("candidate tag");
    symlink(
        link.join(MANIFEST_FILE_NAME),
        control.join(MANIFEST_FILE_NAME),
    )
    .expect("manifest symlink");
    let fresh = copy(&root, "20200101_000004", 4, None);

    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, Some(3))
    );
    assert!(fs::symlink_metadata(link.join("out"))
        .expect("link remains")
        .is_symlink());
    assert!(fs::symlink_metadata(&socket_path).is_ok());
    assert!(fs::symlink_metadata(control.join(MANIFEST_FILE_NAME))
        .expect("manifest remains")
        .is_symlink());
    assert!(outside.exists());
    drop(listener);
}
