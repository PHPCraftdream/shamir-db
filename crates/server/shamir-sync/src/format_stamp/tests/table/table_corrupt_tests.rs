//! Decision-table row 18: one test per corruption kind (design 3.4, 6.2).

use crate::format_stamp::check::{check_data_dir, StampMode};
use crate::format_stamp::io::IgnoreSet;
use crate::format_stamp::model::{CorruptKind, StampError};

use crate::format_stamp::tests::support::stamp_fixture::{
    add_data, policy, temp_dir, valid_frame, with_header_epoch, write_raw_stamp, DirSnapshot,
};

fn assert_corrupt_refused(frame: &[u8], expected: CorruptKind) {
    let dir = temp_dir();
    write_raw_stamp(dir.path(), frame);
    add_data(dir.path());
    let before = DirSnapshot::take(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect_err("row 18 refuses");
    assert!(
        matches!(
            err,
            StampError::Corrupt { ref kind, .. } if *kind == expected
        ),
        "expected {expected}, got {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("is corrupt") && message.contains("Restore a backup"),
        "message: {message}"
    );
    before.assert_unchanged(dir.path());
}

#[test]
fn table_row18_corrupt_short_file_refuses() {
    let frame = valid_frame(1);
    assert_corrupt_refused(&frame[..10], CorruptKind::ShortFile);
}

#[test]
fn table_row18_corrupt_bad_magic_refuses() {
    let mut frame = valid_frame(1);
    frame[1] ^= 0xFF;
    assert_corrupt_refused(&frame, CorruptKind::BadMagic);
}

#[test]
fn table_row18_corrupt_truncated_refuses() {
    let mut frame = valid_frame(1);
    frame[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_corrupt_refused(&frame, CorruptKind::Truncated);
}

#[test]
fn table_row18_corrupt_bad_checksum_refuses() {
    let mut frame = valid_frame(1);
    let last = frame.len() - 1;
    frame[last] ^= 0x01;
    assert_corrupt_refused(&frame, CorruptKind::BadChecksum);
}

#[test]
fn table_row18_corrupt_bad_body_refuses() {
    let mut frame = valid_frame(1);
    frame[18] = b'!';
    // Recompute the checksum so the failure is the body, not the trailer.
    let frame = with_header_epoch(frame, 1);
    assert_corrupt_refused(&frame, CorruptKind::BadBody);
}

#[test]
fn table_row18_corrupt_epoch_mismatch_refuses() {
    let frame = with_header_epoch(valid_frame(1), 7);
    assert_corrupt_refused(&frame, CorruptKind::EpochMismatch);
}
