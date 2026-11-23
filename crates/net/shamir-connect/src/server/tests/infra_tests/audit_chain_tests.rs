use crate::common::crypto::hmac_sha256;
use crate::server::audit_chain::{
    canonical_bytes, compute_entry_hmac, AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
    AuditError, MAX_RECORD_BYTES, RETAINED_ENTRIES,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::sync::Arc;

#[test]
fn audit_error_display_reports_context() {
    assert_eq!(
        AuditError::SequenceGap {
            at: 2,
            expected: 3,
            found: 5
        }
        .to_string(),
        "sequence gap at 2: expected 3, found 5"
    );
    assert_eq!(
        AuditError::Persistence("disk full".into()).to_string(),
        "audit persistence: disk full"
    );
}

// Independently derived HMAC-SHA256 outputs: the stdlib Python recipe in
// IMPLEMENTATION_GUIDE.md (never the code under test) produced these values.
const RU4_V1_EXPECTED_HMAC: Option<&str> =
    Some("e19384884a5e7ae49c2937b2a5201172a7c0d0ba2141ab12c627547a5c2f3fc2");
const RU4_V2_EXPECTED_HMAC: Option<&str> =
    Some("bc7ac9783498c88914bfe945503dfa0050cd0a29a1e5c380ef8faa678dd30818");
const RU4_KEY: [u8; 32] = [0x0b; 32];

fn ru4_expected_hmac(value: Option<&str>, name: &str) -> [u8; 32] {
    let hex = value.expect(match name {
        "RU4_V1_EXPECTED_HMAC" => "CI must fill RU4_V1_EXPECTED_HMAC using the independent Python recipe in IMPLEMENTATION_GUIDE.md",
        "RU4_V2_EXPECTED_HMAC" => "CI must fill RU4_V2_EXPECTED_HMAC using the independent Python recipe in IMPLEMENTATION_GUIDE.md",
        _ => "CI must fill the named RU4 expected HMAC constant",
    });
    let bytes = hex::decode(hex).expect("RU4 expected HMAC must be valid hex");
    bytes
        .try_into()
        .unwrap_or_else(|_| panic!("{name} must decode to 32 bytes"))
}

fn ru4_entries(v1_hmac: [u8; 32]) -> [AuditEntry; 2] {
    [
        AuditEntry {
            seq: 1,
            ts_ns: 0,
            event: "e".into(),
            transport: "t".into(),
            user: "".into(),
            ip_subnet: "".into(),
            session_id_prefix: [0; 8],
            result: "ok".into(),
            details_canonical_msgpack: vec![],
            prev_hmac: [0; 32],
            hmac: [0; 32],
        },
        AuditEntry {
            seq: 2,
            ts_ns: 1,
            event: "e".into(),
            transport: "t".into(),
            user: "é".into(),
            ip_subnet: "".into(),
            session_id_prefix: [1, 2, 3, 4, 5, 6, 7, 8],
            result: "ok".into(),
            details_canonical_msgpack: vec![0x80],
            prev_hmac: v1_hmac,
            hmac: [0; 32],
        },
    ]
}

#[test]
fn rv_connect_7dbc7bdb_v1_canonical_bytes_are_pinned_independently() {
    let entry = ru4_entries([0; 32])[0].clone();
    let expected = hex::decode(concat!(
        "0000000000000001",
        "0000000000000000",
        "0001",
        "65",
        "01",
        "74",
        "00",
        "00",
        "0000000000000000",
        "02",
        "6f6b",
        "00000000",
        "0000000000000000000000000000000000000000000000000000000000000000"
    ))
    .unwrap();
    assert_eq!(canonical_bytes(&entry).unwrap(), expected);
}

#[test]
fn rv_connect_7dbc7bdb_ru4_v1_v2_hmac_vectors_and_rejections() {
    let v1_hmac = ru4_expected_hmac(RU4_V1_EXPECTED_HMAC, "RU4_V1_EXPECTED_HMAC");
    let v2_hmac = ru4_expected_hmac(RU4_V2_EXPECTED_HMAC, "RU4_V2_EXPECTED_HMAC");
    let mut entries = ru4_entries(v1_hmac);
    let v2_prefix = hex::decode(concat!(
        "0000000000000002",
        "0000000000000001",
        "0001",
        "65",
        "01",
        "74",
        "02",
        "c3a9",
        "00",
        "0102030405060708",
        "02",
        "6f6b",
        "00000001",
        "80"
    ))
    .unwrap();
    let mut canonical_v2 = v2_prefix;
    canonical_v2.extend_from_slice(&v1_hmac);
    assert_eq!(canonical_bytes(&entries[1]).unwrap(), canonical_v2);
    assert_eq!(compute_entry_hmac(&RU4_KEY, &entries[0]).unwrap(), v1_hmac);
    assert_eq!(compute_entry_hmac(&RU4_KEY, &entries[1]).unwrap(), v2_hmac);
    entries[0].hmac = v1_hmac;
    entries[1].hmac = v2_hmac;
    assert!(AuditChain::verify_chain(&RU4_KEY, &entries).is_ok());
    let mut broken = entries.clone();
    broken[1].prev_hmac[0] ^= 1;
    assert!(matches!(
        AuditChain::verify_chain(&RU4_KEY, &broken),
        Err(AuditError::ChainBroken { at: 1 })
    ));
    assert!(matches!(
        AuditChain::verify_chain(&[0x0c; 32], &entries),
        Err(AuditError::HmacMismatch { .. })
    ));
    let mut payload = entries.clone();
    payload[1].user.push('!');
    assert!(AuditChain::verify_chain(&RU4_KEY, &payload).is_err());
    let mut rejected_prefix = entries[0].prev_hmac.to_vec();
    rejected_prefix.extend_from_slice(&canonical_bytes(&entries[0]).unwrap());
    let prefixed = hmac_sha256(&RU4_KEY, &rejected_prefix);
    assert_ne!(prefixed, v1_hmac);
}

#[test]
fn rv_connect_7dbc7bdb_v2_canonical_layout_uses_eight_byte_session_prefix() {
    let predecessor = [0x11; 32];
    let entry = ru4_entries(predecessor)[1].clone();
    let prefix = hex::decode(concat!(
        "0000000000000002",
        "0000000000000001",
        "0001",
        "65",
        "01",
        "74",
        "02",
        "c3a9",
        "00",
        "0102030405060708",
        "02",
        "6f6b",
        "00000001",
        "80"
    ))
    .unwrap();
    let mut expected = prefix;
    expected.extend_from_slice(&[0x11; 32]);
    assert_eq!(entry.prev_hmac, predecessor);
    assert_eq!(canonical_bytes(&entry).unwrap(), expected);
}

#[test]
fn rv_connect_7dbc7bdb_literal_legacy_chain_resumes_and_extends() {
    let v1_hmac = ru4_expected_hmac(RU4_V1_EXPECTED_HMAC, "RU4_V1_EXPECTED_HMAC");
    let v2_hmac = ru4_expected_hmac(RU4_V2_EXPECTED_HMAC, "RU4_V2_EXPECTED_HMAC");
    let mut legacy = ru4_entries(v1_hmac);
    legacy[0].hmac = v1_hmac;
    legacy[1].hmac = v2_hmac;
    assert!(AuditChain::verify_chain(&RU4_KEY, &legacy).is_ok());

    let resumed = AuditChain::from_checkpoint(RU4_KEY, 3, v2_hmac);
    let third = resumed
        .append("e", "t", "", "", [0; 8], "ok", vec![], 2)
        .unwrap();
    let mut combined = legacy.to_vec();
    combined.push(third);
    assert!(AuditChain::verify_chain(&RU4_KEY, &combined).is_ok());
}
fn key() -> [u8; 32] {
    [0xa1u8; 32]
}

fn append_event(chain: &AuditChain, idx: u64) -> AuditEntry {
    chain
        .append(
            "auth_success",
            "tcp",
            "alice",
            "192.0.2.0/24",
            [0u8; 8],
            "ok",
            vec![],
            1_000_000 + idx,
        )
        .expect("test audit fields fit canonical layout")
}

#[test]
fn rv_connect_7dbc7bdb_rejects_unrepresentable_lengths_without_advancing_chain() {
    let chain = AuditChain::new(key());
    let invalid = chain.append("e".repeat(65_536), "tcp", "", "", [0; 8], "ok", vec![], 1);
    assert!(matches!(
        invalid,
        Err(AuditError::FieldTooLong { field: "event" })
    ));
    assert_eq!(chain.checkpoint(), (1, [0; 32]));

    for (field, transport, user, subnet, result) in [
        (
            "transport",
            "x".repeat(256),
            String::new(),
            String::new(),
            String::new(),
        ),
        (
            "user",
            String::new(),
            "x".repeat(256),
            String::new(),
            String::new(),
        ),
        (
            "ip_subnet",
            String::new(),
            String::new(),
            "x".repeat(256),
            String::new(),
        ),
        (
            "result",
            String::new(),
            String::new(),
            String::new(),
            "x".repeat(256),
        ),
    ] {
        let attempt = chain.append("ok", transport, user, subnet, [0; 8], result, vec![], 1);
        assert!(
            matches!(attempt, Err(AuditError::FieldTooLong { field: found }) if found == field)
        );
        assert_eq!(chain.checkpoint(), (1, [0; 32]));
    }

    assert!(chain
        .append(
            "x".repeat(u16::MAX as usize),
            "x".repeat(255),
            "x".repeat(255),
            "x".repeat(255),
            [0; 8],
            "x".repeat(255),
            vec![0; 255],
            1
        )
        .is_ok());
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
fn rv_connect_48f29207_accepts_authenticated_stale_checkpoint_in_extended_chain() {
    let chain = AuditChain::new(key());
    for seq in 1..=150 {
        append_event(&chain, seq);
    }
    let entries = chain.snapshot();
    let checkpoint_hmac = entries[99].hmac;
    assert!(AuditChain::verify_chain(&key(), &entries).is_ok());
    assert!(AuditChain::verify_against_checkpoint(&entries, 101, &checkpoint_hmac).is_ok());
}

#[test]
fn rv_connect_48f29207_rejects_unmatched_checkpoint_positions_and_digests() {
    let chain = AuditChain::new(key());
    for seq in 1..=4 {
        append_event(&chain, seq);
    }
    let entries = chain.snapshot();
    let zero = [0; 32];
    assert!(matches!(
        AuditChain::verify_against_checkpoint(&entries, 0, &zero),
        Err(AuditError::TruncationDetected { .. })
    ));
    assert!(matches!(
        AuditChain::verify_against_checkpoint(&entries, 6, &entries[3].hmac),
        Err(AuditError::TruncationDetected { .. })
    ));
    assert!(matches!(
        AuditChain::verify_against_checkpoint(&entries, 3, &entries[3].hmac),
        Err(AuditError::TruncationDetected { .. })
    ));
    assert!(matches!(
        AuditChain::verify_against_checkpoint(&entries, 4, &zero),
        Err(AuditError::TruncationDetected { .. })
    ));
}

#[test]
fn rv_connect_48f29207_authenticates_genesis_and_empty_chain_checkpoint() {
    let chain = AuditChain::new(key());
    let first = append_event(&chain, 0);
    let entries = chain.snapshot();
    assert!(AuditChain::verify_against_checkpoint(&entries, 1, &[0; 32]).is_ok());
    assert!(AuditChain::verify_against_checkpoint(&[], 1, &[0; 32]).is_ok());
    assert!(matches!(
        AuditChain::verify_against_checkpoint(&entries, 1, &first.hmac),
        Err(AuditError::TruncationDetected { .. })
    ));
    assert!(matches!(
        AuditChain::verify_against_checkpoint(&[], 2, &[0; 32]),
        Err(AuditError::TruncationDetected { .. })
    ));
}

#[test]
fn rv_connect_7ffa5398_writer_latches_append_and_checkpoint_failures() {
    struct FailingAppend;
    impl AuditAppender for FailingAppend {
        fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
            Err(std::io::Error::other("injected append failure"))
        }
        fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
            Ok(())
        }
    }
    struct FailingCheckpoint;
    impl AuditAppender for FailingCheckpoint {
        fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
            Ok(())
        }
        fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
            Err(std::io::Error::other("injected checkpoint failure"))
        }
    }
    let failed_append_chain = Arc::new(AuditChain::new(key()));
    let writer =
        AuditChainWriter::new_with_shared(failed_append_chain.clone(), Arc::new(FailingAppend));
    assert!(matches!(
        writer.append("e", "t", "u", "i", [0; 8], "r", vec![], 0),
        Err(AuditError::Persistence(_))
    ));
    assert!(matches!(
        writer.checkpoint_now(),
        Err(AuditError::Persistence(_))
    ));
    let success = CountingAppender::default();
    assert!(matches!(
        failed_append_chain.checkpoint_with_appender(&success),
        Err(AuditError::Persistence(_))
    ));
    assert!(success.checkpoints.lock().unwrap().is_empty());

    let failed_checkpoint_chain = Arc::new(AuditChain::new(key()));
    let appender = Arc::new(FailingCheckpoint);
    let writer = AuditChainWriter::new_with_shared(failed_checkpoint_chain.clone(), appender);
    writer
        .append("e", "t", "u", "i", [0; 8], "r", vec![], 0)
        .unwrap();
    assert!(matches!(
        writer.checkpoint_now(),
        Err(AuditError::Persistence(_))
    ));
    let before = failed_checkpoint_chain.checkpoint();
    assert!(matches!(
        writer.append("e", "t", "u", "i", [0; 8], "r", vec![], 1),
        Err(AuditError::Persistence(_))
    ));
    assert_eq!(failed_checkpoint_chain.checkpoint(), before);
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
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.appends.lock().unwrap().push(entry.clone());
        Ok(())
    }
    fn checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) -> Result<(), std::io::Error> {
        self.checkpoints
            .lock()
            .unwrap()
            .push((next_seq, *prev_hmac));
        Ok(())
    }
}

