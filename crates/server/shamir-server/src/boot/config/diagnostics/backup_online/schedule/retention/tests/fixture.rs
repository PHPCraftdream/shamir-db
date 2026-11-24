//! Shared fixtures for the retention-pass tests: a private managed root, v2
//! online copies over a single WAL segment (with and without a copied
//! `format.stamp`), the sidecar helpers, and the owner tag both pass-level
//! test files write and assert on.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use shamir_private_dir::create_private_dir;
use tempfile::TempDir;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{
    build_v2, encode, write_backup_meta, write_manifest, ConsistencySection, ManifestV2,
    RepoConsistency, WalDirEntry, WalSection, WalSegmentEntry, BACKUP_META_FILE_NAME,
};
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{verify_manifest, ManifestFileEntry};
use crate::sync::format_stamp::{write_stamp, FormatStamp, CURRENT_FORMAT_EPOCH, STAMP_FILE_NAME};

use super::super::{prune, RetentionOutcome};

pub(super) const OWNER: &str = "retention-test-owner";
pub(super) const WAL_DIR: &str = "store.shamirwal";
pub(super) const WAL_FILE: &str = "store.shamirwal/00000000.wal";

pub(super) fn private_root() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = private_root_in(temp.path());
    (temp, root)
}

pub(super) fn private_root_in(parent: &Path) -> PathBuf {
    let root = parent.join("private");
    create_private_dir(&root).expect("private root");
    root.canonicalize().expect("canonical root")
}

/// The sidecar tag an owned copy carries, as the fixture writes it and the
/// assertions expect it back.
pub(super) fn owner_tag(owner: &str) -> Value {
    json!({
        "version": 1,
        "owner": owner,
    })
}

pub(super) fn sidecar(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join(BACKUP_META_FILE_NAME)).expect("read sidecar"))
        .expect("decode sidecar")
}

/// One pass over `root` with the newest copy `fresh` and no pending
/// provenance records.
pub(super) fn retain(root: &Path, fresh: &Path, keep_last: usize) -> RetentionOutcome {
    prune(
        root,
        fresh,
        OWNER,
        &BackupRetentionConfig { keep_last },
        &CancelFlag::default(),
        &BTreeSet::new(),
    )
}

/// A v2 online copy whose only payload is `payload` as one WAL segment.
pub(super) fn copy_with_payload(
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
        consistency_section(),
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
    finish_copy(&dir, &manifest, owner)
}

/// A v2 online copy with an empty WAL segment plus a real `format.stamp`,
/// listed in `files` exactly as the production writer lists it, so the pass
/// charges the stamp's two real reads.
pub(super) fn copy_with_stamp(root: &Path, name: &str, time: u128, owner: Option<&str>) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(dir.join(WAL_DIR)).expect("WAL dir");
    fs::write(dir.join(WAL_FILE), []).expect("WAL segment");
    write_stamp(&dir, &FormatStamp::new("retention-test", 0)).expect("format stamp");
    let stamp = fs::read(dir.join(STAMP_FILE_NAME)).expect("read stamp");
    let stamp_size = u64::try_from(stamp.len()).expect("stamp size fits u64");
    let empty = hex::encode(Sha256::digest(b""));
    let manifest = build_v2(
        time,
        vec![
            ManifestFileEntry {
                path: WAL_FILE.to_owned(),
                sha256: empty.clone(),
                size_bytes: 0,
            },
            ManifestFileEntry {
                path: STAMP_FILE_NAME.to_owned(),
                sha256: hex::encode(Sha256::digest(&stamp)),
                size_bytes: stamp_size,
            },
        ],
        Some(CURRENT_FORMAT_EPOCH),
        None,
        consistency_section(),
        WalSection {
            dirs: vec![WalDirEntry {
                repo: "repo".to_owned(),
                dir: WAL_DIR.to_owned(),
                segments: vec![WalSegmentEntry {
                    seq: 0,
                    file: WAL_FILE.to_owned(),
                    sidecar: None,
                    sha256: empty,
                    size: 0,
                }],
            }],
        },
    );
    finish_copy(&dir, &manifest, owner)
}

fn consistency_section() -> ConsistencySection {
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
    }
}

fn finish_copy(dir: &Path, manifest: &ManifestV2, owner: Option<&str>) -> PathBuf {
    write_manifest(dir, &encode(manifest).expect("encode manifest")).expect("write manifest");
    write_backup_meta(dir).expect("sidecar");
    verify_manifest(dir).expect("valid fixture");
    if let Some(owner) = owner {
        let mut value = sidecar(dir);
        value["scheduled_retention"] = owner_tag(owner);
        fs::write(
            dir.join(BACKUP_META_FILE_NAME),
            serde_json::to_vec(&value).expect("encode sidecar"),
        )
        .expect("write sidecar");
    }
    dir.to_path_buf()
}
