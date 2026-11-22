use super::super::super::{recovery, rotation, AuditRecoveryError, FjallAuditAppender};
use shamir_connect::server::audit_chain::{
    compute_entry_hmac, AuditChain, AuditChainWriter, AuditEntry, AuditError,
};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

type Position = (u64, [u8; 32]);

const KEY: [u8; 32] = [0x39; 32];
const WRONG_KEY: [u8; 32] = [0xa7; 32];
const GENESIS: Position = (1, [0; 32]);
const FLUSH_INTERVAL: Duration = Duration::from_secs(3600);
const HANG_GUARD: Duration = Duration::from_secs(120);
const ACTIVE: &str = "audit.log";
const ROTATED: &str = "audit.log.00000000000000000001";
const DURABLE_EVENT: &str = "auth_success";

fn append(writer: &AuditChainWriter, event: &str) -> AuditEntry {
    writer
        .append(
            event,
            "test",
            "user",
            "127.0.0.0/24",
            [7; 8],
            "ok",
            vec![0xc0],
            42,
        )
        .unwrap()
}

fn after(entry: &AuditEntry) -> Position {
    (entry.seq + 1, entry.hmac)
}

fn strict_history(dir: &Path, count: usize, checkpoint_at: Option<usize>) -> Vec<AuditEntry> {
    let appender = FjallAuditAppender::open_strict(dir).unwrap();
    let writer = AuditChainWriter::new(AuditChain::new(KEY), appender);
    let mut entries = Vec::new();
    for index in 1..=count {
        let entry = append(&writer, DURABLE_EVENT);
        if checkpoint_at == Some(index) {
            writer.checkpoint_now().unwrap();
        }
        entries.push(entry);
    }
    entries
}

fn read_entries(dir: &Path) -> Vec<AuditEntry> {
    FjallAuditAppender::read_log_for_verify(dir).unwrap()
}

fn assert_full_history(dir: &Path, expected: &[AuditEntry]) {
    let actual = read_entries(dir);
    AuditChain::verify_chain(&KEY, &actual).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.seq, expected.seq);
        assert_eq!(actual.event, expected.event);
        assert_eq!(actual.prev_hmac, expected.prev_hmac);
        assert_eq!(actual.hmac, expected.hmac);
    }
}

fn recover(
    dir: &Path,
    key: &[u8; 32],
    legacy: Option<Position>,
) -> Result<(Arc<FjallAuditAppender>, Position), AuditRecoveryError> {
    FjallAuditAppender::open_recovered_batched(dir, key, legacy, FLUSH_INTERVAL, None, 0)
}

fn assert_rejected(dir: &Path, key: &[u8; 32], legacy: Option<Position>) -> AuditRecoveryError {
    let active = fs::read(dir.join(ACTIVE)).unwrap();
    let rotated = dir.join(ROTATED);
    let rotated_before = rotated.exists().then(|| fs::read(&rotated).unwrap());
    let error = recover(dir, key, legacy).expect_err("recovery accepted an invalid history");
    assert_eq!(fs::read(dir.join(ACTIVE)).unwrap(), active);
    assert_eq!(
        rotated.exists().then(|| fs::read(&rotated).unwrap()),
        rotated_before
    );
    error
}

fn first_line_end(bytes: &[u8]) -> usize {
    bytes.iter().position(|&byte| byte == b'\n').unwrap() + 1
}

fn flip_authenticated_body(bytes: &mut [u8]) {
    let start = bytes
        .windows(DURABLE_EVENT.len())
        .position(|window| window == DURABLE_EVENT.as_bytes())
        .unwrap();
    bytes[start] ^= 1;
}

#[tokio::test]
async fn empty_history_recovers_genesis() {
    for legacy in [None, Some(GENESIS)] {
        let dir = TempDir::new().unwrap();
        let (appender, position) = recover(dir.path(), &KEY, legacy).unwrap();
        assert_eq!(position, GENESIS);
        assert!(fs::read(dir.path().join(ACTIVE)).unwrap().is_empty());
        drop(appender);
    }
}

