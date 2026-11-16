//! Decision-table rows 3, 5a, 5b, 6 and 15 (design 3.4, 6.2).

use crate::format_stamp::check::{check_data_dir, StampMode, StampOutcome};
use crate::format_stamp::io::IgnoreSet;
use crate::format_stamp::model::{
    CorruptKind, StampError, BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH,
};

use crate::format_stamp::tests::support::stamp_fixture::{
    add_data, add_leftover_tmp, add_lock, policy, temp_dir, write_raw_stamp,
    write_stamp_with_epoch, DirSnapshot,
};

fn writer() -> (StampMode, IgnoreSet) {
    (StampMode::Writer, IgnoreSet::none())
}

#[test]
fn table_row3_absent_empty_creates_stamp() {
    let dir = temp_dir();
    add_lock(dir.path());
    let before = DirSnapshot::take(dir.path());
    let (mode, ignore) = writer();
    let outcome = check_data_dir(
        dir.path(),
        mode,
        &ignore,
        &policy(CURRENT_FORMAT_EPOCH, BASE_FORMAT_EPOCH),
    )
    .expect("row 3 creates");
    assert_eq!(outcome, StampOutcome::Created);
    // The stamp is the only new entry; the lock file pre-existed.
    let after = DirSnapshot::take(dir.path());
    assert_eq!(after.len(), before.len() + 1);
    assert!(dir.path().join("format.stamp").exists());
}

#[test]
fn table_row5a_absent_data_adopts_base() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    let (mode, ignore) = writer();
    let outcome = check_data_dir(dir.path(), mode, &ignore, &policy(1, 1)).expect("row 5a adopts");
    assert_eq!(outcome, StampOutcome::Adopted);
    let stamp = crate::format_stamp::codec::decode(
        &std::fs::read(dir.path().join("format.stamp")).expect("read stamp"),
        dir.path(),
    )
    .expect("decode");
    let actor = stamp.created_by();
    assert_eq!(actor.adopted, Some(true));
    assert_eq!(stamp.format_epoch(), 1);
}

#[test]
fn table_row5b_absent_data_older_base_refuses_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    let before = DirSnapshot::take(dir.path());
    let (mode, ignore) = writer();
    let err = check_data_dir(dir.path(), mode, &ignore, &policy(2, 1)).expect_err("row 5b refuses");
    assert!(
        matches!(err, StampError::AbsentOlderBase { .. }),
        "got {err}"
    );
    assert!(
        err.to_string().contains("no format stamp")
            && err.to_string().contains("refusing to start"),
        "message: {err}"
    );
    before.assert_unchanged(dir.path());
}

#[test]
fn table_row6_leftover_tmp_data_refuses_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    add_leftover_tmp(dir.path());
    let before = DirSnapshot::take(dir.path());
    let (mode, ignore) = writer();
    let err = check_data_dir(dir.path(), mode, &ignore, &policy(1, 1)).expect_err("row 6 refuses");
    assert!(
        matches!(
            err,
            StampError::Corrupt {
                kind: CorruptKind::TornTmp,
                ..
            }
        ),
        "got {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("was interrupted") && message.contains("remove format.stamp.tmp"),
        "message: {message}"
    );
    // The tmp is never deleted by a refusal.
    before.assert_unchanged(dir.path());
}

#[test]
fn table_row15_corrupt_empty_replaced_and_evidence_kept() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_raw_stamp(dir.path(), b"not a stamp");
    let (mode, ignore) = writer();
    let outcome = check_data_dir(dir.path(), mode, &ignore, &policy(1, 1)).expect("row 15 writes");
    assert_eq!(outcome, StampOutcome::Created);
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("format.stamp.corrupt-")),
        "evidence kept: {names:?}"
    );
    assert!(names.contains(&"format.stamp".to_string()));
}

#[test]
fn table_row15_corrupt_empty_inspect_refuses() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    // Corrupt the file in place.
    std::fs::write(dir.path().join("format.stamp"), b"broken").expect("corrupt");
    let before = DirSnapshot::take(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Inspect,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect_err("row 15 inspect refuses");
    assert!(matches!(err, StampError::Corrupt { .. }), "got {err}");
    before.assert_unchanged(dir.path());
}