#[test]
fn audit_writer_and_checkpoint_share_chain_state() {
    // Build a shared chain via the same path server.rs uses.
    let shared = Arc::new(AuditChain::new(key()));
    let appender = Arc::new(CountingAppender::default()) as Arc<dyn AuditAppender>;
    let writer = AuditChainWriter::new_with_shared(Arc::clone(&shared), appender);

    // Append an event through the writer.
    let entry = writer
        .append(
            "auth_success",
            "tcp",
            "alice",
            "192.0.2.0/24",
            [0u8; 8],
            "ok",
            vec![],
            42,
        )
        .expect("valid test audit entry");
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

    let first_after_restart = writer
        .append(
            "auth_success",
            "tcp",
            "alice",
            "192.0.2.0/24",
            [0u8; 8],
            "ok",
            vec![],
            999,
        )
        .expect("valid test audit entry");
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
    let bytes = canonical_bytes(&entry).expect("fixture field lengths are valid");

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

/// RV-CONNECT-8A65768F: the in-memory retained Vec must be bounded. The
/// HMAC chain, `checkpoint()` and the durable appender are unaffected by
/// retention, so appending far more than [`RETAINED_ENTRIES`]
/// events must leave `snapshot()` capped at the most recent window — not
/// grow without bound.
#[test]
fn rv_audit_8a65768f_snapshot_window_is_bounded() {
    let c = AuditChain::new(key());
    let total = (RETAINED_ENTRIES as u64) + 100;
    for i in 0..total {
        append_event(&c, i);
    }

    let entries = c.snapshot();
    assert!(
        entries.len() <= RETAINED_ENTRIES,
        "retained window must be bounded at {}, got {}",
        RETAINED_ENTRIES,
        entries.len()
    );
    // The retained window holds the MOST RECENT entries: last seq and
    // chain/checkpoint state must reflect every append.
    assert_eq!(entries.last().map(|e| e.seq), Some(total));
    let (next_seq, _) = c.checkpoint();
    assert_eq!(next_seq, total + 1);
    // The retained suffix is itself a valid HMAC sub-chain tail.
    for pair in entries.windows(2) {
        assert_eq!(pair[1].prev_hmac, pair[0].hmac);
    }
}

// --- Finding 5: shared exact audit record size admission -----------------

/// Byte-exact replica of the durable appender's fixed-format line
/// (eleven tab-separated columns, unpadded base64url, `\`/tab/LF
/// escaping) plus the terminating LF, so the admission bound is checked
/// against REAL emitted bytes — including actual base64 encoding —
/// not against the formula under test.
fn emitted_line_len(entry: &AuditEntry) -> usize {
    let escape = |s: &str| {
        let mut out = String::new();
        for ch in s.chars() {
            match ch {
                '\\' => out.push_str(r"\\"),
                '\t' => out.push_str(r"\t"),
                '\n' => out.push_str(r"\n"),
                c => out.push(c),
            }
        }
        out
    };
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        entry.seq,
        entry.ts_ns,
        escape(&entry.event),
        escape(&entry.transport),
        escape(&entry.user),
        escape(&entry.ip_subnet),
        URL_SAFE_NO_PAD.encode(entry.session_id_prefix),
        escape(&entry.result),
        URL_SAFE_NO_PAD.encode(&entry.details_canonical_msgpack),
        URL_SAFE_NO_PAD.encode(entry.prev_hmac),
        URL_SAFE_NO_PAD.encode(entry.hmac),
    );
    line.len() + 1 // LF
}