#[tokio::test]
async fn verified_tail_wins_over_both_stale_anchors() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, Some(1));
    let stale_appender = after(&entries[0]);
    let stale_legacy = after(&entries[1]);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(stale_appender)
    );
    assert_full_history(dir.path(), &entries);
    for (seq, hmac) in [stale_appender, stale_legacy] {
        AuditChain::verify_against_checkpoint(&entries, seq, &hmac).unwrap();
    }
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let (appender, position) = recover(dir.path(), &KEY, Some(stale_legacy)).unwrap();
    assert_eq!(position, after(&entries[2]));
    assert_eq!(fs::read(path).unwrap(), bytes);
    drop(appender);
}

#[tokio::test]
async fn flipped_authenticated_body_and_wrong_key_are_rejected_without_rewrite() {
    let dir = TempDir::new().unwrap();
    let expected = strict_history(dir.path(), 2, None);
    assert_full_history(dir.path(), &expected);
    let entries = read_entries(dir.path());
    assert_eq!(
        AuditChain::verify_chain(&WRONG_KEY, &entries),
        Err(AuditError::HmacMismatch { at: 0 })
    );
    let error = assert_rejected(dir.path(), &WRONG_KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::HmacMismatch { offset: 0, .. }),
        "{error:?}"
    );

    let path = dir.path().join(ACTIVE);
    let mut bytes = fs::read(&path).unwrap();
    flip_authenticated_body(&mut bytes);
    fs::write(path, bytes).unwrap();
    let entries = read_entries(dir.path());
    assert_eq!(entries.len(), expected.len());
    assert_ne!(entries[0].event, expected[0].event);
    assert_eq!(entries[0].hmac, expected[0].hmac);
    assert_eq!(
        AuditChain::verify_chain(&KEY, &entries),
        Err(AuditError::HmacMismatch { at: 0 })
    );
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::HmacMismatch { offset: 0, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn authenticated_later_row_does_not_excuse_a_sequence_gap() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_strict(dir.path()).unwrap();
    let writer = AuditChainWriter::new(AuditChain::new(KEY), appender.clone());
    let first = append(&writer, DURABLE_EVENT);
    drop(writer);
    // Give row 3 a real MAC and even the correct predecessor for row 1.
    // The only invalid chain invariant is the missing sequence number 2.
    let gap_writer =
        AuditChainWriter::new(AuditChain::from_checkpoint(KEY, 3, first.hmac), appender);
    let later = append(&gap_writer, DURABLE_EVENT);
    drop(gap_writer);
    let entries = read_entries(dir.path());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].prev_hmac, first.hmac);
    assert_eq!(entries[1].hmac, later.hmac);
    assert_eq!(compute_entry_hmac(&KEY, &entries[1]).unwrap(), later.hmac);
    assert_eq!(
        AuditChain::verify_chain(&KEY, &entries),
        Err(AuditError::SequenceGap {
            at: 1,
            expected: 2,
            found: 3,
        })
    );
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(
            error,
            AuditRecoveryError::SequenceGap {
                expected: 2,
                found: 3,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn history_truncated_behind_either_anchor_is_rejected_including_empty() {
    for appender_anchor in [false, true] {
        let dir = TempDir::new().unwrap();
        let entries = strict_history(dir.path(), 2, appender_anchor.then_some(2));
        let anchor = after(&entries[1]);
        let legacy = (!appender_anchor).then_some(anchor);
        assert_eq!(
            FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
            appender_anchor.then_some(anchor)
        );
        let path = dir.path().join(ACTIVE);
        let bytes = fs::read(&path).unwrap();
        for keep in [first_line_end(&bytes), 0] {
            fs::write(&path, &bytes[..keep]).unwrap();
            let prefix = read_entries(dir.path());
            AuditChain::verify_chain(&KEY, &prefix).unwrap();
            assert!(matches!(
                AuditChain::verify_against_checkpoint(&prefix, anchor.0, &anchor.1),
                Err(AuditError::TruncationDetected { .. })
            ));
            let expected_next_seq = if keep == 0 { 1 } else { 2 };
            let error = assert_rejected(dir.path(), &KEY, legacy);
            assert!(
                matches!(
                    error,
                    AuditRecoveryError::AnchorPastTail {
                        next_seq: 3,
                        verified_next_seq,
                    } if verified_next_seq == expected_next_seq
                ),
                "{error:?}"
            );
        }
    }
}

#[tokio::test]
async fn conflicting_anchors_at_the_same_position_are_rejected() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, Some(2));
    let good = after(&entries[1]);
    let mut wrong = good;
    wrong.1[0] ^= 1;
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(good)
    );
    AuditChain::verify_against_checkpoint(&entries, good.0, &good.1).unwrap();
    assert!(AuditChain::verify_against_checkpoint(&entries, wrong.0, &wrong.1).is_err());
    assert_full_history(dir.path(), &entries);
    let error = assert_rejected(dir.path(), &KEY, Some(wrong));
    assert!(
        matches!(
            error,
            AuditRecoveryError::ConflictingAnchors { next_seq: 3 }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn rotated_history_recovers_from_genesis_through_active_tail() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, None);
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    // Lay out real strict-log bytes as a rotated segment and the active log,
    // without depending on wall-clock rotation timestamps or a size threshold.
    fs::write(dir.path().join(ROTATED), &bytes[..end]).unwrap();
    fs::write(&path, &bytes[end..]).unwrap();
    assert_full_history(dir.path(), &entries);
    let (appender, position) = recover(dir.path(), &KEY, None).unwrap();
    assert_eq!(position, after(&entries[2]));
    drop(appender);
}

