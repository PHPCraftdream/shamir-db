//! Corruption, truncation, trailing-data, over-limit and unsupported-feature
//! cases for the logical-dump container. Every case asserts the error's
//! byte-offset/sequence/section context.

use std::io::Cursor;

use sha2::{Digest, Sha256};

use super::super::super::codec::{decode_header, encode_canonical, encode_header};
use super::super::super::content::PortableItem;
use super::super::super::header::LogicalDumpHeader;
use super::super::super::section::{SectionKind, SectionRecord};
use super::super::super::trailer::{Trailer, TrailerSection};
use super::fixture::{build_fixture, fixture_header, walk_frames};
use crate::replication_seed::artifact::logical_dump::stream::verify_dump;

const FRAME_LIMIT: usize = 16 * 1024 * 1024;

fn opens(bytes: &[u8]) -> Result<(), String> {
    verify_dump(Cursor::new(bytes.to_vec()))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn assert_context(message: &str) {
    assert!(
        message.contains("byte offset") && message.contains("frame sequence"),
        "error lacks byte-offset/sequence context: {message}"
    );
}

fn expect_error(bytes: &[u8], needle: &str) -> String {
    let message = opens(bytes).expect_err("corrupt artifact must be rejected");
    assert!(
        message.contains(needle),
        "expected \"{needle}\" in: {message}"
    );
    assert_context(&message);
    message
}

/// Re-encode one frame with a replaced payload, preserving its sequence. Any
/// later frames shift, but the reader consumes the stream linearly.
fn replace_frame_payload(bytes: &mut Vec<u8>, frame_start: usize, sequence: u64, payload: &[u8]) {
    let old_len = 4
        + u32::from_be_bytes(
            bytes[frame_start..frame_start + 4]
                .try_into()
                .expect("length"),
        ) as usize
        + 32;
    let mut wire = Vec::with_capacity(payload.len() + 36);
    wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    wire.extend_from_slice(payload);
    let mut checksum = Sha256::new();
    checksum.update(sequence.to_be_bytes());
    checksum.update((payload.len() as u32).to_be_bytes());
    checksum.update(payload);
    wire.extend_from_slice(&checksum.finalize());
    bytes.splice(frame_start..frame_start + old_len, wire);
}

#[test]
fn wrong_magic_is_rejected_at_offset_zero() {
    let mut bytes = build_fixture();
    bytes[0] = b'X';
    let message = expect_error(&bytes, "invalid logical-dump magic");
    assert!(message.contains("offset 0"), "unexpected: {message}");
}

#[test]
fn wrong_version_is_rejected() {
    let mut bytes = build_fixture();
    bytes[8] ^= 0xff;
    expect_error(&bytes, "unsupported logical-dump version");
}

#[test]
fn unsupported_flags_are_rejected() {
    let mut bytes = build_fixture();
    bytes[10] = 1;
    expect_error(&bytes, "unsupported logical-dump flags");
}

#[test]
fn zero_header_length_is_rejected() {
    let mut bytes = build_fixture();
    bytes[12..16].copy_from_slice(&0_u32.to_be_bytes());
    expect_error(&bytes, "header length is outside limits");
}

#[test]
fn over_limit_header_length_is_rejected_before_allocation() {
    let mut bytes = build_fixture();
    bytes[12..16].copy_from_slice(&((FRAME_LIMIT + 1) as u32).to_be_bytes());
    bytes.truncate(20);
    expect_error(&bytes, "header length is outside limits");
}

#[test]
fn over_16mib_frame_length_is_rejected_before_allocation() {
    let mut bytes = build_fixture();
    let (frames_start, _) = walk_frames(&bytes);
    bytes[frames_start..frames_start + 4]
        .copy_from_slice(&((FRAME_LIMIT + 1) as u32).to_be_bytes());
    bytes.truncate(frames_start + 8);
    expect_error(&bytes, "frame length is outside limits");
}

#[test]
fn zero_frame_length_is_rejected() {
    let mut bytes = build_fixture();
    let (frames_start, _) = walk_frames(&bytes);
    bytes[frames_start..frames_start + 4].copy_from_slice(&0_u32.to_be_bytes());
    expect_error(&bytes, "frame length is outside limits");
}

#[test]
fn truncated_header_is_reported_with_offset() {
    let mut bytes = build_fixture();
    bytes.truncate(20);
    let message = expect_error(&bytes, "seed I/O");
    assert!(message.contains("offset 16"), "unexpected: {message}");
}

#[test]
fn flipped_header_byte_fails_the_header_checksum() {
    let mut bytes = build_fixture();
    bytes[17] ^= 0x01;
    let header_length = u32::from_be_bytes(bytes[12..16].try_into().unwrap()) as u64;
    let message = expect_error(&bytes, "header checksum mismatch");
    assert!(
        message.contains(&format!("offset {}", 16 + header_length)),
        "unexpected: {message}"
    );
}

#[test]
fn truncated_mid_frame_is_reported_with_offset_and_sequence() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    bytes.truncate(frames[2].start + 10);
    let message = expect_error(&bytes, "seed I/O");
    assert!(
        message.contains(&format!("offset {}", frames[2].start)),
        "unexpected: {message}"
    );
    assert!(message.contains("sequence 2"), "unexpected: {message}");
}

