//! Audit log rotation: when the active file crosses `max_size_bytes`,
//! it's renamed to `audit_log.jsonl.<unix_nanos>` and a fresh file is
//! opened for subsequent writes. The HMAC chain continues unbroken.

use std::time::Duration;

use shamir_connect::server::audit_chain::{AuditAppender, AuditEntry};
use shamir_server::audit_appender::RedbAuditAppender;
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
        RedbAuditAppender::open_strict_with_rotation(temp.path(), Some(1024)).unwrap();

    // Write enough entries to comfortably cross 1 KB. Each JSON line is
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
    assert!(active.exists(), "active audit.log must be reopened after rotation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_rotation_below_threshold() {
    let temp = TempDir::new().unwrap();
    // 1 MB threshold — well above what 5 entries write.
    let appender =
        RedbAuditAppender::open_strict_with_rotation(temp.path(), Some(1_000_000)).unwrap();
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
    let appender = RedbAuditAppender::open_strict_with_rotation(temp.path(), None).unwrap();
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
