//! Decision-table rows 7-12, 19, 20 and 21 (design 3.4, 6.2).

use crate::format_stamp::check::{check_data_dir, StampMode, StampOutcome};
use crate::format_stamp::io::IgnoreSet;
use crate::format_stamp::model::{StampError, BASE_FORMAT_EPOCH};

use crate::format_stamp::tests::support::stamp_fixture::{
    add_data, add_leftover_tmp, add_lock, policy, temp_dir, write_same_version_stamp_with_epoch,
    write_stamp_with_epoch, DirSnapshot,
};

fn assert_refused(dir: &std::path::Path, current: u64) -> StampError {
    let before = DirSnapshot::take(dir);
    let err = check_data_dir(
        dir,
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(current, 1),
    )
    .expect_err("refused");
    before.assert_unchanged(dir);
    err
}

#[test]
fn table_row7_present_empty_equal_ok() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_same_version_stamp_with_epoch(dir.path(), 1);
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("row 7 ok");
    assert_eq!(outcome, StampOutcome::Unchanged);
}

#[test]
fn table_row8_present_empty_newer_refuses_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 5);
    let err = assert_refused(dir.path(), 1);
    assert!(
        matches!(
            err,
            StampError::NewerEpoch {
                found: 5,
                supported: 1,
                ..
            }
        ),
        "got {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("format epoch 5") && message.contains("Use a binary that supports"),
        "message: {message}"
    );
}

#[test]
fn table_row9_present_empty_older_refuses_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    let err = assert_refused(dir.path(), 2);
    assert!(matches!(err, StampError::OlderEpoch { .. }), "got {err}");
    let message = err.to_string();
    assert!(
        message.contains("requires epoch 2") && message.contains("format-migration"),
        "message: {message}"
    );
}

#[test]
fn table_row10_present_data_equal_ok() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    write_same_version_stamp_with_epoch(dir.path(), 1);
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("row 10 ok");
    assert_eq!(outcome, StampOutcome::Unchanged);
}

#[test]
fn table_row11_present_data_newer_refuses_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    write_stamp_with_epoch(dir.path(), 9);
    let err = assert_refused(dir.path(), 1);
    assert!(
        matches!(err, StampError::NewerEpoch { found: 9, .. }),
        "got {err}"
    );
}

#[test]
fn table_row12_present_data_older_refuses_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    let err = assert_refused(dir.path(), 2);
    assert!(matches!(err, StampError::OlderEpoch { .. }), "got {err}");
}

#[test]
fn table_row19_newer_layout_newer_epoch_refuses() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    // A layout-2 frame with header epoch 5; the epoch comes from the header.
    let body = r#"{ "future": true }"#;
    let frame = frame_layout2(body, 5);
    std::fs::write(dir.path().join("format.stamp"), &frame).expect("write layout2");
    let before = DirSnapshot::take(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect_err("row 19 refuses");
    assert!(
        matches!(
            err,
            StampError::NewerEpoch {
                found: 5,
                supported: 1,
                ..
            }
        ),
        "got {err}"
    );
    before.assert_unchanged(dir.path());
}

#[test]
fn table_row20_newer_layout_equal_epoch_refuses() {
    let dir = temp_dir();
    add_lock(dir.path());
    let body = r#"{ "future": true }"#;
    let frame = frame_layout2(body, 1);
    std::fs::write(dir.path().join("format.stamp"), &frame).expect("write layout2");
    let before = DirSnapshot::take(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect_err("row 20 refuses");
    assert!(
        matches!(
            err,
            StampError::NewerLayout {
                layout: 2,
                epoch: 1,
                ..
            }
        ),
        "got {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("layout 2") && message.contains("cannot read completely"),
        "message: {message}"
    );
    before.assert_unchanged(dir.path());
}

#[test]
fn table_row21_present_leftover_tmp_deletes_then_applies() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_same_version_stamp_with_epoch(dir.path(), BASE_FORMAT_EPOCH);
    add_leftover_tmp(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("row 21 proceeds");
    assert_eq!(outcome, StampOutcome::Unchanged);
    assert!(
        !dir.path().join("format.stamp.tmp").exists(),
        "stale tmp deleted by the writer"
    );
}

#[test]
fn table_row21_refusal_leaves_tmp_in_place() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    add_leftover_tmp(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(2, 1),
    )
    .expect_err("older epoch refuses");
    assert!(matches!(err, StampError::OlderEpoch { .. }), "got {err}");
    assert!(
        dir.path().join("format.stamp.tmp").exists(),
        "a refused run leaves the tmp in place"
    );
}

#[test]
fn table_row21_inspect_never_deletes_tmp() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    add_leftover_tmp(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Inspect,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("row 21 inspect allows");
    assert_eq!(outcome, StampOutcome::Unchanged);
    assert!(
        dir.path().join("format.stamp.tmp").exists(),
        "inspect never deletes"
    );
}

/// Layout-2 frame: valid header, unknown body, correct checksum.
fn frame_layout2(body: &str, epoch: u64) -> Vec<u8> {
    let body = body.as_bytes();
    let mut frame = Vec::new();
    frame.extend_from_slice(b"SHFS");
    frame.extend_from_slice(&2u16.to_le_bytes());
    frame.extend_from_slice(&epoch.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    frame.extend_from_slice(&[0u8; 32]);
    frame
}