fn size_probe(seq: u64, user: &str, details_len: usize) -> AuditEntry {
    AuditEntry {
        seq,
        ts_ns: 0,
        event: "event".into(),
        transport: "t".into(),
        user: user.into(),
        ip_subnet: String::new(),
        session_id_prefix: [0; 8],
        result: "ok".into(),
        details_canonical_msgpack: vec![0u8; details_len],
        prev_hmac: [0; 32],
        hmac: [0; 32],
    }
}

/// Pinned reader ceiling: the security appender's recovery reader
/// refuses records above this exact byte count.
#[test]
fn record_size_ceiling_matches_reader_expression() {
    assert_eq!(MAX_RECORD_BYTES, 22_502_883usize);
}

/// Exact-boundary admission, verified against actually emitted bytes:
/// the boundary entry's real line (with real base64) is exactly
/// `MAX_RECORD_BYTES` and is accepted; one more user byte gives
/// exactly `MAX + 1` and is refused; a further base64 group gives
/// `MAX + 4` and is refused by the preflight lower bound.
#[test]
fn record_size_admission_boundary_matches_emitted_bytes() {
    let boundary = size_probe(1, "x", 16_877_073);
    assert_eq!(emitted_line_len(&boundary), MAX_RECORD_BYTES);
    boundary.validate_log_record_size().expect("boundary fits");
    let chain = AuditChain::new(key());
    let entry = chain
        .append(
            "event",
            "t",
            "x",
            "",
            [0; 8],
            "ok",
            vec![0u8; 16_877_073],
            0,
        )
        .expect("boundary record must be admitted");
    assert_eq!(entry.seq, 1);

    let over_by_one = size_probe(1, "xx", 16_877_073);
    assert_eq!(emitted_line_len(&over_by_one), MAX_RECORD_BYTES + 1);
    assert_eq!(
        over_by_one.validate_log_record_size(),
        Err(AuditError::RecordTooLarge)
    );

    let over_by_four = size_probe(1, "x", 16_877_076);
    assert_eq!(emitted_line_len(&over_by_four), MAX_RECORD_BYTES + 4);
    assert_eq!(
        chain
            .append(
                "event",
                "t",
                "x",
                "",
                [0; 8],
                "ok",
                vec![0u8; 16_877_076],
                0,
            )
            .unwrap_err(),
        AuditError::RecordTooLarge
    );
    // The refusals changed no chain state.
    assert_eq!(chain.checkpoint(), (2u64, entry.hmac));
}