#[tokio::test]
async fn retained_suffix_requires_a_usable_anchor() {
    for appender_anchor in [false, true] {
        let dir = TempDir::new().unwrap();
        let entries = strict_history(dir.path(), 3, appender_anchor.then_some(1));
        let anchor = after(&entries[0]);
        let path = dir.path().join(ACTIVE);
        let bytes = fs::read(&path).unwrap();
        let suffix = &bytes[first_line_end(&bytes)..];
        let end = first_line_end(suffix);
        fs::write(dir.path().join(ROTATED), &suffix[..end]).unwrap();
        fs::write(path, &suffix[end..]).unwrap();
        let kept = read_entries(dir.path());
        assert_eq!(kept.len(), 2);
        let mut position = anchor;
        for row in &kept {
            assert_eq!((row.seq, row.prev_hmac), position);
            assert_eq!(row.hmac, compute_entry_hmac(&KEY, row).unwrap());
            position = after(row);
        }
        assert_eq!(position, after(&entries[2]));
        if !appender_anchor {
            for legacy in [None, Some(GENESIS)] {
                let error = assert_rejected(dir.path(), &KEY, legacy);
                assert!(
                    matches!(error, AuditRecoveryError::UnanchoredSuffix { first_seq: 2 }),
                    "{error:?}"
                );
            }
        }
        let legacy = (!appender_anchor).then_some(anchor);
        let (appender, position) = recover(dir.path(), &KEY, legacy).unwrap();
        assert_eq!(position, after(&entries[2]));
        drop(appender);
    }
}

#[tokio::test]
async fn torn_active_tail_is_discarded_after_verified_prefix_and_anchors() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, Some(1));
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    // Start writing the second entry but stop before the first tab.
    fs::write(&path, &bytes[..end + 1]).unwrap();
    let (appender, position) = recover(dir.path(), &KEY, Some(GENESIS)).unwrap();
    assert_eq!(position, after(&entries[0]));
    assert_eq!(fs::read(path).unwrap(), &bytes[..end]);
    assert_full_history(dir.path(), &entries[..1]);
    drop(appender);
}

