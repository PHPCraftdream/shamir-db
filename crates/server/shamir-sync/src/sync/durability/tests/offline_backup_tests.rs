//! Offline `backup --from` refuses a live server (brief 47-r2a-3b2 §5, Q5).
//!
//! What each test proves:
//!
//! * Refusal is typed ([`BackupError::SourceRunning`]) and happens
//!   PRE-destination: the `--to` root is not even created.
//! * The refused attempt leaves the source bytes identical (the probe is
//!   read-only shared locks only).
//! * A backup of a STOPPED source leaves the source bytes identical and
//!   succeeds once every lock is released; the snapshot then verifies.
//!
//! Fixtures are deterministic: a real persistent ShamirDb catalogue
//! (`shamir_db_meta.redb`) or literal Fjall databases, opened on a roomy
//! OS thread with its own current-thread Tokio runtime (the init chain
//! nests deep async frames; `Box::pin` keeps the future small). Every
//! `TempDir` is held by the test body; fixture handles drop inside the
//! fixture thread before the stop-state assertions. The only sleeps are
//! bounded retries of ACTUAL lock contention after the fixture dropped its
//! handles (Windows releases asynchronously) — no blind backoff anywhere.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use shamir_db::{ShamirDb, SystemStoreConfig};
use tempfile::TempDir;

use crate::maintenance_lock;
use crate::sync::durability::backup::{backup, verify_manifest, BackupError, BackupReport};

/// Recursive snapshot of `dir`: sorted `(rel path, size, sha256)` of every
/// regular file. The three lock artifacts are excluded: a live holder's
/// exclusive byte-range lock makes them unreadable from another handle on
/// Windows, and they are ownership metadata, not data.
fn snapshot(dir: &Path) -> Vec<(String, u64, String)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, u64, String)>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let ft = entry.file_type().expect("file_type");
            if ft.is_dir() {
                walk(&entry.path(), &format!("{rel}/"), out);
            } else if ft.is_file() && !is_lock_artifact(&rel) {
                let bytes = fs::read(entry.path()).expect("read file");
                out.push((rel, bytes.len() as u64, hex::encode(Sha256::digest(&bytes))));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

/// The lock artifacts the source probe takes; excluded from snapshots.
fn is_lock_artifact(rel: &str) -> bool {
    rel == ".shamir.lock"
        || rel == "server_meta/lock"
        || rel.ends_with("/server_meta/lock")
        || rel == "shamir_db_meta.redb/lock"
        || rel.ends_with("/shamir_db_meta.redb/lock")
}

/// Run `body` on a roomy thread with its own current-thread runtime; the
/// init chain nests async frames deeper than a debug test thread's default
/// stack allows. Every handle the body drops (database, lock file) is
/// released before `join` returns.
fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn fixture thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// Assert the live-source refusal: typed error, `--to` root still absent,
/// source bytes identical to `before`.
fn assert_refused(source: &Path, to: &Path, before: &[(String, u64, String)]) {
    let err = backup(source, to).expect_err("live source must refuse");
    assert!(
        matches!(err, BackupError::SourceRunning(_)),
        "expected SourceRunning, got: {err}"
    );
    let rendered = err.to_string();
    assert!(
        rendered.contains("StartBackup"),
        "refusal must name the online alternative: {rendered}"
    );
    assert!(
        !to.exists(),
        "refusal must happen before destination creation"
    );
    assert_eq!(
        snapshot(source),
        before,
        "refused backup must not write the source"
    );
}

/// Backup with bounded retries over ACTUAL lock contention only: the
/// fixture dropped its handles but Windows releases the advisory locks
/// asynchronously. `SourceRunning` past the deadline is a real failure (a
/// leaked fixture lock), as is any other error.
fn backup_after_release(source: &Path, to: &Path) -> BackupReport {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match backup(source, to) {
            Ok(report) => return report,
            Err(BackupError::SourceRunning(_)) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => panic!("backup of stopped source failed: {err}"),
        }
    }
}

/// Stop-state success: the source is byte-identical to `stopped` across
/// the copy (backup only reads), files were copied and the manifest
/// verifies.
fn assert_backup_verified(source: &Path, to: &Path, stopped: &[(String, u64, String)]) {
    let report = backup_after_release(source, to);
    assert_eq!(
        snapshot(source),
        stopped,
        "backup must not write the source"
    );
    assert!(report.files_copied > 0, "snapshot must contain files");
    verify_manifest(&report.dest_dir).expect("snapshot manifest verifies");
}

#[test]
fn offline_backup_refused_while_server_meta_fjall_lock_is_held_then_allowed_after_release() {
    let data = TempDir::new().expect("tempdir");
    let dest = TempDir::new().expect("tempdir");
    let to = dest.path().join("backups");
    let source = data.path().to_path_buf();

    on_big_stack({
        let source = source.clone();
        let to = to.clone();
        move || async move {
            let server_meta = fjall::Database::builder(source.join("server_meta"))
                .open()
                .expect("open server_meta fjall db");
            assert!(source.join("server_meta/lock").exists());

            let before = snapshot(&source);
            assert_refused(&source, &to, &before);

            drop(server_meta);
        }
    });

    assert_backup_verified(&source, &to, &snapshot(&source));
}

#[test]
fn offline_backup_refused_while_live_shamir_db_holds_catalogue_then_allowed_after_drop() {
    let data = TempDir::new().expect("tempdir");
    let dest = TempDir::new().expect("tempdir");
    let to = dest.path().join("backups");
    let source = data.path().to_path_buf();

    on_big_stack({
        let source = source.clone();
        let to = to.clone();
        move || async move {
            let db = tokio::time::timeout(
                Duration::from_secs(60),
                Box::pin(ShamirDb::init(SystemStoreConfig::Fjall(
                    source.join("shamir_db_meta.redb"),
                ))),
            )
            .await
            .expect("fixture init must not hang")
            .expect("open live ShamirDb");
            assert!(source.join("shamir_db_meta.redb/lock").exists());

            let before = snapshot(&source);
            assert_refused(&source, &to, &before);

            drop(db);
        }
    });

    assert_backup_verified(&source, &to, &snapshot(&source));
}

#[test]
fn offline_backup_refused_while_only_the_maintenance_lock_is_held() {
    let data = TempDir::new().expect("tempdir");
    let dest = TempDir::new().expect("tempdir");
    let to = dest.path().join("backups");
    let source = data.path().to_path_buf();

    let maintenance = maintenance_lock::acquire(&source).expect("acquire .shamir.lock");
    let before = snapshot(&source);
    assert_refused(&source, &to, &before);
    drop(maintenance);
}