/// Admission must use the assigned sequence's decimal width: the same
/// record fits at seq 1 but not at a 20-digit sequence, and the
/// in-lock exact check (not the placeholder) decides.
#[test]
fn record_size_admission_is_seq_width_exact() {
    let fits = size_probe(1, "", 16_877_073);
    assert_eq!(emitted_line_len(&fits), MAX_RECORD_BYTES - 1);
    fits.validate_log_record_size().expect("fits at seq 1");
    let mut huge = fits.clone();
    huge.seq = u64::MAX;
    assert_eq!(emitted_line_len(&huge), MAX_RECORD_BYTES + 18);
    assert_eq!(
        huge.validate_log_record_size(),
        Err(AuditError::RecordTooLarge)
    );
    let chain = AuditChain::from_checkpoint(key(), u64::MAX, [0; 32]);
    assert!(matches!(
        chain.append("event", "t", "", "", [0; 8], "ok", vec![0u8; 16_877_073], 0,),
        Err(AuditError::RecordTooLarge)
    ));
    assert_eq!(chain.checkpoint(), (u64::MAX, [0; 32]));
}

/// Escape counting and small-entry exactness against real emitted
/// bytes: a 7-byte user with all three escapes emits a 10-byte column,
/// identical to ten plain bytes.
#[test]
fn record_size_counts_escapes_like_emitted_bytes() {
    let escaped = size_probe(1, "a\\b\tc\nd", 1);
    let plain_ten = size_probe(1, "xxxxxxxxxx", 1);
    assert_eq!(emitted_line_len(&escaped), 130);
    assert_eq!(emitted_line_len(&plain_ten), 130);
    assert_eq!(emitted_line_len(&size_probe(1, "x", 1)), 121);
    for entry in [&escaped, &plain_ten] {
        entry.validate_log_record_size().expect("small records fit");
    }
}