#[tokio::test]
async fn torn_active_tail_is_preserved_if_prefix_or_anchors_fail_verification() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, Some(1));
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    let mut torn = bytes[..end + 1].to_vec();
    fs::write(&path, &torn).unwrap();
    let error = assert_rejected(dir.path(), &KEY, Some(after(&entries[1])));
    assert!(
        matches!(
            error,
            AuditRecoveryError::AnchorPastTail {
                next_seq: 3,
                verified_next_seq: 2,
            }
        ),
        "{error:?}"
    );

    flip_authenticated_body(&mut torn);
    fs::write(path, torn).unwrap();
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::HmacMismatch { offset: 0, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn parseable_active_record_without_lf_is_rejected_and_preserved() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, Some(1));
    let path = dir.path().join(ACTIVE);
    let mut bytes = fs::read(&path).unwrap();
    let second_offset = u64::try_from(first_line_end(&bytes)).unwrap();
    assert_eq!(bytes.pop(), Some(b'\n'));
    fs::write(&path, bytes).unwrap();
    // The independent reader accepts the parseable line at EOF and verifies
    // its HMAC; recovery must still reject it rather than discard or append.
    assert_full_history(dir.path(), &entries);
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(
            error,
            AuditRecoveryError::UnterminatedActiveRecord { offset, .. } if offset == second_offset
        ),
        "{error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn crash_drops_published_but_unflushed_entries_and_resumes_without_gap() {
    let dir = TempDir::new().unwrap();
    let appender = FjallAuditAppender::open_batched(dir.path(), FLUSH_INTERVAL).unwrap();
    let writer = AuditChainWriter::new(AuditChain::new(KEY), appender.clone());
    let first = append(&writer, DURABLE_EVENT);
    let second = append(&writer, DURABLE_EVENT);
    writer.checkpoint_now().unwrap();
    let durable = [first, second];
    assert_full_history(dir.path(), &durable);
    let bytes = fs::read(dir.path().join(ACTIVE)).unwrap();
    let lost_first = append(&writer, "lost-first");
    let lost_last = append(&writer, "lost-last");
    assert_eq!(lost_first.seq, after(&durable[1]).0);
    assert_eq!(writer.chain().checkpoint(), after(&lost_last));
    assert_eq!(fs::read(dir.path().join(ACTIVE)).unwrap(), bytes);

    // No await between buffering and dropping on this current-thread runtime:
    // the flusher cannot run, and we never call shutdown or a final flush.
    drop(writer);
    drop(appender);
    assert_eq!(fs::read(dir.path().join(ACTIVE)).unwrap(), bytes);

    let (appender, (next_seq, prev_hmac)) = recover(dir.path(), &KEY, None).unwrap();
    assert_eq!((next_seq, prev_hmac), after(&durable[1]));
    let writer = AuditChainWriter::new(
        AuditChain::from_checkpoint(KEY, next_seq, prev_hmac),
        appender.clone(),
    );
    let resumed = append(&writer, "after-restart");
    assert_eq!(resumed.seq, lost_first.seq);
    assert_eq!(resumed.prev_hmac, durable[1].hmac);
    assert_ne!(resumed.hmac, lost_first.hmac);
    drop(writer);
    tokio::time::timeout(HANG_GUARD, appender.shutdown())
        .await
        .expect("recovered appender did not shut down");
    drop(appender);
    let [first, second] = durable;
    assert_full_history(dir.path(), &[first, second, resumed]);
}

#[tokio::test]
async fn nonmonotonic_archive_names_recover_in_content_order() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, None);
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let split = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(1)
        .map(|(offset, _)| offset + 1)
        .unwrap();
    // Names step backward while the chain steps forward: filename order and
    // content order disagree, and content order must win.
    fs::write(
        dir.path().join("audit.log.00000000000000000099"),
        &bytes[..split],
    )
    .unwrap();
    fs::write(
        dir.path().join("audit.log.00000000000000000005"),
        &bytes[split..],
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    let (appender, position) = recover(dir.path(), &KEY, None).unwrap();
    assert_eq!(position, after(&entries[2]));
    drop(appender);
    assert_full_history(dir.path(), &entries);
}

#[tokio::test]
async fn unrelated_prefixed_file_is_ignored() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, None);
    fs::write(dir.path().join("audit.log.bak"), b"operator backup").unwrap();
    fs::write(dir.path().join("audit.log.not-a-rotation"), b"junk").unwrap();
    let (appender, position) = recover(dir.path(), &KEY, None).unwrap();
    assert_eq!(position, after(&entries[1]));
    drop(appender);
    // The diagnostic reader includes only exact-pattern archives.
    assert_full_history(dir.path(), &entries);
}

#[tokio::test]
async fn overflowing_rotation_name_is_refused() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, None);
    let invalid = dir.path().join("audit.log.99999999999999999999");
    fs::write(&invalid, b"beyond u64").unwrap();
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::InvalidRotationName { .. }),
        "{error:?}"
    );
    // The refusal preserves the exact invalid fixture byte-for-byte.
    assert_eq!(fs::read(&invalid).unwrap(), b"beyond u64");
    fs::remove_file(&invalid).unwrap();
    assert_full_history(dir.path(), &entries);
}

