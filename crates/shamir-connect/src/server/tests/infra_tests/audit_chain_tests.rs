use crate::server::audit_chain::{
    canonical_bytes, AuditAppender, AuditChain, AuditChainWriter, AuditEntry, AuditError,
};
use std::sync::Arc;

fn key() -> [u8; 32] {
    [0xa1u8; 32]
}

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

#[test]
fn first_entry_has_seq_1_and_zero_prev_hmac() {
    let c = AuditChain::new(key());
    let e = append_event(&c, 0);
    assert_eq!(e.seq, 1);
    assert_eq!(e.prev_hmac, [0u8; 32]);
    assert_ne!(e.hmac, [0u8; 32]);
}

#[test]
fn second_entry_chains_to_first() {
    let c = AuditChain::new(key());
    let e1 = append_event(&c, 0);
    let e2 = append_event(&c, 1);
    assert_eq!(e2.seq, 2);
    assert_eq!(e2.prev_hmac, e1.hmac);
}

#[test]
fn verify_chain_ok_for_clean_log() {
    let c = AuditChain::new(key());
    for i in 0..10 {
        append_event(&c, i);
    }
    let entries = c.snapshot();
    assert!(AuditChain::verify_chain(&key(), &entries).is_ok());
}

#[test]
fn verify_chain_detects_tampered_event_field() {
    let c = AuditChain::new(key());
    for i in 0..5 {
        append_event(&c, i);
    }
    let mut entries = c.snapshot();
    // Tamper: change event of entry 2.
    entries[2].event = "auth_failed".into();
    match AuditChain::verify_chain(&key(), &entries) {
        Err(AuditError::HmacMismatch { at }) => assert_eq!(at, 2),
        other => panic!("expected HmacMismatch at 2, got {:?}", other),
    }
}

#[test]
fn verify_chain_detects_broken_link() {
    let c = AuditChain::new(key());
    for i in 0..5 {
        append_event(&c, i);
    }
    let mut entries = c.snapshot();
    // Drop entry 1 — entry 2's prev_hmac no longer matches entry 0's hmac.
    entries.remove(1);
    // Re-number seq so SequenceGap doesn't fire first.
    for (i, e) in entries.iter_mut().enumerate() {
        e.seq = (i + 1) as u64;
    }
    let result = AuditChain::verify_chain(&key(), &entries);
    assert!(matches!(
        result,
        Err(AuditError::ChainBroken { .. }) | Err(AuditError::HmacMismatch { .. })
    ));
}

#[test]
fn verify_chain_detects_sequence_gap() {
    let c = AuditChain::new(key());
    for i in 0..5 {
        append_event(&c, i);
    }
    let mut entries = c.snapshot();
    entries[2].seq = 999;
    match AuditChain::verify_chain(&key(), &entries) {
        Err(AuditError::SequenceGap {
            at,
            expected,
            found,
        }) => {
            assert_eq!(at, 2);
            assert_eq!(expected, 3);
            assert_eq!(found, 999);
        }
        other => panic!("expected SequenceGap, got {:?}", other),
    }
}

#[test]
fn verify_chain_detects_wrong_key() {
    let c = AuditChain::new(key());
    append_event(&c, 0);
    let entries = c.snapshot();
    let wrong_key = [0xffu8; 32];
    assert!(matches!(
        AuditChain::verify_chain(&wrong_key, &entries),
        Err(AuditError::HmacMismatch { .. })
    ));
}

#[test]
fn truncation_defence_detects_missing_trailing_entries() {
    let c = AuditChain::new(key());
    for i in 0..10 {
        append_event(&c, i);
    }
    let (next_seq, prev_hmac) = c.checkpoint();
    let entries = c.snapshot();

    // Truncate last 3 entries.
    let truncated = &entries[..entries.len() - 3];
    let result = AuditChain::verify_against_checkpoint(truncated, next_seq, &prev_hmac);
    assert!(matches!(result, Err(AuditError::TruncationDetected { .. })));
}

#[test]
fn truncation_defence_passes_when_log_complete() {
    let c = AuditChain::new(key());
    for i in 0..10 {
        append_event(&c, i);
    }
    let (next_seq, prev_hmac) = c.checkpoint();
    let entries = c.snapshot();
    assert!(AuditChain::verify_against_checkpoint(&entries, next_seq, &prev_hmac).is_ok());
}

#[test]
fn from_checkpoint_continues_chain() {
    let c1 = AuditChain::new(key());
    for i in 0..5 {
        append_event(&c1, i);
    }
    let (next_seq, prev_hmac) = c1.checkpoint();

    // Simulate restart: load checkpoint into a fresh chain instance.
    let c2 = AuditChain::from_checkpoint(key(), next_seq, prev_hmac);
    let next = append_event(&c2, 100);
    assert_eq!(next.seq, 6);
    assert_eq!(next.prev_hmac, prev_hmac);
}