#[test]
fn flipped_frame_payload_byte_fails_the_frame_checksum() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    let payload_at = frames[0].start + 4;
    bytes[payload_at] ^= 0x01;
    let message = expect_error(&bytes, "frame checksum mismatch");
    assert!(message.contains("sequence 0"), "unexpected: {message}");
}

#[test]
fn trailing_byte_after_trailer_is_rejected() {
    let mut bytes = build_fixture();
    bytes.push(0);
    expect_error(&bytes, "trailing bytes after logical-dump trailer");
}

#[test]
fn truncated_trailer_frame_is_reported_at_its_offset() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    let last = frames.last().expect("trailer frame present");
    bytes.truncate(last.start + 4);
    let message = expect_error(&bytes, "seed I/O");
    assert!(
        message.contains(&format!("offset {}", last.start)),
        "unexpected: {message}"
    );
}

#[test]
fn section_end_count_mismatch_is_rejected() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    // Frame 2 is the Identities End record; claim a wrong observed count.
    let payload = encode_canonical(&SectionRecord::<PortableItem>::End {
        section: SectionKind::Identities,
        actual_items: 9,
        digest: [0; 32],
    })
    .expect("End record encodes");
    replace_frame_payload(&mut bytes, frames[2].start, 2, &payload);
    expect_error(&bytes, "section end count differs from observed item count");
}

#[test]
fn section_digest_mismatch_is_rejected() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    let payload = encode_canonical(&SectionRecord::<PortableItem>::End {
        section: SectionKind::Identities,
        actual_items: 1,
        digest: [7; 32],
    })
    .expect("End record encodes");
    replace_frame_payload(&mut bytes, frames[2].start, 2, &payload);
    expect_error(&bytes, "section item digest mismatch");
}

#[test]
fn trailer_totals_mismatch_is_rejected() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    let trailer_frame = frames.last().expect("trailer frame present");
    let payload = encode_canonical(&Trailer {
        preceding_frames: 999,
        preceding_items: 13,
        preceding_sections: 12,
        sections: Vec::new(),
        preceding_bytes: 0,
        prefix_sha256: [0; 32],
    })
    .expect("trailer encodes");
    replace_frame_payload(&mut bytes, trailer_frame.start, 37, &payload);
    expect_error(&bytes, "trailer totals or prefix digest mismatch");
}

#[test]
fn trailer_section_reports_mismatch_is_rejected() {
    let mut bytes = build_fixture();
    let (_, frames) = walk_frames(&bytes);
    let trailer_frame = frames.last().expect("trailer frame present");
    let payload = encode_canonical(&Trailer {
        preceding_frames: 37,
        preceding_items: 13,
        preceding_sections: 12,
        sections: vec![TrailerSection {
            section: SectionKind::Identities,
            items: 0,
            digest: [0; 32],
        }],
        preceding_bytes: 0,
        prefix_sha256: [0; 32],
    })
    .expect("trailer encodes");
    replace_frame_payload(&mut bytes, trailer_frame.start, 37, &payload);
    expect_error(&bytes, "trailer totals or prefix digest mismatch");
}

#[test]
fn unknown_required_feature_fails_closed() {
    let mut header = fixture_header();
    header.required_features = vec!["base".into()];
    let bytes = encode_header(&header, &["base"]).expect("supported header encodes");
    assert!(decode_header(&bytes, &["base"]).is_ok());
    let error = decode_header(&bytes, &[]).expect_err("unknown feature");
    assert!(
        error
            .to_string()
            .contains("unknown required logical-dump feature"),
        "unexpected: {error}"
    );
}

#[test]
fn unsupported_format_version_fails_closed() {
    let mut header: LogicalDumpHeader = fixture_header();
    header.format_version = 2;
    let error = encode_header(&header, &[]).expect_err("version 2 unsupported");
    assert!(
        error
            .to_string()
            .contains("unsupported logical-dump format version"),
        "unexpected: {error}"
    );
}