#[tokio::test]
async fn empty_exact_pattern_archive_is_refused() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, None);
    let invalid = dir.path().join("audit.log.00000000000000000002");
    fs::write(&invalid, b"").unwrap();
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::MalformedRecord { offset: 0, .. }),
        "{error:?}"
    );
    // The refusal preserves the exact invalid fixture byte-for-byte.
    assert_eq!(fs::read(&invalid).unwrap(), b"");
    fs::remove_file(&invalid).unwrap();
    assert_full_history(dir.path(), &entries);
}

// Finding 7 — the recovery-side directory-sync seam: closures prove the
// sync runs only AFTER the verified truncation and the active reopen/create,
// and that a sync failure is a typed refusal that anchors nothing.

/// A torn active tail is truncated before the seam, then the REAL parent
/// sync runs. Red: a sync before `set_len`/`open_log` would see the torn byte.
#[tokio::test]
async fn recovery_syncs_directory_after_truncation_and_reopen() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, Some(1));
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    fs::write(&path, &bytes[..end + 1]).unwrap();
    let active = dir.path().join(ACTIVE);
    let expected = bytes[..end].to_vec();
    let expected_in_sync = expected.clone();
    let recovered =
        recovery::recover_with_dir_sync(dir.path(), &KEY, Some(GENESIS), move |log_path| {
            assert_eq!(log_path, active);
            // Truncation and the writable reopen already happened.
            assert_eq!(fs::read(log_path).unwrap(), expected_in_sync);
            rotation::sync_parent(log_path)
        })
        .unwrap();
    assert_eq!(recovered.position, after(&entries[0]));
    assert_eq!(recovered.initial_size, u64::try_from(end).unwrap());
    drop(recovered);
    assert_eq!(fs::read(&path).unwrap(), expected);
}

/// A missing active log is recreated (zero length) before the REAL sync.
/// Red: a sync before `open_log` would see no file.
#[tokio::test]
async fn recovery_syncs_directory_after_creating_the_active_log() {
    let dir = TempDir::new().unwrap();
    let active = dir.path().join(ACTIVE);
    assert!(!active.exists());
    let active_in_sync = active.clone();
    let recovered = recovery::recover_with_dir_sync(dir.path(), &KEY, None, move |log_path| {
        assert_eq!(log_path, active_in_sync);
        assert_eq!(fs::metadata(log_path).unwrap().len(), 0);
        rotation::sync_parent(log_path)
    })
    .unwrap();
    assert_eq!(recovered.position, GENESIS);
    assert_eq!(recovered.initial_size, 0);
    drop(recovered);
    assert_eq!(fs::metadata(&active).unwrap().len(), 0);
}

/// An injected sync failure after a verified truncation is a typed
/// `AuditRecoveryError::Io` refusal and leaves the prior persisted
/// checkpoint exactly as it was.
#[tokio::test]
async fn recovery_refuses_when_the_directory_sync_fails_and_keeps_the_checkpoint() {
    let dir = TempDir::new().unwrap();
    // Anchor at the FIRST row; two more complete rows plus a torn third
    // would verify a NEW tail position if recovery ever anchored before the
    // sync — the unchanged-anchor assertion can detect that premature
    // advance only because the verified position differs from the anchor.
    let entries = strict_history(dir.path(), 3, Some(1));
    let anchor = after(&entries[0]);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(anchor)
    );
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let torn = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(1)
        .map(|(offset, _)| offset + 1)
        .unwrap();
    fs::write(&path, &bytes[..torn + 1]).unwrap();
    let error = recovery::recover_with_dir_sync(dir.path(), &KEY, Some(GENESIS), |_| {
        Err(io::Error::other("injected directory sync failure"))
    })
    .err()
    .expect("recovery accepted a failing directory sync");
    // Typed refusal at the exact active path, source preserved.
    let AuditRecoveryError::Io {
        path: io_path,
        source,
    } = error
    else {
        panic!("unexpected refusal: {error:?}");
    };
    assert_eq!(io_path, dir.path().join(ACTIVE));
    assert_eq!(source.to_string(), "injected directory sync failure");
    // Sync runs after truncation; nothing was anchored.
    assert_eq!(fs::read(&path).unwrap(), &bytes[..torn]);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(anchor)
    );
}

