use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::super::validate_copied_data_dir;
use crate::audit_appender::FjallAuditAppender;
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

#[test]
fn copied_server_meta_validator_accepts_valid_chain_with_both_anchors() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(&source).expect("create source root");
    let meta_path = source.join("server_meta");
    let store = crate::server_meta::ServerMetaStore::open_or_init(&meta_path)
        .expect("initialize source meta store");
    let key = store.audit_chain_key().expect("read audit key");
    let chain = AuditChain::new(key);
    let appender = FjallAuditAppender::open_strict(&source).expect("open audit appender");
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
    copy_tree(&source.join("server_meta"), &copied.join("server_meta"));
    fs::copy(source.join("audit.log"), copied.join("audit.log")).expect("copy active audit log");
    copy_tree(
        &source.join("audit_checkpoint"),
        &copied.join("audit_checkpoint"),
    );

    assert!(validate_copied_data_dir(&source, &copied).is_ok());
}

#[test]
fn copied_server_meta_validator_refuses_missing_checkpoint_copy() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(&source).expect("create source root");
    let meta_path = source.join("server_meta");
    let store = crate::server_meta::ServerMetaStore::open_or_init(&meta_path)
        .expect("initialize source meta store");
    let key = store.audit_chain_key().expect("read audit key");
    let chain = AuditChain::new(key);
    let appender = FjallAuditAppender::open_strict(&source).expect("open audit appender");
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
    copy_tree(&source.join("server_meta"), &copied.join("server_meta"));
    fs::copy(source.join("audit.log"), copied.join("audit.log")).expect("copy active audit log");

    let error =
        validate_copied_data_dir(&source, &copied).expect_err("missing checkpoint copy refused");
    assert!(error
        .to_string()
        .contains("copied audit chain does not validate"));
}

#[test]
fn copied_server_meta_validator_refuses_missing_server_meta_copy() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir_all(source.join("server_meta")).expect("create source server_meta");
    fs::create_dir_all(&copied).expect("create copied root");
    let error = validate_copied_data_dir(&source, &copied).expect_err("missing meta copy refused");
    assert!(error.to_string().contains("copied server_meta is missing"));
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