/// The writer path refuses oversized input as a typed input refusal
/// BEFORE publication: nothing reaches the appender, no state change,
/// and no fail-closed persistence latch.
#[test]
fn writer_refuses_oversized_record_without_latching() {
    let shared = Arc::new(AuditChain::new(key()));
    let concrete = Arc::new(CountingAppender::default());
    let appender = Arc::clone(&concrete) as Arc<dyn AuditAppender>;
    let writer = AuditChainWriter::new_with_shared(Arc::clone(&shared), appender);
    assert_eq!(
        writer
            .append(
                "event",
                "t",
                "x",
                "",
                [0; 8],
                "ok",
                vec![0u8; 16_877_076],
                0,
            )
            .unwrap_err(),
        AuditError::RecordTooLarge
    );
    // The refusal happened before publication: nothing reached the
    // appender at all.
    assert!(concrete.appends.lock().unwrap().is_empty());
    assert!(concrete.checkpoints.lock().unwrap().is_empty());
    let seq = writer
        .append("event", "t", "x", "", [0; 8], "ok", Vec::new(), 0)
        .map(|e| e.seq)
        .expect("typed input refusal must not latch persistence");
    assert_eq!(seq, 1);
    // Exactly one publication (and its companion checkpoint) after the
    // valid append.
    assert_eq!(concrete.appends.lock().unwrap().len(), 1);
    // Checkpointing is periodic (every 1000th seq), so seq=1 alone does not
    // checkpoint; an explicit checkpoint_now() produces exactly one.
    assert!(concrete.checkpoints.lock().unwrap().is_empty());
    writer.checkpoint_now().expect("manual checkpoint succeeds");
    assert_eq!(concrete.checkpoints.lock().unwrap().len(), 1);
}

