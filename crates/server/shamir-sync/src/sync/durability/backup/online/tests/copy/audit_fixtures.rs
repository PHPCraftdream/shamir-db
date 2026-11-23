//! Shared audit fixtures of the online-copy tests: the two trusted anchors a
//! copy must carry (`audit_checkpoint` and `server_meta`), the rotation
//! archive name, and the plain snapshot-tree copy the restore-side
//! assertions use.
//!
//! The `server_meta` anchor is a REAL Fjall store in the production layout
//! (`<data_dir>/server_meta`, keyspace `server_meta_v1`, row
//! `audit_checkpoint` encoded with `rmp_serde::to_vec_named` and a
//! `serde_bytes` digest — the shape `shamir-server`'s `ServerMetaStore`
//! writes). Only the Rust type is local: the server's row type is private to
//! that crate. The store is closed before the copy runs and reopened after
//! the restore, so a phase-1 mutant that skips `server_meta/` fails on the
//! missing store, not on a missing text file.

use std::fs;
use std::path::Path;

use fjall::{Database, KeyspaceCreateOptions};

use crate::sync::durability::backup::online::copy_engine::test_seams;
use crate::sync::durability::backup::online::manifest;

/// Directory of the SERVER metadata store inside `data_dir` — the second
/// trusted audit anchor (`server_meta`), persisted by the server's meta hook
/// after the appender's own `audit_checkpoint` anchor.
pub(crate) const SERVER_META_DIR: &str = "server_meta";

/// Keyspace of that store (production `ServerMetaStore`).
const SERVER_META_KEYSPACE: &str = "server_meta_v1";

/// Key of the audit anchor row inside [`SERVER_META_KEYSPACE`].
const SERVER_META_ANCHOR_KEY: &str = "audit_checkpoint";

/// The production row shape of the `server_meta` audit anchor
/// (`PersistedAuditCheckpoint`): named fields, digest as bytes.
#[derive(serde::Serialize, serde::Deserialize)]
struct ServerMetaAnchorRow {
    last_audit_seq: u64,
    #[serde(with = "serde_bytes")]
    last_audit_hmac: Vec<u8>,
}

/// A per-fixture audit chain key: every fixture picks its own seed, so a
/// mutant that ignores the key cannot pass them all.
pub(crate) fn audit_key(seed: u8) -> [u8; 32] {
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = seed.wrapping_add(index as u8).wrapping_mul(3);
    }
    key
}

/// The exact rotation archive name of the audit chain: `audit.log.` + 20
/// ASCII decimal digits.
pub(crate) fn archive_name(index: u64) -> String {
    format!("audit.log.{index:020}")
}

/// Persist the `server_meta` anchor the way the server's meta hook does:
/// AFTER the appender's `audit_checkpoint` anchor, with the same
/// `(next_seq, hmac)` pair. The store is created, filled, synced and CLOSED
/// again, so the following copy sees closed files.
pub(crate) fn persist_server_meta_anchor(dir: &Path, position: (u64, [u8; 32])) {
    let path = dir.join(SERVER_META_DIR);
    fs::create_dir_all(&path).expect("mkdirs server_meta");
    let store = Database::builder(&path)
        .open()
        .expect("open the fixture server_meta store");
    let keyspace = store
        .keyspace(SERVER_META_KEYSPACE, KeyspaceCreateOptions::default)
        .expect("server_meta keyspace");
    let row = ServerMetaAnchorRow {
        last_audit_seq: position.0,
        last_audit_hmac: position.1.to_vec(),
    };
    let encoded = rmp_serde::to_vec_named(&row).expect("encode the server_meta anchor");
    keyspace
        .insert(SERVER_META_ANCHOR_KEY.as_bytes(), encoded.as_slice())
        .expect("insert the server_meta anchor");
    store
        .persist(fjall::PersistMode::SyncAll)
        .expect("persist the server_meta store");
    drop(keyspace);
    drop(store);
}

/// Read the anchor back through a REOPENED Fjall store, panicking with the
/// phase-1 requirement when the store is absent: the data-directory copy's
/// phase 1 captures every anchor store, so a restored tree without this
/// store means the copy dropped a whole anchor.
pub(crate) fn read_server_meta_anchor(dir: &Path) -> (u64, [u8; 32]) {
    let path = dir.join(SERVER_META_DIR);
    assert!(
        path.join("keyspaces").is_dir(),
        "the copy must carry the server_meta anchor store (phase 1 captures it): {} has no keyspaces",
        path.display()
    );
    let store = Database::builder(&path)
        .open()
        .expect("reopen the restored server_meta store");
    let keyspace = store
        .keyspace(SERVER_META_KEYSPACE, KeyspaceCreateOptions::default)
        .expect("server_meta keyspace");
    let entry = keyspace
        .get(SERVER_META_ANCHOR_KEY.as_bytes())
        .expect("read the server_meta anchor row")
        .expect("the restored server_meta store must carry the audit anchor row");
    let row: ServerMetaAnchorRow =
        rmp_serde::from_slice(&entry).expect("decode the server_meta anchor row");
    let hmac: [u8; 32] = row
        .last_audit_hmac
        .try_into()
        .expect("the server_meta anchor digest is 32 bytes");
    (row.last_audit_seq, hmac)
}

/// Every snapshot-relative path the published manifest lists.
pub(crate) fn manifest_paths(dest_dir: &Path) -> Vec<String> {
    let raw = fs::read_to_string(dest_dir.join("manifest.json")).expect("read the manifest");
    let decoded = manifest::decode(&raw).expect("decode the manifest");
    decoded.files.into_iter().map(|entry| entry.path).collect()
}

/// The published manifest's entry for one snapshot-relative path:
/// `(sha256, size_bytes)`.
pub(crate) fn manifest_entry(dest_dir: &Path, rel: &str) -> (String, u64) {
    let raw = fs::read_to_string(dest_dir.join("manifest.json")).expect("read the manifest");
    let decoded = manifest::decode(&raw).expect("decode the manifest");
    decoded
        .files
        .into_iter()
        .find(|entry| entry.path == rel)
        .map(|entry| (entry.sha256, entry.size_bytes))
        .unwrap_or_else(|| panic!("the manifest must list {rel}"))
}

/// Plain file-tree copy of a snapshot into a fresh directory: restoring an
/// online copy moves the snapshot tree into the target `data_dir` (engine
/// stores are replayed separately; the audit files are carried as bytes).
/// NOT the production restore: manifest verification, staging and the swap
/// have their own tests, and the audit tests compensate by asserting each
/// manifest entry against the bytes they open. The snapshot omits every
/// Fjall `lock`, so the copy recreates them where the production restore
/// does, before any staged store is opened.
pub(crate) fn copy_snapshot_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create the restored directory");
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read the snapshot directory") {
            let entry = entry.expect("snapshot entry");
            let path = entry.path();
            let target = to.join(path.strip_prefix(from).expect("under the snapshot root"));
            if path.is_dir() {
                fs::create_dir_all(&target).expect("create the restored subdirectory");
                stack.push(path);
            } else {
                fs::copy(&path, &target).expect("copy a snapshot file");
            }
        }
    }
    test_seams::recreate_fjall_locks(to);
}
