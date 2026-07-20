//! Audit log rotation: when the active file crosses `max_size_bytes`,
//! it's renamed to `audit_log.log.<unix_nanos>` and a fresh file is
//! opened for subsequent writes. The HMAC chain continues unbroken.

use std::time::Duration;

use shamir_connect::server::audit_chain::{AuditAppender, AuditChain, AuditEntry};
use shamir_server::audit_appender::FjallAuditAppender;
use tempfile::TempDir;

fn make_entry(seq: u64) -> AuditEntry {
    AuditEntry {
        seq,
        ts_ns: 1_000_000_000 + seq,
        event: format!("evt_{seq:04}"),
        transport: "tcp".into(),
        user: "alice".into(),
        ip_subnet: "127.0.0.0/24".into(),
        session_id_prefix: [0u8; 8],
        result: "ok".into(),
        details_canonical_msgpack: vec![],
        prev_hmac: [0xAB; 32],
        hmac: [(seq & 0xff) as u8; 32],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_kicks_in_after_threshold() {
    let temp = TempDir::new().unwrap();
    // Tiny 1 KB threshold so the test only has to write a handful of entries.
    let appender =
        FjallAuditAppender::open_strict_with_rotation(temp.path(), Some(1024), 0).unwrap();

    // Write enough entries to comfortably cross 1 KB. Each log line is
    // ~250 bytes, so 10 entries = ~2.5 KB.
    for i in 0..10 {
        appender.append_entry(&make_entry(i));
    }

    // Wait a tick for any in-flight rotation rename to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Find rotated files: the active file is `audit.log` (constant from
    // crate); rotated files have names like `audit.log.<digits>`.
    let entries: Vec<_> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        !entries.is_empty(),
        "expected at least one rotated audit.log.* file, found {:?}",
        entries
    );

    // Active file should still be present (just freshly opened post-rotation).
    let active = temp.path().join("audit.log");
    assert!(
        active.exists(),
        "active audit.log must be reopened after rotation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_rotation_below_threshold() {
    let temp = TempDir::new().unwrap();
    // 1 MB threshold — well above what 5 entries write.
    let appender =
        FjallAuditAppender::open_strict_with_rotation(temp.path(), Some(1_000_000), 0).unwrap();
    for i in 0..5 {
        appender.append_entry(&make_entry(i));
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let rotated_count = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(rotated_count, 0, "no rotation expected below threshold");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_disabled_when_max_size_is_none() {
    let temp = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_strict_with_rotation(temp.path(), None, 0).unwrap();
    for i in 0..50 {
        appender.append_entry(&make_entry(i));
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let rotated_count = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(rotated_count, 0, "rotation must be off when None");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hmac_chain_is_intact_across_rotation() {
    // Critical security regression test: rotation must NOT break the
    // HMAC chain. After multiple rotations, reading every file in
    // chronological order must yield a chain that
    // `AuditChain::verify_chain` accepts (every prev_hmac matches the
    // previous entry's hmac, hmacs are recomputable from the chain key).

    let temp = TempDir::new().unwrap();
    let key = [0xC0u8; 32];
    let chain = AuditChain::new(key);
    // Tiny 1 KB threshold to force several rotations.
    let appender =
        FjallAuditAppender::open_strict_with_rotation(temp.path(), Some(1024), 0).unwrap();

    // Write 30 entries via the chain so each one carries a real
    // `prev_hmac → hmac` link (the make_entry helper above uses fake
    // hmacs and would fail verify_chain by design).
    for i in 0..30u64 {
        let entry = chain.append(
            "auth_success",
            "tcp",
            format!("user_{i:02}"),
            "127.0.0.0/24",
            [0u8; 8],
            "ok",
            Vec::new(),
            1_000_000_000 + i,
        );
        appender.append_entry(&entry);
    }
    // Brief pause so any rename/file-handle settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Sanity: there ARE rotated files (otherwise this test trivially
    // passes by reading just the active file).
    let rotated_count = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("audit.log."))
                .unwrap_or(false)
        })
        .count();
    assert!(
        rotated_count >= 1,
        "expected at least one rotation to actually exercise the chain check"
    );

    // The fix in `read_log_for_verify`: it must read rotated files
    // (lex-sorted) plus the active file, in chronological order.
    let entries =
        FjallAuditAppender::read_log_for_verify(temp.path()).expect("read_log_for_verify");
    assert_eq!(entries.len(), 30, "every entry survives across rotation");

    // The headline assertion: the chain still verifies after rotation.
    AuditChain::verify_chain(&key, &entries).expect("HMAC chain unbroken across rotation");
}

// ---------------------------------------------------------------------------
// Retention sweep tests (audit.retention_days).
//
// The sweep runs piggyback on rotation: when a new rotated file is created,
// sibling rotated files older than `retention_days` are deleted. File age is
// determined from the embedded `unix_nanos` suffix in the filename (same
// format `rotate_locked` produces), not from filesystem mtime.
// ---------------------------------------------------------------------------

fn now_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

const NANOS_PER_SEC: u64 = 1_000_000_000;
const SECS_PER_DAY: u64 = 86400;
const NANOS_PER_DAY: u64 = SECS_PER_DAY * NANOS_PER_SEC;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_deletes_old_rotated_files_only() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let old_ns = now - 10 * NANOS_PER_DAY; // 10 days ago
    let recent_ns = now - NANOS_PER_DAY; // 1 day ago

    // Pre-create rotated files with KNOWN embedded timestamps.
    std::fs::write(
        dir.join(format!("audit.log.{old_ns:020}")),
        b"old-should-be-deleted",
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("audit.log.{recent_ns:020}")),
        b"recent-should-survive",
    )
    .unwrap();

    // retention_days = 5: the 10-day-old file exceeds it, the 1-day-old does not.
    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 5).unwrap();

    // Trigger a rotation (1 KB threshold → ~10 entries suffice).
    for i in 0..10 {
        appender.append_entry(&make_entry(i));
    }

    // The old file is gone; the recent file survives.
    assert!(
        !dir.join(format!("audit.log.{old_ns:020}")).exists(),
        "old rotated file should be swept"
    );
    assert!(
        dir.join(format!("audit.log.{recent_ns:020}")).exists(),
        "recent rotated file should survive retention window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_zero_means_no_sweep() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let ancient_ns = now - 365 * NANOS_PER_DAY; // 1 year ago

    std::fs::write(
        dir.join(format!("audit.log.{ancient_ns:020}")),
        b"ancient-should-survive-when-retention-is-0",
    )
    .unwrap();

    // retention_days = 0: the documented off-switch — no sweep ever.
    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 0).unwrap();

    for i in 0..10 {
        appender.append_entry(&make_entry(i));
    }

    assert!(
        dir.join(format!("audit.log.{ancient_ns:020}")).exists(),
        "retention_days=0 must NOT sweep even very old files"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_failure_does_not_break_writes() {
    // A rotated-file-shaped entry that `remove_file` cannot delete (a
    // directory) must NOT propagate an error to the write/rotation caller.
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let old_ns = now - 100 * NANOS_PER_DAY; // 100 days ago — well past retention

    // Create a DIRECTORY whose name matches the rotated-file pattern.
    // `fs::remove_file` on a directory fails on both Unix and Windows.
    std::fs::create_dir(dir.join(format!("audit.log.{old_ns:020}"))).unwrap();

    let appender = FjallAuditAppender::open_strict_with_rotation(dir, Some(1024), 5).unwrap();

    // Trigger a rotation — the sweep encounters the undeleatable directory,
    // logs a warn, but the write path is unaffected.
    for i in 0..10 {
        appender.append_entry(&make_entry(i)); // must not panic
    }

    // Writes landed successfully despite the sweep failure: the active log
    // file has content. (Cannot use `read_log_for_verify` here because it
    // tries to open the directory-shaped entry as a file, which fails on
    // Windows — the assertion is about write-path resilience, not the chain.)
    let active_contents =
        std::fs::read_to_string(dir.join("audit.log")).expect("active audit.log must be readable");
    assert!(
        !active_contents.is_empty(),
        "entries must be written even when the retention sweep hits an error"
    );

    // The directory survives (remove_file failed).
    assert!(
        dir.join(format!("audit.log.{old_ns:020}")).exists(),
        "directory-shaped rotated entry survives failed remove_file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_sweep_runs_in_batched_mode_too() {
    // Regression: the batched-mode constructor must also thread
    // retention_days and sweep on rotation, not just strict mode.
    let temp = TempDir::new().unwrap();
    let dir = temp.path();

    let now = now_unix_nanos();
    let old_ns = now - 10 * NANOS_PER_DAY;

    std::fs::write(
        dir.join(format!("audit.log.{old_ns:020}")),
        b"old-should-be-deleted-in-batched-mode",
    )
    .unwrap();

    let appender = FjallAuditAppender::open_batched_with_rotation(
        dir,
        Duration::from_millis(50),
        Some(1024),
        5,
    )
    .unwrap();

    // Write enough to cross the 1 KB threshold.
    for i in 0..10 {
        appender.append_entry(&make_entry(i));
    }

    // Wait for the batched flusher to land the writes + trigger rotation.
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        !dir.join(format!("audit.log.{old_ns:020}")).exists(),
        "batched-mode retention sweep must delete old rotated files"
    );
}
