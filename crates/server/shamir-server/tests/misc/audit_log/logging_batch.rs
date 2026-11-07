//! Tests for the batched file writer (`logging::batched_file_writer`).
//!
//! These test the standalone writer component directly — no global
//! subscriber involved — so they are fully deterministic.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use shamir_server::logging::batched_file_writer;

fn temp_log_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

fn read_log(path: &PathBuf) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Poll `path` until `cond(content)` holds or `timeout` elapses; returns the
/// last-read content either way.
///
/// The batched writer flushes from a BACKGROUND worker thread, so the visible
/// on-disk state is reached asynchronously. A single fixed `sleep` then assert
/// races that worker: under a saturated machine (the full `@e2e` run schedules
/// dozens of test processes at once) the worker may not have drained its
/// channel + flushed within a fixed window, so the assert sees a short file and
/// flakes — while the SAME test passes in isolation. Polling for the real
/// condition is deterministic-on-success: it waits exactly as long as the
/// worker needs, and only fails if the flush genuinely never happens (a real
/// bug) within the generous cap. This is NOT a masked timeout — the asserted
/// property is unchanged; only the "wait for async work" mechanism is fixed.
fn poll_log_until(path: &PathBuf, timeout: Duration, cond: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let contents = read_log(path);
        if cond(&contents) || Instant::now() >= deadline {
            return contents;
        }
        thread::sleep(Duration::from_millis(20));
    }
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

    // Poll for the timer flush (robust under parallel-test load — a fixed
    // sleep races the background worker). The 200 ms interval fires well
    // inside the 5 s cap on any non-pathological run.
    let contents = poll_log_until(&path, Duration::from_secs(5), |c| {
        c.contains("line one") && c.contains("line two") && c.contains("line three")
    });
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

    // Poll for the burst flush (robust under parallel-test load — a fixed
    // sleep races the background worker when the machine is saturated). The
    // burst-guard must have flushed ~one full buffer (≈256 KiB) to disk WELL
    // before the 30 s interval — proving the threshold path works
    // independently of the timer. The trailing partial buffer (< capacity)
    // stays in memory until the interval / shutdown, so we assert "most of
    // the burst landed", not "every last byte". 5 s cap is far inside the 30 s
    // interval, so a hit here is genuinely the threshold path, not the timer.
    let contents = poll_log_until(&path, Duration::from_secs(5), |c| c.len() >= 200 * 1024);
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
