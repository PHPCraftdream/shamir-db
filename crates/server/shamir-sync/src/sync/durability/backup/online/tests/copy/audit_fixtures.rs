//! Shared audit fixtures of the online-copy tests: the two trusted anchors a
//! copy must carry (`audit_checkpoint` and `server_meta`), the rotation
//! archive name, and the plain snapshot-tree copy the restore-side
//! assertions use.

use std::fs;
use std::path::Path;

use crate::sync::durability::backup::online::copy_engine::test_seams;

/// Directory of the SERVER metadata store inside `data_dir` — the second
/// trusted audit anchor (`server_meta`), persisted by the server's meta hook
/// after the appender's own `audit_checkpoint` anchor.
pub(crate) const SERVER_META_DIR: &str = "server_meta";

/// The anchor row inside [`SERVER_META_DIR`] as this fixture stores it.
pub(crate) const SERVER_META_ANCHOR_FILE: &str = "audit_anchor";

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
/// `(next_seq, hmac)` pair.
pub(crate) fn persist_server_meta_anchor(dir: &Path, position: (u64, [u8; 32])) {
    let path = dir.join(SERVER_META_DIR).join(SERVER_META_ANCHOR_FILE);
    fs::create_dir_all(path.parent().expect("server_meta parent")).expect("mkdirs server_meta");
    fs::write(
        &path,
        format!("{} {}\n", position.0, hex::encode(position.1)),
    )
    .expect("write the server_meta anchor");
}

/// Read the anchor back from a RESTORED tree, panicking with the phase-1
/// requirement when the store is absent: the data-directory copy's phase 1
/// captures every anchor store, so a restored tree without this file means
/// the copy dropped a whole anchor.
pub(crate) fn read_server_meta_anchor(dir: &Path) -> (u64, [u8; 32]) {
    let path = dir.join(SERVER_META_DIR).join(SERVER_META_ANCHOR_FILE);
    let raw = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "the copy must carry the server_meta anchor store (phase 1 captures it): {}: {error}",
            path.display()
        )
    });
    let (seq, hmac) = raw
        .trim_end()
        .split_once(' ')
        .expect("the server_meta anchor is `seq hmac`");
    let seq: u64 = seq.parse().expect("the server_meta anchor sequence");
    let decoded = hex::decode(hmac).expect("the server_meta anchor hmac hex");
    let hmac: [u8; 32] = decoded
        .try_into()
        .expect("the server_meta anchor hmac is 32 bytes");
    (seq, hmac)
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
