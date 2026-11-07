//! Unit tests for the bounded ring: record capacity, line truncation,
//! eviction order, and relative rendering.

use std::time::{Duration, Instant};

use crate::ring::{render_records, DiagRing, RingRecord};

/// The record bound is respected: pushing past the cap evicts the OLDEST
/// records and counts them.
#[test]
fn ring_respects_the_record_bound() {
    let ring = DiagRing::new(8, 64);
    for i in 0..100 {
        ring.push("INFO", "ring_bound_tests", format!("record {i}"));
    }
    let records = ring.snapshot();
    assert_eq!(records.len(), 8);
    assert_eq!(ring.dropped_count(), 92);
    assert_eq!(
        records.first().map(|r| r.message.as_str()),
        Some("record 92")
    );
    assert_eq!(
        records.last().map(|r| r.message.as_str()),
        Some("record 99")
    );
}

/// Over-long messages are truncated to the line cap (plus a marker), so one
/// huge log line cannot blow up the report.
#[test]
fn ring_truncates_over_long_lines() {
    let ring = DiagRing::new(4, 32);
    ring.push("INFO", "line_cap_tests", "x".repeat(10_000));
    let records = ring.snapshot();
    assert_eq!(records.len(), 1);
    let message = &records[0].message;
    assert!(
        message.chars().count() <= 48,
        "line cap not applied: {message}"
    );
    assert!(
        message.contains("truncated"),
        "truncation marker missing: {message}"
    );
}

/// Records render oldest-first with elapsed seconds relative to the given
/// base instant.
#[test]
fn render_records_orders_by_insertion_relative_to_base() {
    let base = Instant::now();
    let records = vec![
        RingRecord {
            at: base + Duration::from_millis(10),
            level: "INFO".to_string(),
            target: "t".to_string(),
            message: "first".to_string(),
        },
        RingRecord {
            at: base + Duration::from_millis(20),
            level: "WARN".to_string(),
            target: "t".to_string(),
            message: "second".to_string(),
        },
    ];
    let text = render_records(&records, base);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].contains("+0.010s INFO t: first"),
        "first line: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("+0.020s WARN t: second"),
        "second line: {}",
        lines[1]
    );
}
