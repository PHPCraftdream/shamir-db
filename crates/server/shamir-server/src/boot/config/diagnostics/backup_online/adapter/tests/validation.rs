use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::super::{set_validation_root_candidate, validate_copied_data_dir};
use crate::audit_appender::FjallAuditAppender;
use crate::sync::durability::backup::online::manifest::{
    self as online_manifest, BackupKind, ManifestV2, UnknownKeys,
};
use crate::sync::durability::backup::online::wal_capture::PortError;
use crate::sync::durability::backup::{verify_manifest, ManifestFileEntry, MANIFEST_FILE_NAME};
use shamir_connect::server::audit_chain::{AuditAppender, AuditChain};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "shamir-validator-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create test directory: {error}"),
            }
        }
        panic!("could not allocate test directory");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Seeds a valid audit chain fixture into `source`: an initialised
/// `server_meta`, one appended entry, and BOTH anchors (the appender
/// checkpoint and the `server_meta` anchor) durably persisted; all
/// handles are dropped before returning.
fn seed_valid_audit_chain(source: &Path) {
    fs::create_dir_all(source).expect("create source root");
    let meta_path = source.join("server_meta");
    let store = crate::server_meta::ServerMetaStore::open_or_init(&meta_path)
        .expect("initialize source meta store");
    let key = store.audit_chain_key().expect("read audit key");
    let chain = AuditChain::new(key);
    let appender = FjallAuditAppender::open_strict(source).expect("open audit appender");
    let entry = chain
        .append("validation", "test", "", "", [0; 8], "ok", Vec::new(), 1)
        .expect("create audit entry");
    appender.append_entry(&entry).expect("persist audit entry");
    let (next_seq, prev_hmac) = chain.checkpoint();
    appender
        .checkpoint(next_seq, &prev_hmac)
        .expect("persist appender anchor");
    store
        .store_audit_checkpoint(next_seq, prev_hmac)
        .expect("persist server_meta anchor");
    drop(appender);
    drop(store);
}

/// Copies the whole fixture layout (server_meta tree, active audit log,
/// checkpoint fjall) from `source` into `copied`.
fn copy_full_source(source: &Path, copied: &Path) {
    copy_tree(&source.join("server_meta"), &copied.join("server_meta"));
    if source.join("audit.log").is_file() {
        fs::copy(source.join("audit.log"), copied.join("audit.log"))
            .expect("copy active audit log");
    }
    if source.join("audit_checkpoint").is_dir() {
        copy_tree(
            &source.join("audit_checkpoint"),
            &copied.join("audit_checkpoint"),
        );
    }
}

/// Matches `PortError::SystemStore` and hands over the inner I/O error,
/// panicking with a readable message on any other variant (`PortError`'s
/// `Display` does NOT include the inner text).
fn expect_system_store(error: PortError, context: &str) -> std::io::Error {
    let PortError::SystemStore(inner) = error else {
        panic!("expected PortError::SystemStore for {context}, got {error:?}")
    };
    inner
}

#[test]
fn copied_server_meta_validator_accepts_valid_chain_with_both_anchors() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    seed_valid_audit_chain(&source);
    copy_full_source(&source, &copied);

    assert!(validate_copied_data_dir(&source, &copied).is_ok());
}

#[test]
fn copied_server_meta_validator_refuses_missing_checkpoint_copy() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    seed_valid_audit_chain(&source);
    copy_tree(&source.join("server_meta"), &copied.join("server_meta"));
    fs::copy(source.join("audit.log"), copied.join("audit.log")).expect("copy active audit log");

    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied).expect_err("missing checkpoint copy refused"),
        "missing checkpoint copy",
    );
    assert!(
        inner_contains(&error, "copied audit chain does not validate"),
        "expected the audit-chain refusal, got {error}"
    );
}

#[test]
fn copied_server_meta_validator_refuses_missing_server_meta_copy() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(source.join("server_meta")).expect("create source server_meta");
    fs::create_dir_all(&copied).expect("create copied root");
    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied).expect_err("missing meta copy refused"),
        "missing server_meta copy",
    );
    assert!(
        inner_contains(&error, "copied server_meta is missing"),
        "expected the copied server_meta refusal, got {error}"
    );
}

#[test]
fn copied_server_meta_validator_accepts_source_without_any_reserved_stores() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(&source).expect("create empty source root");
    fs::create_dir_all(&copied).expect("create empty copied root");
    assert!(validate_copied_data_dir(&source, &copied).is_ok());
}