/// The sequence boundary at `u64::MAX - 1` and above: the direct chain
/// refuses BEFORE consuming the sequence, predecessor HMAC or retained
/// history; `u64::MAX - 2` is still admitted, the refusal repeats
/// without latching and leaves the checkpoint state untouched. A
/// public `from_checkpoint` can also start at `u64::MAX` directly;
/// `MAX` must not be consumed either. Without the fix the old code
/// consumed the sequence and published the entry (saturating
/// arithmetic left a `MAX` start pinned there), so the durable
/// appender's refusal of a `MAX - 1` record surfaced only afterwards
/// as a latching persistence failure, not a typed stateless refusal.
#[test]
fn chain_refuses_sequence_exhaustion_before_consuming_state() {
    // Positive control: MAX - 2 is admitted, landing at the boundary.
    let chain = AuditChain::from_checkpoint(key(), u64::MAX - 2, [7; 32]);
    let last = chain
        .append("event", "t", "x", "", [0; 8], "ok", Vec::new(), 0)
        .expect("MAX - 2 must be admitted");
    assert_eq!(last.seq, u64::MAX - 2);
    assert_eq!(chain.checkpoint(), (u64::MAX - 1, last.hmac));
    for _ in 0..2 {
        assert!(matches!(
            chain.append("event", "t", "x", "", [0; 8], "ok", Vec::new(), 0),
            Err(AuditError::SequenceExhausted { next }) if next == u64::MAX - 1
        ));
    }
    // The refusals consumed nothing: state is as after MAX - 2.
    assert_eq!(chain.checkpoint(), (u64::MAX - 1, last.hmac));
    let retained = chain.snapshot();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].seq, u64::MAX - 2);
    assert_eq!(retained[0].hmac, last.hmac);
    // A chain constructed directly at `MAX` is refused too.
    let at_max = AuditChain::from_checkpoint(key(), u64::MAX, [7; 32]);
    assert!(matches!(
        at_max.append("event", "t", "x", "", [0; 8], "ok", Vec::new(), 0),
        Err(AuditError::SequenceExhausted { next }) if next == u64::MAX
    ));
    assert_eq!(at_max.checkpoint(), (u64::MAX, [7; 32]));
}

