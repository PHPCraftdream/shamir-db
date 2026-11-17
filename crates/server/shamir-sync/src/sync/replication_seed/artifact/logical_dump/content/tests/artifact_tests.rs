//! Full-artifact tests: byte determinism, round trip, frame overhead,
//! empty-End enforcement and the 1 TiB preflight.

use std::io::Cursor;

use sha2::{Digest, Sha256};

use super::super::super::content::{LogicalValue, PortableItem, RecordItem};
use super::super::super::section::SectionKind;
use super::super::super::stream::{verify_dump, DumpWriter};
use super::fixture::{
    build_fixture, fixture_header, fixture_items, item_record_payload, read_fixture_items,
    walk_frames,
};
use crate::replication_seed::preflight_artifact_size;

#[test]
fn fixture_is_byte_deterministic() {
    let first = build_fixture();
    let second = build_fixture();
    assert_eq!(first, second, "same fixture bytes must be produced twice");
}

#[test]
fn fixture_round_trips_every_item_in_order() {
    let bytes = build_fixture();
    let (header, items) = read_fixture_items(&bytes);
    let expected_header = fixture_header();
    assert_eq!(header, expected_header, "header survives the round trip");
    let expected: Vec<PortableItem> = fixture_items()
        .into_iter()
        .flat_map(|(_, group)| group)
        .collect();
    assert_eq!(items, expected, "all items surface in order");

    let report = verify_dump(Cursor::new(bytes)).expect("fixture verifies");
    assert_eq!(report.items, 13);
    assert_eq!(report.sections, 12);
    assert_eq!(report.frames, 38);
    assert_eq!(report.header, expected_header);
}

#[test]
fn item_frame_overhead_is_exactly_36_bytes() {
    let bytes = build_fixture();
    let identity = match &fixture_items()[0].1[0] {
        item @ PortableItem::Identity(_) => item.clone(),
        _ => unreachable!("first fixture item is an identity"),
    };
    let payload = item_record_payload(&identity);
    let sequence = 1_u64; // identities Start is frame 0, its item is frame 1
    let mut wire = Vec::with_capacity(payload.len() + 36);
    wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    wire.extend_from_slice(&payload);
    let mut checksum = Sha256::new();
    checksum.update(sequence.to_be_bytes());
    checksum.update((payload.len() as u32).to_be_bytes());
    checksum.update(&payload);
    wire.extend_from_slice(&checksum.finalize());
    assert_eq!(wire.len(), payload.len() + 36, "4 length + 32 checksum");
    assert!(
        bytes.windows(wire.len()).any(|window| window == wire),
        "the identities item frame appears on the wire with 36-byte overhead"
    );
}

#[test]
fn end_section_rejects_nonzero_declared_count() {
    let mut header = fixture_header();
    for entry in &mut header.sections {
        if entry.section == SectionKind::End {
            entry.expected_items = Some(1);
        }
    }
    let mut bytes = Vec::new();
    let mut writer = DumpWriter::new(&mut bytes, header.clone()).expect("preamble writes");
    let error = writer
        .start_section(SectionKind::End)
        .expect_err("End section must be empty");
    assert!(
        error.to_string().contains("End section"),
        "unexpected error: {error}"
    );

    let mut reader_bytes = Vec::new();
    let writer = DumpWriter::new(&mut reader_bytes, fixture_header()).expect("preamble writes");
    drop(writer);
    let error = verify_dump(Cursor::new(reader_bytes)).expect_err("empty artifact rejected");
    assert!(
        error.to_string().contains("byte offset"),
        "unexpected error: {error}"
    );
}

#[test]
fn writer_rejects_wrong_section_item() {
    let mut bytes = Vec::new();
    let mut writer = DumpWriter::new(&mut bytes, fixture_header()).expect("preamble writes");
    writer
        .start_section(SectionKind::Identities)
        .expect("section starts");
    let record = PortableItem::Record(RecordItem {
        repository: "app/main".into(),
        table: "accounts".into(),
        raw_key: vec![1],
        value: LogicalValue::Null,
    });
    let error = writer
        .write_item(&record)
        .expect_err("record in identities section");
    assert!(
        error.to_string().contains("does not match its section"),
        "unexpected error: {error}"
    );
}

#[test]
fn preflight_rejects_one_tebibyte_plus_one_without_disk() {
    let limit: u64 = 1024 * 1024 * 1024 * 1024;
    assert!(
        preflight_artifact_size(limit).is_ok(),
        "1 TiB exactly is accepted"
    );
    let error = preflight_artifact_size(limit + 1).expect_err("1 TiB + 1 is rejected");
    assert!(
        error.to_string().contains("1 TiB"),
        "unexpected error: {error}"
    );
}

#[test]
fn fixture_frame_walk_reaches_exact_eof() {
    let bytes = build_fixture();
    let (frames_start, frames) = walk_frames(&bytes);
    let report = verify_dump(Cursor::new(bytes.clone())).expect("fixture verifies");
    assert_eq!(
        frames.len(),
        report.frames as usize,
        "walker matches production frame count"
    );
    assert_eq!(frames.len(), 38);
    let total: usize = frames.iter().map(|frame| frame.wire_len).sum();
    assert_eq!(frames_start + total, bytes.len());
}