/// P1-1: the published audit set is enumerated from the COMPLETED
/// DESTINATION, never from the live source. A corrupt archive copied into
/// the destination (but absent from the source, e.g. planted by a bad
/// copier) must be cloned into the validation tree and refuse — boot scans
/// EVERY archive of the directory it opens, so the validator must see the
/// same file set.
#[test]
fn copied_server_meta_validator_refuses_corrupt_destination_only_archive() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    seed_valid_audit_chain(&source);
    copy_full_source(&source, &copied);
    fs::write(
        copied.join("audit.log.00000000000000000001"),
        b"garbage archive bytes",
    )
    .expect("plant corrupt destination-only archive");

    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied)
            .expect_err("corrupt destination-only archive must be refused"),
        "corrupt destination-only archive",
    );
    assert!(
        inner_contains(&error, "copied audit chain does not validate"),
        "expected the audit-chain refusal, got {error}"
    );
}

/// P1-1, converse: an extra garbage archive in the live SOURCE only must
/// NOT make the validator reject a valid copy — the source decides
/// existence and kind only, never membership of the published set.
#[test]
fn copied_server_meta_validator_accepts_source_only_archive_missing_from_destination() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    seed_valid_audit_chain(&source);
    copy_full_source(&source, &copied);
    fs::write(
        source.join("audit.log.00000000000000000042"),
        b"garbage archive bytes",
    )
    .expect("plant source-only archive");

    assert!(validate_copied_data_dir(&source, &copied).is_ok());
}

/// Clone-omission guard (GREEN on the parent — the parent already refuses
/// this): a checkpoint anchor row that conflicts with the audit chain must
/// refuse, so any mutation that omits `audit_checkpoint` from the
/// validator's clone (letting the meta anchor alone "validate") is killed.
#[test]
fn copied_server_meta_validator_refuses_conflicting_checkpoint_anchor() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    seed_valid_audit_chain(&source);
    // Overwrite the checkpoint anchor row DIRECTLY (format per
    // audit_appender/checkpoint.rs: 8-byte BE next_seq + 32-byte HMAC).
    // The keyspace name "last_audit_hmac" and the key [0u8] mirror the
    // private constants CHECKPOINT_KEYSPACE / CHECKPOINT_KEY.
    let db = fjall::Database::builder(source.join("audit_checkpoint"))
        .open()
        .expect("open checkpoint fjall db");
    let keyspace = db
        .keyspace("last_audit_hmac", fjall::KeyspaceCreateOptions::default)
        .expect("open last_audit_hmac keyspace");
    let mut row = [0u8; 40];
    row[..8].copy_from_slice(&999u64.to_be_bytes());
    row[8..].copy_from_slice(&[0xAA; 32]);
    keyspace
        .insert([0u8], row.as_slice())
        .expect("overwrite anchor row");
    db.persist(fjall::PersistMode::SyncAll)
        .expect("persist conflicting anchor");
    drop(keyspace);
    drop(db);
    copy_full_source(&source, &copied);

    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied)
            .expect_err("conflicting checkpoint anchor must be refused"),
        "conflicting checkpoint anchor",
    );
    assert!(
        inner_contains(&error, "copied audit chain does not validate"),
        "expected the audit-chain refusal, got {error}"
    );
}

/// P1-2: the validation allocator must never ADOPT (and later DELETE) a
/// pre-existing directory at its first candidate root name — the planted
/// marker file proves nothing was removed.
#[test]
fn validation_allocator_never_adopts_or_deletes_a_preexisting_directory() {
    let fixture = tempfile::tempdir().expect("create RAII validation fixture");
    let planted = fixture.path().join("occupant");
    shamir_private_dir::create_exclusive_private_dir(&planted)
        .unwrap_or_else(|error| panic!("create verified-private occupant: {error}"));
    fs::write(planted.join("marker.txt"), b"pre-existing").expect("write marker file");

    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(&source).expect("create empty source root");
    fs::create_dir_all(&copied).expect("create empty copied root");

    set_validation_root_candidate(Some(planted.to_string_lossy().into_owned()));
    let result = validate_copied_data_dir(&source, &copied);
    set_validation_root_candidate(None);

    assert!(
        result.is_ok(),
        "an empty validation pair must pass, got {result:?}"
    );
    assert!(
        planted.is_dir(),
        "the pre-existing planted directory must survive the allocator"
    );
    assert!(
        planted.join("marker.txt").is_file(),
        "the marker file inside the pre-existing directory must survive"
    );
}

#[test]
fn copied_server_meta_validator_refuses_destination_only_server_meta() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(&source).expect("create source without server_meta");
    fs::create_dir_all(copied.join("server_meta"))
        .expect("create destination-only empty server_meta");

    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied)
            .expect_err("destination-only server_meta must be opened and refused"),
        "destination-only server_meta",
    );
    assert!(
        inner_contains(&error, "copied server_meta cannot open"),
        "expected the copied server_meta open refusal, got {error}"
    );
}

