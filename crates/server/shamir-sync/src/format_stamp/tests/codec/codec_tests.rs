//! Codec tests (design 6.2, fixture: in-memory byte vectors).

use sha2::{Digest, Sha256};
use std::path::Path;

use crate::format_stamp::codec::{decode, encode, read_header, HEADER_LEN, TRAILER_LEN};
use crate::format_stamp::model::{CorruptKind, FormatStamp, StampError, CURRENT_FORMAT_EPOCH};

use crate::format_stamp::tests::support::stamp_fixture::{
    valid_frame, with_header_epoch, FIXTURE_UNIX_MS, OTHER_VERSION,
};

fn fixture_dir() -> &'static Path {
    Path::new("fixture")
}

fn corrupt_kind_of(frame: &[u8]) -> CorruptKind {
    match decode(frame, fixture_dir()) {
        Err(StampError::Corrupt { kind, .. }) => kind,
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

/// Build a frame around an arbitrary body JSON (unknown keys, layouts).
fn frame_from_body(body: &str, layout_version: u16, epoch: u64) -> Vec<u8> {
    let body = body.as_bytes();
    let mut frame = Vec::new();
    frame.extend_from_slice(b"SHFS");
    frame.extend_from_slice(&layout_version.to_le_bytes());
    frame.extend_from_slice(&epoch.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    let digest = Sha256::digest(&frame);
    frame.extend_from_slice(&digest);
    frame
}

#[test]
fn codec_round_trip_preserves_all_fields() {
    let mut stamp = FormatStamp::with_epoch(CURRENT_FORMAT_EPOCH, OTHER_VERSION, FIXTURE_UNIX_MS);
    stamp = stamp.with_last_opened(OTHER_VERSION, FIXTURE_UNIX_MS + 1);
    let frame = encode(&stamp).expect("encode");
    let decoded = decode(&frame, fixture_dir()).expect("decode");
    assert_eq!(stamp, decoded);
    assert_eq!(decoded.format_epoch(), CURRENT_FORMAT_EPOCH);
    assert_eq!(decoded.created_by().version, OTHER_VERSION);
    assert_eq!(
        decoded.last_opened_by().expect("last_opened_by").unix_ms,
        FIXTURE_UNIX_MS + 1
    );
    assert!(decoded.last_format_migration().is_none());
    assert_eq!(
        decoded.components().get("wal").and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn codec_unknown_body_keys_preserved_on_rewrite() {
    let body = r#"{
        "format_epoch": 1,
        "components": {},
        "created_by": { "version": "0.1.0", "unix_ms": 1 },
        "future_key": { "nested": [1, 2, 3] }
    }"#;
    let frame = frame_from_body(body, 1, 1);
    let decoded = decode(&frame, fixture_dir()).expect("decode");
    assert!(
        decoded.extra().contains_key("future_key"),
        "unknown key must survive decode"
    );
    let reencoded = encode(&decoded).expect("re-encode");
    let rededcoded = decode(&reencoded, fixture_dir()).expect("re-decode");
    assert_eq!(decoded, rededcoded);
    assert!(rededcoded.extra().contains_key("future_key"));
}

#[test]
fn codec_bad_checksum_is_corrupt() {
    // Flip one bit in the header.
    let mut frame = valid_frame(1);
    frame[4] ^= 0x01;
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::BadChecksum);
    // Flip one bit in the body.
    let mut frame = valid_frame(1);
    let body_start = HEADER_LEN;
    frame[body_start] ^= 0x01;
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::BadChecksum);
    // Flip one bit in the trailer.
    let mut frame = valid_frame(1);
    let last = frame.len() - 1;
    frame[last] ^= 0x01;
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::BadChecksum);
}

#[test]
fn codec_short_file_is_corrupt() {
    let frame = valid_frame(1);
    for len in [0usize, 3, 17, 18, frame.len() - 33, frame.len() - 1] {
        let kind = corrupt_kind_of(&frame[..len]);
        assert!(
            matches!(
                kind,
                CorruptKind::ShortFile | CorruptKind::Truncated | CorruptKind::BadChecksum
            ),
            "len {len}: got {kind:?}"
        );
    }
}

#[test]
fn codec_bad_magic_is_corrupt() {
    let mut frame = valid_frame(1);
    frame[0] ^= 0xFF;
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::BadMagic);
}

#[test]
fn codec_body_len_beyond_file_is_corrupt() {
    let mut frame = valid_frame(1);
    frame[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::Truncated);
}

#[test]
fn codec_header_body_epoch_mismatch_is_corrupt() {
    let frame = with_header_epoch(valid_frame(1), 7);
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::EpochMismatch);
}

#[test]
fn codec_newer_layout_still_yields_epoch() {
    let body = r#"{ "future": true }"#;
    let frame = frame_from_body(body, 2, 5);
    let (layout, epoch) = read_header(&frame).expect("read_header");
    assert_eq!((layout, epoch), (2, 5));
    match decode(&frame, fixture_dir()) {
        Err(StampError::NewerLayout {
            layout: got_layout,
            epoch: got_epoch,
            ..
        }) => {
            assert_eq!((got_layout, got_epoch), (2, 5));
        }
        other => panic!("expected NewerLayout, got {other:?}"),
    }
}

#[test]
fn codec_epoch_below_base_is_corrupt() {
    let frame = valid_frame(0);
    assert_eq!(corrupt_kind_of(&frame), CorruptKind::BadBody);
}

#[test]
fn codec_frame_lengths_are_exact() {
    let body = r#"{
        "format_epoch": 1,
        "components": {},
        "created_by": { "version": "0.1.0", "unix_ms": 1 }
    }"#;
    let frame = frame_from_body(body, 1, 1);
    assert_eq!(frame.len(), HEADER_LEN + body.len() + TRAILER_LEN);
    assert!(decode(&frame, fixture_dir()).is_ok());
}
