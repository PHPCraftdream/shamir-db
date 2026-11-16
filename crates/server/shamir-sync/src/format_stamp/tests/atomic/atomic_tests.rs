//! Atomic write and torn-stamp tests (design 2.4, 6.2).

use std::fs;

use crate::format_stamp::check::{check_data_dir, StampMode, StampOutcome};
use crate::format_stamp::codec::{decode, STAMP_FILE_NAME, STAMP_TMP_NAME};
use crate::format_stamp::io::{inspect, write_stamp, IgnoreSet, StampState};
use crate::format_stamp::model::{FormatStamp, StampError, CURRENT_FORMAT_EPOCH};

use crate::format_stamp::tests::support::stamp_fixture::{
    add_data, add_leftover_tmp, add_lock, temp_dir, write_stamp_with_epoch, DirSnapshot,
};

fn stamp(version: &str) -> FormatStamp {
    FormatStamp::new(version, 1_000)
}

#[test]
fn atomic_write_creates_and_read_back_verifies() {
    let dir = temp_dir();
    write_stamp(dir.path(), &stamp("0.1.0")).expect("write");
    let bytes = fs::read(dir.path().join(STAMP_FILE_NAME)).expect("read");
    let decoded = decode(&bytes, dir.path()).expect("read back verifies");
    assert_eq!(decoded.format_epoch(), CURRENT_FORMAT_EPOCH);
    assert!(!dir.path().join(STAMP_TMP_NAME).exists());
}

#[test]
fn atomic_write_replaces_existing_stamp() {
    let dir = temp_dir();
    write_stamp_with_epoch(dir.path(), 1);
    write_stamp(dir.path(), &FormatStamp::with_epoch(1, "0.2.0", 2_000)).expect("replace");
    let bytes = fs::read(dir.path().join(STAMP_FILE_NAME)).expect("read");
    let decoded = decode(&bytes, dir.path()).expect("decode");
    assert_eq!(decoded.created_by().version, "0.2.0");
    assert!(!dir.path().join(STAMP_TMP_NAME).exists());
}

#[test]
fn atomic_write_replace_after_reader_closed_handle() {
    let dir = temp_dir();
    write_stamp_with_epoch(dir.path(), 1);
    // Open and close the stamp like a reader must; the closed handle must
    // not block the replacement (Windows sharing-violation guard).
    {
        let bytes = fs::read(dir.path().join(STAMP_FILE_NAME)).expect("read");
        assert!(!bytes.is_empty());
    }
    write_stamp(dir.path(), &FormatStamp::with_epoch(1, "0.3.0", 3_000))
        .expect("replace after reader closed");
}

#[test]
fn interrupted_replacement_keeps_old_stamp() {
    let dir = temp_dir();
    write_stamp_with_epoch(dir.path(), 1);
    // A complete tmp, no rename: the old stamp is the one trusted.
    add_leftover_tmp(dir.path());
    match inspect(dir.path()) {
        StampState::Valid(s) => assert_eq!(s.format_epoch(), 1),
        other => panic!("expected the old valid stamp, got {other:?}"),
    }
    // The next Writer check deletes the stale tmp.
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &crate::format_stamp::model::EpochPolicy::compiled(),
    )
    .expect("proceeds");
    assert_eq!(outcome, StampOutcome::Unchanged);
    assert!(!dir.path().join(STAMP_TMP_NAME).exists());
}

#[test]
fn torn_first_stamp_tmp_only_with_data_refuses() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    add_leftover_tmp(dir.path());
    let before = DirSnapshot::take(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &crate::format_stamp::model::EpochPolicy::compiled(),
    )
    .expect_err("row 6 refuses");
    assert!(
        matches!(
            err,
            StampError::Corrupt {
                kind: crate::format_stamp::model::CorruptKind::TornTmp,
                ..
            }
        ),
        "got {err}"
    );
    before.assert_unchanged(dir.path());
}

#[test]
fn tmp_only_empty_dir_creates_stamp() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_leftover_tmp(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &crate::format_stamp::model::EpochPolicy::compiled(),
    )
    .expect("row 3 with leftover tmp");
    assert_eq!(outcome, StampOutcome::Created);
    assert!(dir.path().join(STAMP_FILE_NAME).exists());
    assert!(!dir.path().join(STAMP_TMP_NAME).exists());
}

#[cfg(unix)]
#[test]
fn dir_fsync_error_propagates() {
    // The strict directory fsync must fail on a directory that is gone,
    // and `write_stamp` propagates that error (fail closed, never a
    // warning).
    let dir = temp_dir();
    let path = dir.path().to_path_buf();
    drop(dir);
    let err = crate::format_stamp::io::atomic_write::fsync_dir(&path)
        .expect_err("fsync of a removed directory fails");
    assert!(err.kind() == std::io::ErrorKind::NotFound, "got {err:?}");
}

#[cfg(windows)]
#[test]
fn dir_fsync_noop_on_windows_does_not_fail_write() {
    // No directory flush exists on Windows; the best-effort file re-sync
    // must not fail the write.
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp(dir.path(), &stamp("0.1.0")).expect("write succeeds without dir fsync");
}

#[test]
fn stamp_is_first_file_written_in_empty_dir() {
    let dir = temp_dir();
    add_lock(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &crate::format_stamp::model::EpochPolicy::compiled(),
    )
    .expect("creates");
    assert_eq!(outcome, StampOutcome::Created);
    let mut names: Vec<_> = fs::read_dir(dir.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![".shamir.lock".to_string(), "format.stamp".to_string()]
    );
}