#[test]
fn copied_server_meta_validator_refuses_case_variant_reserved_name() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(&source).expect("create empty source root");
    fs::create_dir_all(&copied).expect("create copied root");
    fs::write(copied.join("AUDIT.LOG"), b"reserved case variant")
        .expect("write case-variant reserved file");

    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied)
            .expect_err("case-variant reserved name must be refused"),
        "case-variant reserved name",
    );
    assert!(
        inner_contains(
            &error,
            "reserved store name \"AUDIT.LOG\" differs from \"audit.log\" only by case"
        ),
        "expected the reserved-name case refusal, got {error}"
    );
}

#[test]
fn copied_server_meta_validator_refuses_real_current_switch_after_listing() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let meta_path = source.join("server_meta");
    let store = crate::server_meta::ServerMetaStore::open_or_init(&meta_path)
        .expect("initialize source meta store");
    store
        .store_audit_checkpoint(2, [0xA5; 32])
        .expect("give the active memtable a durable write to seal");

    let keyspaces_dir = meta_path.join("keyspaces");
    let listed = tree_paths(&meta_path);
    let mut pointers: Vec<(PathBuf, Vec<u8>, Vec<String>)> = Vec::new();
    for relative in &listed {
        if relative.file_name().is_some_and(|name| name == "current")
            && relative.starts_with("keyspaces")
        {
            let dir_relative = relative
                .parent()
                .expect("current parent directory")
                .to_path_buf();
            let dir_path = meta_path.join(&dir_relative);
            let version_names = fs::read_dir(&dir_path)
                .expect("list keyspace tree versions")
                .filter_map(|entry| {
                    let entry = entry.expect("read keyspace tree entry");
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let metadata = entry.file_type().expect("stat keyspace tree entry");
                    (metadata.is_file() && name.starts_with('v')).then_some(name)
                })
                .collect();
            pointers.push((
                dir_path,
                fs::read(meta_path.join(relative)).expect("read current pointer before rotation"),
                version_names,
            ));
        }
    }
    assert!(
        !pointers.is_empty(),
        "unmet fixture assumption: no keyspace current pointers found under {}",
        keyspaces_dir.display()
    );

    crate::server_meta::rotate_meta_memtable_for_test(&store)
        .expect("force a persisted Fjall version rotation through the store's own handle");

    let mut changed = Vec::new();
    for (dir_path, old_bytes, version_names) in &pointers {
        let new_bytes =
            fs::read(dir_path.join("current")).expect("read current pointer after rotation");
        if new_bytes != *old_bytes {
            changed.push((
                dir_path.clone(),
                old_bytes.clone(),
                new_bytes,
                version_names.clone(),
            ));
        }
    }
    assert!(
        changed.len() == 1,
        "unmet fixture assumption: expected exactly one fjall keyspace pointer to switch after rotation, observed {} changed of {}",
        changed.len(), pointers.len()
    );
    let (tree_root, old_current_bytes, new_current_bytes, old_version_names) = changed.remove(0);
    let old_id = u64::from_le_bytes(
        old_current_bytes[..8]
            .try_into()
            .expect("current version id"),
    );
    let new_id = u64::from_le_bytes(
        new_current_bytes[..8]
            .try_into()
            .expect("current version id"),
    );
    assert!(
        old_version_names
            .iter()
            .any(|name| name == &format!("v{old_id}")),
        "pre-rotation v{old_id} existed in the changed tree"
    );
    assert!(
        tree_root.join(format!("v{new_id}")).is_file(),
        "post-rotation v{new_id} exists in the changed tree"
    );
    assert!(
        tree_root.join(format!("v{old_id}")).is_file(),
        "old v{old_id} is retained after rotation"
    );
    let new_version_relative = tree_root
        .join(format!("v{new_id}"))
        .strip_prefix(&meta_path)
        .expect("relative version path")
        .to_path_buf();
    assert!(
        !listed.contains(&new_version_relative),
        "the rotated target is absent from the pre-listing copy set"
    );
    assert_ne!(
        old_current_bytes, new_current_bytes,
        "the source current pointer changed after the listing"
    );

    // Copy the pre-listing NAMES but read the bytes AFTER the rotation: the
    // copy then holds the NEW `current` bytes while missing `v{new_id}` and
    // keeping `v{old_id}` — the exact mixed image the validator must refuse.
    let copied = fixture.path().join("copied");
    let mut expected_manifest = Vec::new();
    for relative in &listed {
        let source_file = meta_path.join(relative);
        let destination_file = copied.join("server_meta").join(relative);
        let metadata = fs::symlink_metadata(&source_file).expect("stat listed source path");
        if metadata.is_dir() {
            fs::create_dir_all(&destination_file).expect("create listed directory");
        } else {
            if let Some(parent) = destination_file.parent() {
                fs::create_dir_all(parent).expect("create listed destination parent");
            }
            let bytes = fs::read(&source_file).expect("read listed source after the rotation");
            expected_manifest.push(ManifestFileEntry {
                path: format!("server_meta/{}", forward_slashes(relative)),
                sha256: hex::encode(Sha256::digest(&bytes)),
                size_bytes: bytes.len() as u64,
            });
            fs::write(destination_file, bytes).expect("copy a path from the pre-rotation listing");
        }
    }

    // Naive-manifest assertions over the copied bytes.
    let copied_hashes = inventory_manifest(&copied.join("server_meta"));
    let listed_files: Vec<PathBuf> = listed
        .iter()
        .filter(|relative| meta_path.join(relative).is_file())
        .cloned()
        .collect();
    let copied_paths: Vec<PathBuf> = copied_hashes
        .iter()
        .map(|(path, _, _)| path.clone())
        .collect();
    assert_eq!(
        copied_paths, listed_files,
        "copied regular-file paths equal the pre-rotation regular-file listing"
    );
    let expected_hashes: Vec<(PathBuf, [u8; 32], u64)> = expected_manifest
        .iter()
        .map(|entry| {
            let relative = entry
                .path
                .strip_prefix("server_meta/")
                .expect("server_meta-prefixed entry")
                .replace('/', std::path::MAIN_SEPARATOR_STR);
            (
                PathBuf::from(relative),
                hex::decode(&entry.sha256)
                    .expect("hex digest")
                    .try_into()
                    .expect("32-byte digest"),
                entry.size_bytes,
            )
        })
        .collect();
    assert_eq!(
        copied_hashes, expected_hashes,
        "the destination independently matches the naive manifest generated from post-rotation source bytes"
    );

    // The file verifier must ACCEPT the mixed image (its job is bytes-level
    // integrity, not semantic store validity) — only the typed validator
    // below refuses it.
    let v2_manifest = ManifestV2 {
        format_version: 2,
        kind: BackupKind::Offline,
        created_at_unix_ns: 1,
        format_epoch: None,
        binary_version: None,
        consistency: None,
        wal: None,
        files: expected_manifest,
        extra: UnknownKeys::default(),
    };
    let encoded = online_manifest::encode(&v2_manifest).expect("encode the v2 manifest");
    fs::write(copied.join(MANIFEST_FILE_NAME), encoded).expect("write manifest.json");
    verify_manifest(&copied).expect("the mixed image must pass file verification");

    drop(store);

    let error = expect_system_store(
        validate_copied_data_dir(&source, &copied)
            .expect_err("the copied server_meta must be refused before publication"),
        "mixed copied server_meta image",
    );
    assert!(
        inner_contains(&error, "copied server_meta cannot open"),
        "expected the copied server_meta open refusal, got {error}"
    );
}

