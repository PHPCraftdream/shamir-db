//! Boot-level format-stamp tests (design 6.2, server level): creation,
//! D6 stability, refusals with an unchanged directory, adoption.

use std::fs;
use std::time::Duration;

use shamir_sync::format_stamp::{read_header, StampError, STAMP_FILE_NAME};

use crate::server::BootError;

use super::support::{
    add_data_entry, compiled_current_epoch, launcher, write_same_version_stamp, DirSnapshot,
};

async fn launch_and_shutdown(dir: &std::path::Path) {
    let handle = launcher(dir).launch().await.expect("server launch");
    let report = tokio::time::timeout(Duration::from_secs(30), handle.shutdown())
        .await
        .expect("server shutdown bounded");
    assert!(report.is_clean(), "{report:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_creates_stamp_in_empty_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    let handle = launcher(&dir).launch().await.expect("server launch");
    assert_eq!(
        handle.format_stamp(),
        shamir_sync::format_stamp::StampOutcome::Created
    );
    let report = tokio::time::timeout(Duration::from_secs(30), handle.shutdown())
        .await
        .expect("server shutdown bounded");
    assert!(report.is_clean(), "{report:?}");
    let bytes = fs::read(dir.join(STAMP_FILE_NAME)).expect("stamp readable");
    let (_, epoch) = read_header(&bytes).expect("stamp header");
    assert_eq!(epoch, compiled_current_epoch());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_second_start_leaves_stamp_bytes_and_mtime_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    launch_and_shutdown(&dir).await;
    let stamp = dir.join(STAMP_FILE_NAME);
    let before_bytes = fs::read(&stamp).expect("stamp bytes");
    let before_mtime = fs::metadata(&stamp)
        .expect("stamp metadata")
        .modified()
        .expect("stamp mtime");
    launch_and_shutdown(&dir).await;
    let after_bytes = fs::read(&stamp).expect("stamp bytes");
    let after_mtime = fs::metadata(&stamp)
        .expect("stamp metadata")
        .modified()
        .expect("stamp mtime");
    assert_eq!(before_bytes, after_bytes);
    assert_eq!(before_mtime, after_mtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_refuses_newer_epoch_and_writes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    fs::create_dir_all(&dir).expect("data dir");
    add_data_entry(&dir);
    write_same_version_stamp(&dir, u64::MAX);
    let before = DirSnapshot::take(&dir);
    let err = match launcher(&dir).launch().await {
        Err(e) => e,
        Ok(_) => panic!("expected a format-stamp refusal"),
    };
    assert!(
        matches!(err, BootError::FormatStamp(StampError::NewerEpoch { .. })),
        "unexpected error: {err:?}"
    );
    before.assert_unchanged(&dir);
    // Refused before any store open: none of the boot stores exist.
    assert!(!dir.join("users").exists());
    assert!(!dir.join("counters").exists());
    assert!(!dir.join("audit.log").exists());
    assert!(!dir.join("shamir_db_meta.redb").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_refuses_corrupt_stamp_with_data_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    fs::create_dir_all(&dir).expect("data dir");
    add_data_entry(&dir);
    fs::write(dir.join(STAMP_FILE_NAME), b"not a stamp").expect("write junk stamp");
    let before = DirSnapshot::take(&dir);
    let err = match launcher(&dir).launch().await {
        Err(e) => e,
        Ok(_) => panic!("expected a format-stamp refusal"),
    };
    assert!(
        matches!(err, BootError::FormatStamp(StampError::Corrupt { .. })),
        "unexpected error: {err:?}"
    );
    before.assert_unchanged(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_adopts_stampless_existing_dir_and_marks_adopted() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();
    fs::create_dir_all(&dir).expect("data dir");
    add_data_entry(&dir);
    let handle = launcher(&dir).launch().await.expect("server launch");
    assert_eq!(
        handle.format_stamp(),
        shamir_sync::format_stamp::StampOutcome::Adopted
    );
    let report = tokio::time::timeout(Duration::from_secs(30), handle.shutdown())
        .await
        .expect("server shutdown bounded");
    assert!(report.is_clean(), "{report:?}");
    assert!(dir.join(STAMP_FILE_NAME).exists(), "stamp written");
}
