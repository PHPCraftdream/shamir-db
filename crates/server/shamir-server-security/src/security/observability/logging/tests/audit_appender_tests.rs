use super::super::super::FjallAuditAppender;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry, AuditError,
};
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

/// Bounded rendezvous deadline for the post-close fence test (never a sleep).
const HANG_GUARD: Duration = Duration::from_secs(120);
/// Audit chain key for the post-close fence test.
const FENCE_KEY: [u8; 32] = [0x5a; 32];
/// The writer's inline checkpoint boundary (`seq % 1000 == 0`).
const INLINE_CHECKPOINT_EVERY: u64 = 1000;

/// Releases the parked writer exactly once: explicitly, or from `Drop` when an
/// assertion panics first, so a failure cannot strand the writer thread.
struct ReleaseGuard(std::sync::mpsc::SyncSender<()>);

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

/// Raw anchor row (8-byte next sequence + 32-byte digest), or `None` before
/// the first durable anchor write.
fn anchor_row(appender: &FjallAuditAppender) -> Option<Vec<u8>> {
    appender
        .checkpoint_ks
        .get(&[0u8][..])
        .expect("anchor row read")
        .map(|value| value.as_ref().to_vec())
}

/// ITEM 4: a checkpoint resumed after the appender was closed must be refused
/// without a second anchor write.
///
/// The writer's first entry is `seq = 1000`, so its own inline checkpoint runs
/// right after the append. It parks inside the one-shot end-of-append hook, the
/// test closes the appender (`request_flusher_stop` + `final_flush_blocking`,
/// which drains the buffer and persists the anchor), records that anchor, then
/// releases the writer. Mutant (fence removed): the resumed checkpoint writes
/// the anchor again and the writer sees `Ok` instead of
/// `AuditError::Persistence`.
#[test]
fn post_close_inline_checkpoint_is_refused_and_anchor_is_not_rewritten_c4e08b17() {
    let dir = TempDir::new().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let appender = {
        // `open_batched` spawns the background flusher. This runtime is never
        // driven afterwards, so that task never polls and cannot flush.
        let _entered = runtime.enter();
        FjallAuditAppender::open_batched(dir.path(), Duration::from_secs(3600)).unwrap()
    };

    let (parked_tx, parked_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
    appender.arm_after_append_hook(Arc::new({
        let release_rx = Arc::clone(&release_rx);
        move || {
            let _ = parked_tx.send(());
            // Bounded: the guard's `Drop` (or the explicit release below)
            // always unparks this thread.
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(HANG_GUARD)
                .expect("release never arrived");
        }
    }));

    let writer = Arc::new(AuditChainWriter::new(
        AuditChain::from_checkpoint(FENCE_KEY, INLINE_CHECKPOINT_EVERY, [0x11; 32]),
        appender.clone(),
    ));
    let appending = Arc::clone(&writer);
    let append_thread = std::thread::spawn(move || {
        appending.append(
            "auth_success",
            "test",
            "user",
            "127.0.0.0/24",
            [7; 8],
            "ok",
            vec![0xc0],
            42,
        )
    });

    parked_rx
        .recv_timeout(HANG_GUARD)
        .expect("the writer never reached the end of its batched append");
    appender.request_flusher_stop();
    let close = appender.final_flush_blocking();
    assert!(close.is_ok(), "the close itself must succeed: {close:?}");
    let closed_anchor = anchor_row(&appender);
    assert!(
        closed_anchor.is_some(),
        "the close must persist the flushed anchor"
    );

    // Release the parked writer: only the fence can keep the writer's resumed
    // inline checkpoint from writing the anchor again.
    drop(ReleaseGuard(release_tx));

    let error = append_thread
        .join()
        .expect("writer thread panicked")
        .expect_err("the resumed post-close checkpoint must be refused");
    assert!(
        matches!(&error, AuditError::Persistence(_)),
        "not a persistence refusal: {error:?}"
    );
    assert_eq!(appender.refused_checkpoint_attempts(), 1);
    assert_eq!(
        anchor_row(&appender),
        closed_anchor,
        "the resumed checkpoint must not write the anchor again"
    );
    assert_eq!(appender.final_close_attempts(), 1);
    drop(appender);
}

/// ITEM 5: a strict appender has nothing to flush, so its public `shutdown`
/// must not dispatch through `blocking_io` — `block_in_place` PANICS inside a
/// `LocalSet`. Mutant (unconditional `blocking_io`): the spawned local task
/// panics and the `expect` below fails the test.
#[test]
fn strict_shutdown_completes_inside_a_local_set_a1d5c9e3() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_strict(dir.path()).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    let closing = Arc::clone(&appender);
    // The timeout is built INSIDE the runtime: `tokio::time::timeout`
    // constructs its timer eagerly, and building it before `block_on` is
    // entered panics with "there is no reactor running" on EITHER
    // implementation. Restoring the unconditional `blocking_io(...)` dispatch
    // makes `spawn_local` panic, which is this test's red case.
    let outcome = runtime.block_on(async {
        tokio::time::timeout(
            HANG_GUARD,
            local.run_until(async move {
                tokio::task::spawn_local(async move { closing.shutdown().await })
                    .await
                    .expect("the strict shutdown task panicked")
            }),
        )
        .await
        .expect("strict shutdown did not complete within the bound")
    });
    assert!(
        outcome.is_ok(),
        "strict shutdown reported an error: {outcome:?}"
    );
    assert!(outcome.flusher_join_error.is_none());
    assert_eq!(appender.final_close_attempts(), 1);
    drop(appender);
}