/// The largest rotation name has no legal successor: recovery refuses such
/// a suffix at boot (failure at start, never after new records were
/// synced), preserving the genuine fixture; `u64::MAX - 1` is the healthy
/// control extended below. Red: without the refusal both fixtures boot and
/// auditing latches only at the next rotation.
#[tokio::test]
async fn max_suffix_rotation_name_is_refused_and_max_minus_one_recovers() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, None);
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    let control = dir.path().join("audit.log.18446744073709551614");
    fs::write(&control, &bytes[..end]).unwrap();
    fs::write(&path, &bytes[end..]).unwrap();
    let (appender, position) = recover(dir.path(), &KEY, None).unwrap();
    assert_eq!(position, after(&entries[2]));
    drop(appender);
    assert_full_history(dir.path(), &entries);

    // Same genuine bytes under the exhausted suffix are refused at boot,
    // leaving the active log and the archive exactly as they were.
    let refused = dir.path().join("audit.log.18446744073709551615");
    fs::rename(&control, &refused).unwrap();
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::InvalidRotationName { .. }),
        "{error:?}"
    );
    assert_eq!(fs::read(&refused).unwrap(), &bytes[..end]);
    assert_eq!(fs::read(&path).unwrap(), &bytes[end..]);

    // With `MAX-1` on disk the rotation skips without minting `MAX` or
    // latching; a fresh boot still resumes at the verified tail.
    fs::rename(&refused, &control).unwrap();
    let (appender, position) = FjallAuditAppender::open_recovered_batched(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1), // every flush rotates: the exhausted-suffix skip fires
        1,
    )
    .unwrap();
    assert_eq!(position, after(&entries[2]));
    let writer = AuditChainWriter::new(
        AuditChain::from_checkpoint(KEY, position.0, position.1),
        appender.clone(),
    );
    let fourth = append(&writer, DURABLE_EVENT);
    drop(writer);
    tokio::time::timeout(HANG_GUARD, appender.flush_now())
        .await
        .unwrap()
        .unwrap();
    drop(appender);
    let suffixes = archive_suffixes(dir.path());
    assert_eq!(suffixes, [u64::MAX - 1]);
    // The skipped rotation kept the prior active bytes and the flushed
    // fourth record grew that same file.
    let active_now = fs::read(&path).unwrap();
    assert!(
        active_now.starts_with(&bytes[end..]),
        "skipped rotation must keep the prior active bytes"
    );
    assert!(
        active_now.len() > bytes.len() - end,
        "the flushed record must have grown the active file"
    );
    let (rebooted, reboot_position) =
        FjallAuditAppender::open_recovered_batched(dir.path(), &KEY, None, FLUSH_INTERVAL, None, 0)
            .unwrap();
    assert_eq!(reboot_position, after(&fourth));
    drop(rebooted);
}

/// Legacy wall-clock-rolled-back names step backward while the chain steps
/// forward: recovery exposes the archive suffixes in verified first-record
/// order (`legacy_archive_order`) so retention never prunes by suffix on
/// such a layout; a healthy strictly-increasing layout exposes `None`.
/// Red: with a suffix-ordered sweep the parent would prune the newest
/// legacy archive first, destroying the authenticated tail.
#[tokio::test]
async fn inverted_suffix_names_expose_the_verified_first_sequence_order() {
    // Inverted fixture: the same layout as the content-order recovery test.
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, None);
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let split = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(1)
        .map(|(offset, _)| offset + 1)
        .unwrap();
    fs::write(
        dir.path().join("audit.log.00000000000000000099"),
        &bytes[..split],
    )
    .unwrap();
    fs::write(
        dir.path().join("audit.log.00000000000000000005"),
        &bytes[split..],
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    let recovered =
        recovery::recover_with_dir_sync(dir.path(), &KEY, None, rotation::sync_parent).unwrap();
    assert_eq!(recovered.position, after(&entries[2]));
    assert_eq!(recovered.legacy_archive_order, Some(vec![99, 5]));
    drop(recovered);

    // Healthy control: suffix order stays authoritative, no order vector.
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 3, None);
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    fs::write(dir.path().join(ROTATED), &bytes[..end]).unwrap();
    fs::write(&path, &bytes[end..]).unwrap();
    let recovered =
        recovery::recover_with_dir_sync(dir.path(), &KEY, None, rotation::sync_parent).unwrap();
    assert_eq!(recovered.position, after(&entries[2]));
    assert_eq!(recovered.legacy_archive_order, None);
    drop(recovered);
}

