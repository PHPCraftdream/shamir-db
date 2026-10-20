//! Integration tests for the durable audit appender.
//!
//! Covers spec IMPL §3.3 NORMATIVE behaviours:
//! - strict-mode per-entry fsync
//! - checkpoint persistence + reload
//! - HMAC chain integrity across restart
//! - truncation detection via checkpoint mismatch
//! - batched-mode interval flushing
//! - shutdown drains pending entries

use std::time::Duration;

use shamir_connect::server::audit_chain::{AuditChain, AuditEntry, AuditError};
use shamir_server::audit_appender::FjallAuditAppender;
use tempfile::TempDir;

fn key() -> [u8; 32] {
    [0xa1u8; 32]
}

/// Append one synthetic entry to `chain` and return it.
fn append_event(chain: &AuditChain, idx: u64) -> AuditEntry {
    chain.append(
        "auth_success",
        "tcp",
        "alice",
        "192.0.2.0/24",
        [0u8; 8],
        "ok",
        vec![],
        1_000_000 + idx,
    )
}

fn entries_equal(a: &AuditEntry, b: &AuditEntry) {
    assert_eq!(a.seq, b.seq);
    assert_eq!(a.ts_ns, b.ts_ns);
    assert_eq!(a.event, b.event);
    assert_eq!(a.transport, b.transport);
    assert_eq!(a.user, b.user);
    assert_eq!(a.ip_subnet, b.ip_subnet);
    assert_eq!(a.session_id_prefix, b.session_id_prefix);
    assert_eq!(a.result, b.result);
    assert_eq!(a.details_canonical_msgpack, b.details_canonical_msgpack);
    assert_eq!(a.prev_hmac, b.prev_hmac);
    assert_eq!(a.hmac, b.hmac);
}

#[test]
fn strict_mode_writes_each_entry_with_fsync() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_strict(dir.path()).unwrap();
    let chain = AuditChain::new(key());

    let mut written = Vec::new();
    for i in 0..5 {
        let e = append_event(&chain, i);
        // strict mode: writes synchronously inside append_entry
        use shamir_connect::server::audit_chain::AuditAppender;
        appender.append_entry(&e);
        written.push(e);
    }

    // Drop the appender → file handles closed.
    drop(appender);

    // Re-read and confirm.
    let read = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(read.len(), 5);
    for (a, b) in written.iter().zip(read.iter()) {
        entries_equal(a, b);
    }
}

#[test]
fn checkpoint_persists_and_loads() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_strict(dir.path()).unwrap();

    {
        use shamir_connect::server::audit_chain::AuditAppender;
        appender.checkpoint(42, &[0xab; 32]);
    }

    drop(appender);

    let loaded = FjallAuditAppender::load_checkpoint(dir.path()).unwrap();
    assert_eq!(loaded, Some((42u64, [0xab; 32])));
}

#[test]
fn chain_verifies_after_restart() {
    let dir = TempDir::new().unwrap();
    let chain = AuditChain::new(key());
    let appender = FjallAuditAppender::open_strict(dir.path()).unwrap();

    use shamir_connect::server::audit_chain::AuditAppender;
    for i in 0..10 {
        let e = append_event(&chain, i);
        appender.append_entry(&e);
    }
    let (next_seq, prev_hmac) = chain.checkpoint();
    appender.checkpoint(next_seq, &prev_hmac);

    drop(appender);
    drop(chain);

    // Restart: re-read the log + checkpoint and verify.
    let read = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(read.len(), 10);
    AuditChain::verify_chain(&key(), &read).expect("chain valid");

    let (cp_seq, cp_hmac) = FjallAuditAppender::load_checkpoint(dir.path())
        .unwrap()
        .expect("checkpoint exists");
    AuditChain::verify_against_checkpoint(&read, cp_seq, &cp_hmac).expect("checkpoint matches");
}

#[test]
fn truncation_detected_after_restart() {
    let dir = TempDir::new().unwrap();
    let chain = AuditChain::new(key());
    let appender = FjallAuditAppender::open_strict(dir.path()).unwrap();

    use shamir_connect::server::audit_chain::AuditAppender;
    for i in 0..10 {
        let e = append_event(&chain, i);
        appender.append_entry(&e);
    }
    let (next_seq, prev_hmac) = chain.checkpoint();
    appender.checkpoint(next_seq, &prev_hmac);

    drop(appender);
    drop(chain);

    // Manually truncate last 3 lines from the audit log.
    let log_path = dir.path().join("audit.log");
    let contents = std::fs::read_to_string(&log_path).unwrap();
    let mut lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 10);
    lines.truncate(7);
    let truncated: String = lines.join("\n") + "\n";
    std::fs::write(&log_path, truncated).unwrap();

    // Re-read and verify against the older checkpoint.
    let read = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(read.len(), 7);
    let (cp_seq, cp_hmac) = FjallAuditAppender::load_checkpoint(dir.path())
        .unwrap()
        .expect("checkpoint exists");
    let res = AuditChain::verify_against_checkpoint(&read, cp_seq, &cp_hmac);
    assert!(
        matches!(res, Err(AuditError::TruncationDetected { .. })),
        "expected TruncationDetected, got {:?}",
        res,
    );
}

// multi_thread flavor: the batched appender's background flusher
// calls `tokio::task::block_in_place` around the synchronous fsync,
// which panics on the default current-thread runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batched_mode_flushes_on_interval() {
    let dir = TempDir::new().unwrap();
    let appender =
        FjallAuditAppender::open_batched(dir.path(), Duration::from_millis(100)).unwrap();
    let chain = AuditChain::new(key());

    use shamir_connect::server::audit_chain::AuditAppender;
    for i in 0..3 {
        let e = append_event(&chain, i);
        appender.append_entry(&e);
    }

    // Allow the background task to fire its 100ms timer.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let read = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(read.len(), 3, "background flusher must drain buffer");

    appender.shutdown().await;
}

// multi_thread flavor: see `batched_mode_flushes_on_interval` above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_flushes_pending() {
    let dir = TempDir::new().unwrap();
    // 1-hour interval — only `shutdown()` should drain the buffer.
    let appender = FjallAuditAppender::open_batched(dir.path(), Duration::from_secs(3600)).unwrap();
    let chain = AuditChain::new(key());

    use shamir_connect::server::audit_chain::AuditAppender;
    for i in 0..5 {
        let e = append_event(&chain, i);
        appender.append_entry(&e);
    }

    appender.shutdown().await;

    let read = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(read.len(), 5, "shutdown must flush all pending entries");
}