/// Regression for the audit-chain split-brain (CRIT-4).
///
/// Before the fix `AuditChainWriter` owned a private `AuditChain`
/// while the scheduler checkpointed a separate `Arc<AuditChain>`.
/// Every append advanced the writer's private chain; the
/// scheduler's checkpoint snapshotted an empty companion. This
/// test pins the invariant that the writer and the scheduler MUST
/// observe the same chain state when constructed via
/// `new_with_shared`.
#[derive(Default)]
struct CountingAppender {
    appends: std::sync::Mutex<Vec<AuditEntry>>,
    checkpoints: std::sync::Mutex<Vec<(u64, [u8; 32])>>,
}
impl AuditAppender for CountingAppender {
    fn append_entry(&self, entry: &AuditEntry) {
        self.appends.lock().unwrap().push(entry.clone());
    }
    fn checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) {
        self.checkpoints
            .lock()
            .unwrap()
            .push((next_seq, *prev_hmac));
    }
}

#[test]
fn audit_writer_and_checkpoint_share_chain_state() {
    // Build a shared chain via the same path server.rs uses.
    let shared = Arc::new(AuditChain::new(key()));
    let appender = Arc::new(CountingAppender::default()) as Arc<dyn AuditAppender>;
    let writer = AuditChainWriter::new_with_shared(Arc::clone(&shared), appender);

    // Append an event through the writer.
    let entry = writer.append(
        "auth_success",
        "tcp",
        "alice",
        "192.0.2.0/24",
        [0u8; 8],
        "ok",
        vec![],
        42,
    );
    assert_eq!(entry.seq, 1);

    // Snapshot/checkpoint via the *same* shared chain — must see
    // the appended event.
    let (next_seq, prev_hmac) = shared.checkpoint();
    assert_eq!(
        next_seq, 2,
        "checkpoint must reflect the appended event (next_seq == last_seq + 1)"
    );
    assert_eq!(
        prev_hmac, entry.hmac,
        "checkpoint hmac must match the appended entry's hmac"
    );
}

/// Pins the restart-continuity invariant: when the writer is
/// constructed from a chain that was loaded from a checkpoint,
/// its first append continues numbering instead of restarting at
/// `seq = 1`.
#[test]
fn audit_writer_continues_seq_after_restart_checkpoint() {
    // Simulate the "prior run": fill some entries and snapshot.
    let prior = AuditChain::new(key());
    for i in 0..7 {
        append_event(&prior, i);
    }
    let (next_seq, prev_hmac) = prior.checkpoint();
    assert_eq!(next_seq, 8);

    // "Restart": rebuild the shared chain from the checkpoint,
    // hand it to a fresh writer.
    let shared = Arc::new(AuditChain::from_checkpoint(key(), next_seq, prev_hmac));
    let appender = Arc::new(CountingAppender::default()) as Arc<dyn AuditAppender>;
    let writer = AuditChainWriter::new_with_shared(Arc::clone(&shared), appender);

    let first_after_restart = writer.append(
        "auth_success",
        "tcp",
        "alice",
        "192.0.2.0/24",
        [0u8; 8],
        "ok",
        vec![],
        999,
    );
    assert_eq!(
        first_after_restart.seq, 8,
        "writer must continue numbering at previous_last_seq + 1 = 8 after restart"
    );
    assert_eq!(first_after_restart.prev_hmac, prev_hmac);
}

#[test]
fn canonical_bytes_byte_exact_for_fixture() {
    // Simple fixture: confirm the layout matches spec §3.3 doc.
    let entry = AuditEntry {
        seq: 1,
        ts_ns: 0x0123456789abcdef,
        event: "auth_success".to_string(),
        transport: "tcp".to_string(),
        user: "alice".to_string(),
        ip_subnet: "192.0.2.0/24".to_string(),
        session_id_prefix: [0u8; 8],
        result: "ok".to_string(),
        details_canonical_msgpack: vec![],
        prev_hmac: [0u8; 32],
        hmac: [0u8; 32], // not part of canonical
    };
    let bytes = canonical_bytes(&entry);

    // Check leading bytes:
    // u64_be(1) = 00 00 00 00 00 00 00 01
    assert_eq!(&bytes[..8], &[0, 0, 0, 0, 0, 0, 0, 1]);
    // u64_be(ts_ns)
    assert_eq!(&bytes[8..16], &0x0123456789abcdefu64.to_be_bytes());
    // u16_be(event_len = 12) followed by "auth_success"
    assert_eq!(&bytes[16..18], &(12u16).to_be_bytes());
    assert_eq!(&bytes[18..30], b"auth_success");
    // ... (subsequent fields).

    // Last 32 bytes = prev_hmac (zeros here).
    assert_eq!(&bytes[bytes.len() - 32..], &[0u8; 32]);
}