fn inner_contains(error: &std::io::Error, needle: &str) -> bool {
    error.to_string().contains(needle)
}

fn forward_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination directory");
    for entry in fs::read_dir(source).expect("list source tree") {
        let source_path = entry.expect("read source entry").path();
        let destination_path = destination.join(source_path.file_name().expect("entry name"));
        let metadata = fs::symlink_metadata(&source_path).expect("stat source entry");
        assert!(
            !metadata.file_type().is_symlink(),
            "fixture contains no symlinks"
        );
        if metadata.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            assert!(
                metadata.is_file(),
                "fixture contains only regular files and directories"
            );
            fs::copy(source_path, destination_path).expect("copy fixture file");
        }
    }
}

fn tree_paths(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("list Fjall tree") {
            let path = entry.expect("read Fjall entry").path();
            let metadata = fs::symlink_metadata(&path).expect("stat Fjall entry");
            assert!(
                !metadata.file_type().is_symlink(),
                "fixture contains no symlinks"
            );
            if metadata.is_dir() {
                files.push(
                    path.strip_prefix(root)
                        .expect("relative listing path")
                        .to_path_buf(),
                );
                visit(root, &path, files);
            } else if metadata.is_file() {
                files.push(
                    path.strip_prefix(root)
                        .expect("relative listing path")
                        .to_path_buf(),
                );
            }
        }
        files.sort();
    }
    let mut files = Vec::new();
    visit(root, root, &mut files);
    files
}

fn file_inventory(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    tree_paths(root)
        .into_iter()
        .filter(|relative| root.join(relative).is_file())
        .map(|relative| {
            let bytes = fs::read(root.join(&relative)).expect("read listed Fjall bytes");
            (relative, bytes)
        })
        .collect()
}

fn inventory_manifest(root: &Path) -> Vec<(PathBuf, [u8; 32], u64)> {
    file_inventory(root)
        .into_iter()
        .map(|(path, bytes)| {
            let size = bytes.len() as u64;
            (path, Sha256::digest(bytes).into(), size)
        })
        .collect()
}
