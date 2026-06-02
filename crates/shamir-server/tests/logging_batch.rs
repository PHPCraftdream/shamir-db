//! Tests for the batched file writer (`logging::batched_file_writer`).
//!
//! These test the standalone writer component directly — no global
//! subscriber involved — so they are fully deterministic.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use shamir_server::logging::batched_file_writer;

fn temp_log_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

fn read_log(path: &PathBuf) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Write 3 lines, wait longer than the flush interval, confirm all 3
/// appeared (timer flush).
#[test]
fn batched_writer_flushes_on_interval() {
    let dir = tempfile::tempdir().unwrap();
    let path = temp_log_path(&dir, "interval.log");
    let interval_ms = 200;

    let (mut writer, guard) = batched_file_writer(&path, interval_ms).unwrap();

    writer.write_all(b"line one\n").unwrap();
    writer.write_all(b"line two\n").unwrap();
    writer.write_all(b"line three\n").unwrap();

    // Sleep well past the flush interval so the timer fires.
    thread::sleep(Duration::from_millis(interval_ms * 3));

    // Lines must already be on disk (timer flush) while guard is alive.
    let contents = read_log(&path);
    assert!(contents.contains("line one"), "missing 'line one'");
    assert!(contents.contains("line two"), "missing 'line two'");
    assert!(contents.contains("line three"), "missing 'line three'");

    // Shutdown: guard drop signals the worker via a dedicated channel.
    drop(guard);
    drop(writer);
}

/// Write a line, drop the guard, confirm the line made it to disk
/// (shutdown drain — no loss).
#[test]
fn batched_writer_flushes_remaining_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = temp_log_path(&dir, "drop.log");

    let (mut writer, guard) = batched_file_writer(&path, 10_000).unwrap();

    writer.write_all(b"before drop\n").unwrap();

    // Guard drop triggers shutdown signal → worker drains + flushes.
    drop(guard);
    drop(writer);

    let contents = read_log(&path);
    assert!(
        contents.contains("before drop"),
        "line lost on drop: {contents:?}"
    );
}

/// Write more than the burst threshold (256 KiB) in one go, confirm the
/// file has content before the interval fires.
#[test]
fn batched_writer_threshold_flush() {
    let dir = tempfile::tempdir().unwrap();
    let path = temp_log_path(&dir, "burst.log");
    let interval_ms = 30_000; // 30 s — long enough that it won't fire.

    let (mut writer, guard) = batched_file_writer(&path, interval_ms).unwrap();

    // Write ~300 KiB of data — exceeds BURST_FLUSH_THRESHOLD (256 KiB).
    let big_line = format!("{}\n", "x".repeat(1024));
    let big_bytes = big_line.as_bytes();
    for _ in 0..300 {
        writer.write_all(big_bytes).unwrap();
    }

    // Give the worker a moment to process and flush (burst threshold).
    thread::sleep(Duration::from_millis(200));

    // The burst-guard must have flushed ~one full buffer (≈256 KiB) to disk
    // WELL before the 30 s interval — proving the threshold path works
    // independently of the timer. The trailing partial buffer (< capacity)
    // stays in memory until the interval / shutdown, so we assert "most of
    // the burst landed", not "every last byte".
    let contents = read_log(&path);
    assert!(
        !contents.is_empty(),
        "burst threshold did not trigger a flush"
    );
    assert!(
        contents.len() >= 200 * 1024,
        "expected a burst flush of ≳256 KiB before the interval, got {} bytes",
        contents.len()
    );

    drop(guard);
    drop(writer);
}
