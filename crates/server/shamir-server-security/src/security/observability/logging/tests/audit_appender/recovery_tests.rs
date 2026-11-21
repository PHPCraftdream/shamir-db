use super::super::super::{recovery, rotation, AuditRecoveryError, FjallAuditAppender};
use shamir_connect::server::audit_chain::{
    compute_entry_hmac, AuditChain, AuditChainWriter, AuditEntry, AuditError,
};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
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
    // The diagnostic reader includes only exact-pattern archives and the
    // active log — never the ignored files.
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

// Finding 7 — the recovery-side directory-sync seam. Production passes the
// real `rotation::sync_parent`; these tests substitute closures to prove the
// sync is issued only AFTER the verified truncation and the `audit.log`
// reopen/create, and that a sync failure is a typed refusal that leaves the
// prior persisted checkpoint untouched.

/// A torn active tail is truncated before the seam: the closure sees exactly
/// the recovered active bytes, then runs the REAL parent-directory sync.
/// Parent counterfactual: `recover` called `sync_parent` before
/// `set_len`/`open_log`, so the closure would observe the torn byte.
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
            // Truncation already applied and the writable handle already
            // reopened: the active file holds exactly the verified prefix.
            assert_eq!(fs::read(log_path).unwrap(), expected_in_sync);
            rotation::sync_parent(log_path)
        })
        .unwrap();
    assert_eq!(recovered.position, after(&entries[0]));
    assert_eq!(recovered.initial_size, u64::try_from(end).unwrap());
    drop(recovered);
    assert_eq!(fs::read(&path).unwrap(), expected);
}

/// A missing active log is recreated (zero length) before the seam: the
/// closure sees the fresh empty file, then runs the REAL sync.
/// Parent counterfactual: a sync issued before `open_log` would see no file.
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

/// An injected sync failure after a verified truncation is a typed refusal
/// and leaves the prior persisted checkpoint exactly as it was.
/// Parent counterfactual: without the seam (or an untyped panic/mask) the
/// failure would either be unobservable or lose its `AuditRecoveryError::Io`
/// type, and an anchor write could not be ruled out.
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
    // Typed refusal at the exact active log path, carrying the injected
    // source — not a panic or a masked generic error.
    let AuditRecoveryError::Io {
        path: io_path,
        source,
    } = error
    else {
        panic!("unexpected refusal: {error:?}");
    };
    assert_eq!(io_path, dir.path().join(ACTIVE));
    assert_eq!(source.to_string(), "injected directory sync failure");
    // The sync runs after the verified truncation, so the on-disk active
    // file is the truncated verified prefix — but nothing was anchored.
    assert_eq!(fs::read(&path).unwrap(), &bytes[..torn]);
    assert_eq!(
        FjallAuditAppender::load_checkpoint(dir.path()).unwrap(),
        Some(anchor)
    );
}
