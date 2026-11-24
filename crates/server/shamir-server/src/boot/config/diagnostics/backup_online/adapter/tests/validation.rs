use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::validate_copied_data_dir;
use sha2::{Digest, Sha256};

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
        "new Fjall current pointer is retained"
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
        crate::server_meta::open_existing(&probe).is_err(),
        "the malformed copied Fjall store itself must be unopenable"
    );
    assert_eq!(
        file_inventory(&copied.join("server_meta")),
        copied_inventory,
        "opening the disposable probe leaves the copied fixture untouched"
    );
    assert!(
        validate_copied_data_dir(&copied).is_err(),
        "throw-away Fjall open or audit-anchor validation must refuse before publication"
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