/// The writer path at the same boundary: the refusal is a typed input
/// refusal raised inside the durable-gate + chain synchronization
/// before publication, so nothing reaches the appender, the checkpoint
/// state is untouched, and the refusal repeats. The append is refused
/// without consuming a sequence or appending, AND the writer *latches*
/// that exhaustion: a later `checkpoint_now` refuses with
/// `SequenceExhaustedUnaudited` instead of certifying the log, because
/// the event that was refused was never audited. Without the fix the
/// writer consumed the sequence and published the entry:
/// `CountingAppender` accepted it without latching, but the real Fjall
/// adapter refuses a `MAX - 1` record itself — a defect this seam
/// cannot expose, so the refusal must be in-chain.
#[test]
fn writer_refuses_sequence_exhaustion_and_latches_the_checkpoint() {
    let shared = Arc::new(AuditChain::from_checkpoint(key(), u64::MAX - 1, [9; 32]));
    let concrete = Arc::new(CountingAppender::default());
    let appender = Arc::clone(&concrete) as Arc<dyn AuditAppender>;
    let writer = AuditChainWriter::new_with_shared(Arc::clone(&shared), appender);
    for _ in 0..2 {
        assert!(matches!(
            writer.append("event", "t", "x", "", [0; 8], "ok", Vec::new(), 0),
            Err(AuditError::SequenceExhausted { next }) if next == u64::MAX - 1
        ));
    }
    assert!(concrete.appends.lock().unwrap().is_empty());
    assert!(concrete.checkpoints.lock().unwrap().is_empty());
    // Chain state unchanged by the refusal itself.
    assert_eq!(writer.chain().checkpoint(), (u64::MAX - 1, [9; 32]));
    // The exhaustion latches: checkpoint_now must NOT certify a log that
    // is missing the refused event, so nothing is persisted either.
    assert!(
        matches!(
            writer.checkpoint_now(),
            Err(AuditError::SequenceExhaustedUnaudited)
        ),
        "sequence exhaustion latches so an unaudited event cannot be certified"
    );
    assert!(concrete.checkpoints.lock().unwrap().is_empty());
}