/// An archive whose first row lacks its terminating LF is refused as an
/// `ArchivedFragment` by the exact framing check. Red: dropping the LF
/// check would accept a torn archive.
#[tokio::test]
async fn unterminated_archive_first_row_is_refused() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, None);
    let invalid = dir.path().join("audit.log.00000000000000000002");
    let bytes = fs::read(dir.path().join(ACTIVE)).unwrap();
    let mut row = bytes[..first_line_end(&bytes)].to_vec();
    row.pop();
    fs::write(&invalid, &row).unwrap();
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(
            error,
            AuditRecoveryError::ArchivedFragment { offset: 0, .. }
        ),
        "{error:?}"
    );
    // The refusal preserves the exact invalid fixture byte-for-byte.
    assert_eq!(fs::read(&invalid).unwrap(), row);
    fs::remove_file(&invalid).unwrap();
    assert_full_history(dir.path(), &entries);
}

/// A first row with no tab inside the probe bound falls back to the
/// bounded full-first-row classification and is refused as malformed.
/// Red: without the fallback boot would panic or sort it arbitrarily.
#[tokio::test]
async fn no_tab_archive_first_row_is_refused_as_malformed() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, None);
    let invalid = dir.path().join("audit.log.00000000000000000002");
    fs::write(&invalid, b"not-a-record\n").unwrap();
    let error = assert_rejected(dir.path(), &KEY, None);
    assert!(
        matches!(error, AuditRecoveryError::MalformedRecord { offset: 0, .. }),
        "{error:?}"
    );
    // The refusal preserves the exact invalid fixture byte-for-byte.
    assert_eq!(fs::read(&invalid).unwrap(), b"not-a-record\n");
    fs::remove_file(&invalid).unwrap();
    assert_full_history(dir.path(), &entries);
}

/// Zero-archive-open seam for the retention path. Boot opens the single
/// ordinary archive twice (prefix probe + body scan: all-open delta 2,
/// body delta 1), while a later retention-ENABLED no-prune rotation — the
/// sweep is metadata-only, it does not even prefix-probe — must not open
/// any archive (both counters flat). Red: the old full-first-record sweep
/// would decode every archive's first full row and both counters would
/// rise.
#[tokio::test(flavor = "current_thread")]
async fn retention_enabled_no_prune_rotation_reads_no_archive_body() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 2, None);
    // Current-time name so the sweep finds it young and prunes nothing.
    let now_ns = now_unix_nanos();
    let young = dir.path().join(format!("audit.log.{now_ns:020}"));
    let path = dir.path().join(ACTIVE);
    let bytes = fs::read(&path).unwrap();
    let end = first_line_end(&bytes);
    fs::write(&young, &bytes[..end]).unwrap();
    fs::write(&path, &bytes[end..]).unwrap();

    // Boot opens the single archive twice (probe + body scan).
    let before = recovery::archive_body_opens();
    let before_opens = recovery::archive_opens();
    let (appender, position) = FjallAuditAppender::open_recovered_batched(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1), // every append rotates: the sweep runs on each rotation
        3_600,   // retention enabled, but both archives are young
    )
    .unwrap();
    assert_eq!(recovery::archive_body_opens() - before, 1);
    assert_eq!(recovery::archive_opens() - before_opens, 2);
    assert_eq!(position, after(&entries[1]));

    // No-prune rotation: both open-count deltas must stay zero.
    let writer = AuditChainWriter::new(
        AuditChain::from_checkpoint(KEY, position.0, position.1),
        appender.clone(),
    );
    let resumed = append(&writer, DURABLE_EVENT);
    assert_eq!(resumed.seq, position.0);
    drop(writer);
    tokio::time::timeout(HANG_GUARD, appender.flush_now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovery::archive_body_opens() - before, 1);
    assert_eq!(recovery::archive_opens() - before_opens, 2);
    // No prune: boot archive stays, rotation created one more.
    assert!(young.exists());
    let archives = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| recovery::archive_name(&entry.unwrap().path()).unwrap())
        .count();
    assert_eq!(archives, 2);
    drop(appender);
    assert_eq!(read_entries(dir.path()).len(), 3);
}