#[test]
fn copied_server_meta_validator_refuses_real_current_switch_after_listing() {
    let fixture = TestDir::new();
    let source = fixture.path().join("source");
    let meta_path = source.join("server_meta");
    let store = crate::server_meta::ServerMetaStore::open_or_init(&meta_path)
        .expect("initialize source meta store");

    let listed = tree_paths(&meta_path);
    let before = file_inventory(&meta_path);
    let mut expected_manifest = Vec::new();
    let current = listed
        .iter()
        .find(|path| path.file_name().is_some_and(|name| name == "current"))
        .expect("Fjall fixture exposes its current pointer")
        .clone();
    assert!(
        file_inventory(&meta_path)
            .iter()
            .any(|(path, _)| path == &current),
        "listed current pointer has bytes"
    );
    let old_versions: Vec<_> = listed
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('v'))
        })
        .cloned()
        .collect();
    assert!(
        !old_versions.is_empty(),
        "fixture starts with retained Fjall versions"
    );
    let old_current_bytes =
        fs::read(meta_path.join(&current)).expect("read listed current pointer before switch");
    store
        .store_audit_checkpoint(2, [0xA5; 32])
        .expect("durably write after listing and flush Fjall");
    let rotation_db = fjall::Database::builder(&meta_path)
        .open()
        .expect("open Fjall fixture for rotation");
    let rotation_keyspace = rotation_db
        .keyspace("server_meta_v1", fjall::KeyspaceCreateOptions::default)
        .expect("open meta keyspace for rotation");
    rotation_keyspace
        .rotate_memtable_and_wait()
        .expect("force a persisted Fjall version rotation");
    drop(rotation_keyspace);
    drop(rotation_db);
    let current_bytes = fs::read(meta_path.join(&current)).expect("read rotated current pointer");
    let version_id = u64::from_le_bytes(current_bytes[..8].try_into().expect("current version id"));
    let rotated_version = meta_path.join(format!("v{version_id}"));
    assert!(
        rotated_version.is_file(),
        "rotated current target exists in source"
    );
    assert!(
        !listed.contains(&PathBuf::from(format!("v{version_id}"))),
        "rotated target is absent from pre-listing copy set"
    );
    let after = file_inventory(&meta_path);
    assert_ne!(
        before, after,
        "durable Fjall write must switch current after listing"
    );
    assert!(
        after
            .iter()
            .filter(|(path, _)| path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('v')))
            .any(|(path, _)| !listed.contains(path)),
        "the pointer switch requires a version file absent from the source listing"
    );
    assert!(
        after.iter().any(|(path, _)| path == &current),
        "fixture assumption unmet: rotate_memtable_and_wait must switch current"
    );
    for old in old_versions {
        assert!(
            after.iter().any(|(path, _)| path == &old),
            "old version retained: {old:?}"
        );
    }
    assert_ne!(
        old_current_bytes,
        fs::read(meta_path.join(&current)).expect("read switched current pointer"),
        "the source current pointer changed after the listing"
    );
    drop(store);

    let copied = fixture.path().join("copied");
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
            let bytes = fs::read(&source_file).expect("read listed source after switch");
            expected_manifest.push((
                relative.clone(),
                Sha256::digest(&bytes).into(),
                bytes.len() as u64,
            ));
            fs::write(destination_file, bytes).expect("copy a path from the pre-write listing");
        }
    }
    let copied_inventory = file_inventory(&copied.join("server_meta"));
    let listed_files: Vec<_> = before.iter().map(|(path, _)| path.clone()).collect();
    let copied_paths: Vec<_> = copied_inventory
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    assert_eq!(
        copied_paths, listed_files,
        "copied regular-file paths equal the pre-switch regular-file listing"
    );
    let copied_hashes = inventory_manifest(&copied.join("server_meta"));
    assert_eq!(
        copied_hashes, expected_manifest,
        "the destination independently matches the naive manifest generated from post-switch source bytes"
    );
    assert!(
        after
            .iter()
            .filter(|(path, _)| path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('v')))
            .any(|(path, _)| !listed.contains(path)
                && !copied.join("server_meta").join(path).exists()),
        "new version file is omitted because copying used the pre-switch path listing"
    );
    assert_eq!(
        fs::read(copied.join("server_meta").join(&current)).expect("copied current pointer"),
        fs::read(meta_path.join(&current)).expect("live switched pointer"),
        "the listed current path is read after the switch and carries new bytes"
    );
    let probe = fixture.path().join("probe").join("server_meta");
    copy_tree(&copied.join("server_meta"), &probe);
    assert!(
        !crate::server_meta::open_existing(&probe)
            .expect_err("malformed copied Fjall store must fail")
            .to_string()
            .is_empty(),
        "probe open must report the failed store"
    );
    assert_eq!(
        file_inventory(&copied.join("server_meta")),
        copied_inventory,
        "opening the disposable probe leaves the copied fixture untouched"
    );
    let error = validate_copied_data_dir(&source, &copied).expect_err(
        "throw-away Fjall open or audit-anchor validation must refuse before publication",
    );
    assert!(
        error
            .to_string()
            .starts_with("system store flush failed: copied server_meta cannot open"),
        "expected typed copied server_meta refusal, got {error}"
    );
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create probe directory");
    for entry in fs::read_dir(source).expect("list copied Fjall tree") {
        let source_path = entry.expect("read copied Fjall entry").path();
        let destination_path = destination.join(source_path.file_name().expect("entry name"));
        let metadata = fs::symlink_metadata(&source_path).expect("stat copied Fjall entry");
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
            fs::copy(source_path, destination_path).expect("copy disposable Fjall probe");
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
