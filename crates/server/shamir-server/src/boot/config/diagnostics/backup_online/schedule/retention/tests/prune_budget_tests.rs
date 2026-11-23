//! Pass-budget accounting and fresh-copy ownership tests: the pass charges the
//! verifier's real reads (a manifest file once, a WAL segment twice) and
//! nothing else, and the fresh copy is owned before a scan can refuse.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use shamir_private_dir::create_private_dir;
use tempfile::TempDir;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{
    build_v2, encode, write_backup_meta, write_manifest, ConsistencySection, RepoConsistency,
    WalDirEntry, WalSection, WalSegmentEntry, BACKUP_META_FILE_NAME,
};
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{verify_manifest, ManifestFileEntry, MANIFEST_FILE_NAME};

use super::super::budget::{Budget, Refusal};
use super::super::prune::prune_inner;
use super::super::{prune, RetentionOutcome};

const OWNER: &str = "retention-test-owner";
const WAL_DIR: &str = "store.shamirwal";
const WAL_FILE: &str = "store.shamirwal/00000000.wal";

fn private_root() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("private");
    create_private_dir(&root).expect("private root");
    (temp, root.canonicalize().expect("canonical root"))
}

fn copy_with_payload(
    root: &Path,
    name: &str,
    time: u128,
    owner: Option<&str>,
    payload: &[u8],
) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(dir.join(WAL_DIR)).expect("WAL dir");
    fs::write(dir.join(WAL_FILE), payload).expect("WAL segment");
    let size = u64::try_from(payload.len()).expect("payload size fits u64");
    let sha256 = hex::encode(Sha256::digest(payload));
    let manifest = build_v2(
        time,
        vec![ManifestFileEntry {
            path: WAL_FILE.to_owned(),
            sha256: sha256.clone(),
            size_bytes: size,
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
                dir: WAL_DIR.to_owned(),
                segments: vec![WalSegmentEntry {
                    seq: 0,
                    file: WAL_FILE.to_owned(),
                    sidecar: None,
                    sha256,
                    size,
                }],
            }],
        },
    );
    write_manifest(&dir, &encode(&manifest).expect("encode manifest")).expect("write manifest");
    write_backup_meta(&dir).expect("sidecar");
    verify_manifest(&dir).expect("valid fixture");
    if let Some(owner) = owner {
        let mut value = sidecar(&dir);
        value["scheduled_retention"] = json!({"version": 1, "owner": owner});
        fs::write(
            dir.join(BACKUP_META_FILE_NAME),
            serde_json::to_vec(&value).expect("encode sidecar"),
        )
        .expect("write sidecar");
    }
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
        &BackupRetentionConfig { keep_last },
        &CancelFlag::default(),
        &BTreeSet::new(),
    )
}

/// Bytes one pass charges for a payload copy: the bounded manifest read, the
/// bounded sidecar read and the payload, which is a WAL segment and therefore
/// read twice by the verifier. Each bounded read reserves one extra byte to
/// detect a size change.
fn true_read_bytes(dir: &Path) -> u64 {
    let manifest = fs::metadata(dir.join(MANIFEST_FILE_NAME))
        .expect("manifest metadata")
        .len();
    let sidecar = fs::metadata(dir.join(BACKUP_META_FILE_NAME))
        .expect("sidecar metadata")
        .len();
    let payload = fs::metadata(dir.join(WAL_FILE))
        .expect("payload metadata")
        .len();
    manifest + 1 + sidecar + 1 + 2 * payload
}

/// Total true reads of the three-copy tree below, in bytes.
fn three_copy_reads(oldest: &Path, newest: &Path, fresh: &Path) -> u64 {
    true_read_bytes(oldest) + true_read_bytes(newest) + true_read_bytes(fresh)
}

#[test]
fn one_pass_charges_the_verifiers_real_reads() {
    let (_temp, root) = private_root();
    let payload = vec![0u8; 64 << 10];
    let oldest = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &payload);
    let newest = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &payload);
    let fresh = copy_with_payload(&root, "20200101_000003", 3, None, &payload);
    // A slack far below one payload: charging a file the verifier never reads,
    // or one extra payload per copy, exceeds it.
    let budget_bytes = three_copy_reads(&oldest, &newest, &fresh) + (4 << 10);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        refusal.is_ok(),
        "one pass of the verifier's true reads fits a budget that covers exactly them"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(0))
    );
    assert!(!oldest.exists());
    assert!(newest.exists());
    assert!(fresh.exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&json!({"version": 1, "owner": OWNER}))
    );
}

#[test]
fn pass_budget_refuses_when_the_second_wal_read_does_not_fit() {
    let (_temp, root) = private_root();
    let payload = vec![0u8; 64 << 10];
    let oldest = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &payload);
    let newest = copy_with_payload(&root, "20200101_000002", 2, Some(OWNER), &payload);
    let fresh = copy_with_payload(&root, "20200101_000003", 3, None, &payload);
    // One payload short of the true reads: a pass that charges a WAL segment
    // once (three copies, three missing second reads) would still fit.
    let budget_bytes = three_copy_reads(&oldest, &newest, &fresh) - (64 << 10);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 2 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Budget)),
        "the verifier reads a WAL segment twice: the pass must run out of budget"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed),
        (0, 0),
        "a refusing scan deletes nothing"
    );
    assert!(oldest.exists());
    assert!(newest.exists());
}

#[test]
fn scan_refusal_keeps_the_fresh_copy_owned() {
    let (_temp, root) = private_root();
    let other = copy_with_payload(&root, "20200101_000001", 1, Some(OWNER), &[0u8; 8 << 10]);
    let fresh = copy_with_payload(&root, "20200101_000002", 2, None, &[0u8; 1 << 10]);
    let budget_bytes = true_read_bytes(&fresh) + (4 << 10);
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, budget_bytes);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 1 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        matches!(refusal, Err(Refusal::Budget)),
        "the other candidate cannot fit the remaining budget"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, None),
        "the refusing scan deletes nothing and publishes no classification; \
         the failure counter belongs to the caller of prune_inner"
    );
    assert!(other.exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&json!({"version": 1, "owner": OWNER})),
        "the fresh copy is tagged before the scan can refuse"
    );

    // A later scheduled run has a NEW fresh copy: the earlier fresh copy is
    // classified by the scan like any other candidate. The floor counts the
    // new fresh copy as one of the newest, so `keep_last = 2` keeps the earlier
    // fresh copy only when it was tagged (otherwise the older copy is the
    // second newest owned one and nothing is pruned).
    let later = copy_with_payload(&root, "20200101_000003", 3, None, &[0u8; 1 << 10]);
    let next = retain(&root, &later, 2);
    assert_eq!(
        (next.deleted, next.failed, next.untouched),
        (1, 0, Some(0)),
        "the earlier fresh copy is owned and the older copy is pruned"
    );
    assert!(next.adopted.is_empty(), "nothing is adopted");
    assert!(!other.exists());
    assert!(fresh.exists());
    assert!(later.exists());
}