/// Round-3 item 1 — physical rotate → retention → reopen: the sweep ages
/// every archive by its mtime (std `FileTimes`), never by the far-future
/// NAME, and a future-dated archive blocks only the contiguous suffix
/// behind it (s1/s2 old, s3 future blocker, s4 old behind it). One append
/// through the recovered rotating writer mints the newest archive and
/// sweeps. Red: aging by name deletes nothing; without the clamp the
/// blocker keeps its future mtime.
#[tokio::test(flavor = "current_thread")]
async fn retention_ages_by_mtime_not_by_far_future_names() {
    let dir = TempDir::new().unwrap();
    let entries = strict_history(dir.path(), 5, None);
    let bytes = fs::read(dir.path().join(ACTIVE)).unwrap();
    let day_ns = 86_400_000_000_000u64;
    let base = now_unix_nanos();
    let names: Vec<u64> = (1..=4)
        .map(|step| base.checked_add(step * day_ns).unwrap())
        .collect();
    let old = SystemTime::now() - Duration::from_secs(30 * 86_400);
    let future = SystemTime::now() + Duration::from_secs(30 * 86_400);
    let ends: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .take(4)
        .map(|(offset, _)| offset + 1)
        .collect();
    let mut begin = 0usize;
    for ((name, end), mtime) in names.iter().zip(&ends).zip([old, old, future, old]) {
        lay_archive(dir.path(), *name, &bytes[begin..*end], mtime);
        begin = *end;
    }
    fs::write(dir.path().join(ACTIVE), &bytes[begin..]).unwrap();
    assert_full_history(dir.path(), &entries);
    let (appender, position) = FjallAuditAppender::open_recovered_batched(
        dir.path(),
        &KEY,
        None,
        FLUSH_INTERVAL,
        Some(1), // every flush rotates: the sweep runs on the rotation
        1,       // retention_days: only the 30-day-old mtimes are eligible
    )
    .unwrap();
    assert_eq!(position, after(&entries[4]));
    let writer = AuditChainWriter::new(
        AuditChain::from_checkpoint(KEY, position.0, position.1),
        appender.clone(),
    );
    let sixth = append(&writer, DURABLE_EVENT);
    drop(writer);
    tokio::time::timeout(HANG_GUARD, appender.flush_now())
        .await
        .unwrap()
        .unwrap();
    let tail = after(&sixth);
    let kept = archive_suffixes(dir.path());
    assert_eq!(&kept[..2], &[names[2], names[3]]);
    assert_eq!(kept.len(), 3);
    // The future-dated blocker was clamped to the present: its delay is
    // bounded by retention_days from now, not by the 30-day clock error.
    let blocker = dir.path().join(format!("audit.log.{:020}", names[2]));
    let clamped = fs::metadata(&blocker).unwrap().modified().unwrap();
    assert!(clamped <= SystemTime::now());
    assert!(clamped > old);
    let newest = kept[2];
    assert!(newest > names[3]);
    drop(appender);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(tail)
    );
    let (strict, strict_position) =
        FjallAuditAppender::open_recovered_strict(dir.path(), &KEY, None).unwrap();
    assert_eq!(strict_position, tail);
    drop(strict);
    // Genesis was pruned with s1/s2: the retained suffix starts at seq 3.
    let retained: Vec<u64> = read_entries(dir.path()).iter().map(|e| e.seq).collect();
    assert_eq!(retained, [3, 4, 5, 6]);
}

fn now_unix_nanos() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    u64::try_from(elapsed.as_nanos()).unwrap()
}

/// Lay out a real archive and pin its mtime (age must not depend on name).
fn lay_archive(dir: &Path, suffix: u64, bytes: &[u8], mtime: SystemTime) {
    let path = dir.join(format!("audit.log.{suffix:020}"));
    fs::write(&path, bytes).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(mtime))
        .unwrap();
}

fn archive_suffixes(dir: &Path) -> Vec<u64> {
    let mut out: Vec<u64> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|entry| recovery::archive_name(&entry.path()).unwrap())
        .collect();
    out.sort_unstable();
    out
}
