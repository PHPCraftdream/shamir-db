use super::super::super::FjallAuditAppender;
use shamir_connect::server::audit_chain::{AuditAppender, AuditEntry};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use tempfile::TempDir;

fn entry(seq: u64) -> AuditEntry {
    AuditEntry {
        seq,
        ts_ns: seq,
        event: format!("event-{seq}"),
        transport: "test".into(),
        user: "user".into(),
        ip_subnet: "127.0.0.1".into(),
        session_id_prefix: [seq as u8; 8],
        result: "ok".into(),
        details_canonical_msgpack: Vec::new(),
        prev_hmac: [0; 32],
        hmac: [seq as u8; 32],
    }
}

#[tokio::test]
async fn flush_checkpoint_race_serializes_checkpoint_after_drain_7ffa5398() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_batched(dir.path(), Duration::from_secs(60)).unwrap();
    appender.append_entry(&entry(1)).unwrap();
    let extracted = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    *appender.after_extract_hook.lock() = Some({
        let extracted = extracted.clone();
        let resume = resume.clone();
        Arc::new(move || {
            extracted.wait();
            resume.wait();
        })
    });

    let flushing = appender.clone();
    let flush_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(flushing.flush_now())
            .unwrap();
    });
    extracted.wait();

    let contention = Arc::new(Barrier::new(2));
    *appender.gate_contention_hook.lock() = Some({
        let contention = contention.clone();
        Arc::new(move || {
            contention.wait();
        })
    });
    let checkpointing = appender.clone();
    let checkpoint_thread = std::thread::spawn(move || checkpointing.checkpoint(2, &[1; 32]));
    contention.wait();
    assert!(appender.checkpoint_ks.get(&[0u8][..]).unwrap().is_none());

    resume.wait();
    flush_thread.join().unwrap();
    assert!(checkpoint_thread.join().unwrap().is_ok());
    assert_eq!(
        appender
            .checkpoint_ks
            .get(&[0u8][..])
            .unwrap()
            .unwrap()
            .as_ref(),
        &[2u64.to_be_bytes().as_slice(), &[1; 32]].concat()
    );
    let entries = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[0].hmac, [1; 32]);
    appender.shutdown().await;
    drop(appender);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some((2, [1; 32]))
    );
}

// Deadlock schedule of the removed async-mutex bridge: the checkpoint runs on
// the only runtime thread while another thread owns the gate mid-drain.
#[tokio::test(flavor = "current_thread")]
async fn checkpoint_on_runtime_thread_waits_for_gate_without_deadlock_7ffa5398() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_batched(dir.path(), Duration::from_secs(60)).unwrap();
    appender.append_entry(&entry(1)).unwrap();
    let extracted = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    *appender.after_extract_hook.lock() = Some({
        let extracted = extracted.clone();
        let resume = resume.clone();
        Arc::new(move || {
            extracted.wait();
            resume.wait();
        })
    });
    let flushing = appender.clone();
    let flush_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(flushing.flush_now())
            .unwrap();
    });
    extracted.wait();

    let contention = Arc::new(Barrier::new(2));
    *appender.gate_contention_hook.lock() = Some({
        let contention = contention.clone();
        Arc::new(move || {
            contention.wait();
        })
    });
    let releaser = {
        let contention = contention.clone();
        let resume = resume.clone();
        std::thread::spawn(move || {
            contention.wait();
            resume.wait();
        })
    };
    appender.checkpoint(2, &[1; 32]).unwrap();
    releaser.join().unwrap();
    flush_thread.join().unwrap();
    assert_eq!(
        FjallAuditAppender::read_log_for_verify(dir.path())
            .unwrap()
            .len(),
        1
    );
    appender.shutdown().await;
    drop(appender);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some((2, [1; 32]))
    );
}

#[tokio::test]
async fn checkpoint_gate_excludes_concurrent_drain_7ffa5398() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_batched(dir.path(), Duration::from_secs(60)).unwrap();
    appender.append_entry(&entry(1)).unwrap();
    let acquired = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    *appender.gate_acquired_hook.lock() = Some({
        let acquired = acquired.clone();
        let release = release.clone();
        Arc::new(move || {
            acquired.wait();
            release.wait();
        })
    });
    let checkpointing = appender.clone();
    let checkpoint_thread = std::thread::spawn(move || checkpointing.checkpoint(2, &[1; 32]));
    acquired.wait();

    let contention = Arc::new(Barrier::new(2));
    *appender.gate_contention_hook.lock() = Some({
        let contention = contention.clone();
        Arc::new(move || {
            contention.wait();
        })
    });
    let flushing = appender.clone();
    let flush_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(flushing.flush_now())
            .unwrap();
    });
    contention.wait();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("audit.log")).unwrap(),
        ""
    );
    release.wait();
    checkpoint_thread.join().unwrap().unwrap();
    flush_thread.join().unwrap();
    assert_eq!(
        FjallAuditAppender::read_log_for_verify(dir.path())
            .unwrap()
            .len(),
        1
    );
    appender.shutdown().await;
}

#[tokio::test]
async fn failed_drain_latches_and_prevents_checkpoint_metadata_7ffa5398() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_batched(dir.path(), Duration::from_secs(60)).unwrap();
    appender.checkpoint(1, &[0; 32]).unwrap();
    appender.append_entry(&entry(1)).unwrap();
    appender
        .fail_next_flush
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(appender.checkpoint(2, &[1; 32]).is_err());
    assert!(appender.checkpoint(3, &[2; 32]).is_err());
    assert_eq!(
        appender
            .checkpoint_ks
            .get(&[0u8][..])
            .unwrap()
            .unwrap()
            .as_ref(),
        &[1u64.to_be_bytes().as_slice(), &[0; 32]].concat()
    );
    appender.shutdown().await;
    drop(appender);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some((1, [0; 32]))
    );
}

#[tokio::test]
async fn flush_now_rotates_and_preserves_entries_7ffa5398() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_batched_with_rotation(
        dir.path(),
        Duration::from_secs(60),
        Some(1),
        0,
    )
    .unwrap();
    appender.append_entry(&entry(1)).unwrap();
    appender.flush_now().await.unwrap();
    assert!(std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|item| item.file_name().to_string_lossy().starts_with("audit.log.")));
    let entries = FjallAuditAppender::read_log_for_verify(dir.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[0].hmac, [1; 32]);
    appender.shutdown().await;
}
